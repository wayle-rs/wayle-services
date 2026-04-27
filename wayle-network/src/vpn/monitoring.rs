use std::sync::{Arc, Weak};

use tokio::sync::Notify;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use wayle_traits::{ModelMonitoring, Reactive};
use zbus::zvariant::OwnedObjectPath;

use super::Vpn;
use crate::{
    core::{
        connection::{ActiveConnection, LiveActiveConnectionParams},
        settings::Settings,
    },
    error::Error,
    proxy::manager::NetworkManagerProxy,
};

impl ModelMonitoring for Vpn {
    type Error = Error;

    async fn start_monitoring(self: Arc<Self>) -> Result<(), Self::Error> {
        let Some(ref cancellation_token) = self.cancellation_token else {
            return Err(Error::MissingCancellationToken);
        };

        populate_active_vpns(&self).await;

        let nm_proxy = NetworkManagerProxy::new(&self.zbus_connection)
            .await
            .map_err(Error::DbusError)?;

        let cancel = cancellation_token.clone();
        let weak_self = Arc::downgrade(&self);
        let settings = self.settings.clone();

        tokio::spawn(async move {
            monitor_vpn(weak_self, nm_proxy, settings, cancel).await;
        });

        Ok(())
    }
}

async fn populate_active_vpns(vpn: &Arc<Vpn>) {
    let Some(ref cancellation_token) = vpn.cancellation_token else {
        return;
    };
    let Ok(nm_proxy) = NetworkManagerProxy::new(&vpn.zbus_connection).await else {
        return;
    };
    let Ok(active_paths) = nm_proxy.active_connections().await else {
        return;
    };

    let mut actives = Vec::new();
    for path in active_paths {
        let Ok(active) = ActiveConnection::get_live(LiveActiveConnectionParams {
            connection: &vpn.zbus_connection,
            path,
            cancellation_token,
        })
        .await
        else {
            continue;
        };

        if Vpn::is_vpn_active(&active) {
            actives.push(active);
        }
    }

    let connectivity = Vpn::connectivity_for(&actives);
    let banner = Vpn::banner_of_first(&vpn.zbus_connection, &actives).await;

    vpn.connectivity.set(connectivity);
    vpn.banner.set(banner);
    vpn.active_connections.set(actives);
}

async fn monitor_vpn(
    weak_vpn: Weak<Vpn>,
    nm_proxy: NetworkManagerProxy<'static>,
    settings: Arc<Settings>,
    cancellation_token: CancellationToken,
) {
    let mut active_connections_changed = nm_proxy.receive_active_connections_changed().await;
    let mut profile_stream = settings.connections.watch();

    let recompute = Arc::new(Notify::new());
    let mut group_token: CancellationToken = {
        let Some(vpn) = weak_vpn.upgrade() else {
            return;
        };
        let new_group = cancellation_token.child_token();
        spawn_state_watchers(
            &vpn.active_connections.get(),
            recompute.clone(),
            new_group.clone(),
        );
        new_group
    };

    loop {
        let Some(vpn) = weak_vpn.upgrade() else {
            return;
        };

        tokio::select! {
            _ = cancellation_token.cancelled() => {
                debug!("VpnMonitor cancelled");
                group_token.cancel();
                return;
            }

            Some(change) = active_connections_changed.next() => {
                if let Ok(paths) = change.get().await {
                    group_token.cancel();
                    let new_group = cancellation_token.child_token();
                    handle_active_connections_changed(&vpn, paths).await;
                    spawn_state_watchers(
                        &vpn.active_connections.get(),
                        recompute.clone(),
                        new_group.clone(),
                    );
                    group_token = new_group;
                    refresh_aggregates(&vpn).await;
                }
            }

            Some(connections) = profile_stream.next() => {
                let vpn_connections = connections
                    .into_iter()
                    .filter(Vpn::is_vpn_settings)
                    .collect();
                vpn.connections.set(vpn_connections);
            }

            () = recompute.notified() => {
                refresh_aggregates(&vpn).await;
            }

            else => {
                break;
            }
        }
    }
}

async fn handle_active_connections_changed(vpn: &Arc<Vpn>, paths: Vec<OwnedObjectPath>) {
    let Some(ref cancellation_token) = vpn.cancellation_token else {
        return;
    };

    let mut current = vpn.active_connections.get();
    current.retain(|existing| paths.contains(&existing.object_path));

    for path in paths {
        if current
            .iter()
            .any(|existing| existing.object_path == path)
        {
            continue;
        }

        let Ok(active) = ActiveConnection::get_live(LiveActiveConnectionParams {
            connection: &vpn.zbus_connection,
            path,
            cancellation_token,
        })
        .await
        else {
            continue;
        };

        if Vpn::is_vpn_active(&active) {
            current.push(active);
        }
    }

    vpn.active_connections.set(current);
}

fn spawn_state_watchers(
    actives: &[Arc<ActiveConnection>],
    recompute: Arc<Notify>,
    group_token: CancellationToken,
) {
    for active in actives {
        let mut state_stream = active.state.watch();
        let notify = recompute.clone();
        let token = group_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => return,
                    next = state_stream.next() => {
                        if next.is_none() {
                            return;
                        }
                        notify.notify_one();
                    }
                }
            }
        });
    }
}

async fn refresh_aggregates(vpn: &Arc<Vpn>) {
    let actives = vpn.active_connections.get();
    let connectivity = Vpn::connectivity_for(&actives);
    let banner = Vpn::banner_of_first(&vpn.zbus_connection, &actives).await;

    vpn.connectivity.set(connectivity);
    vpn.banner.set(banner);
}
