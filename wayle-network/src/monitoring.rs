use std::sync::Arc;

use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use wayle_core::Property;
use wayle_traits::{Reactive, ServiceMonitoring};
use zbus::Connection;

use super::{
    core::settings::Settings,
    discovery::NetworkServiceDiscovery,
    error::Error,
    proxy::manager::NetworkManagerProxy,
    service::NetworkService,
    types::connectivity::ConnectionType,
    wifi::{LiveWifiParams, Wifi},
    wired::{LiveWiredParams, Wired},
};

impl ServiceMonitoring for NetworkService {
    type Error = Error;

    async fn start_monitoring(&self) -> Result<(), Self::Error> {
        spawn_primary_monitoring(
            self.zbus_connection.clone(),
            self.primary.clone(),
            self.cancellation_token.child_token(),
        )
        .await?;

        spawn_device_monitoring(
            self.zbus_connection.clone(),
            self.wifi.clone(),
            self.wired.clone(),
            self.settings.clone(),
            self.cancellation_token.child_token(),
        )
        .await
    }
}

async fn spawn_primary_monitoring(
    connection: Connection,
    primary: Property<ConnectionType>,
    cancellation_token: CancellationToken,
) -> Result<(), Error> {
    let nm_proxy = NetworkManagerProxy::new(&connection)
        .await
        .map_err(Error::DbusError)?;

    let initial_type = nm_proxy.primary_connection_type().await?;
    update_primary_connection(&initial_type, &primary);

    let mut type_changed = nm_proxy.receive_primary_connection_type_changed().await;

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("NetworkMonitoring primary monitoring cancelled");
                    return;
                }
                Some(change) = type_changed.next() => {
                    if let Ok(nm_type) = change.get().await {
                        debug!(nm_type = %nm_type, "Primary connection type changed");
                        update_primary_connection(&nm_type, &primary);
                    }
                }
            }
        }
    });

    Ok(())
}

async fn spawn_device_monitoring(
    connection: Connection,
    wifi: Property<Option<Arc<Wifi>>>,
    wired: Property<Option<Arc<Wired>>>,
    settings: Arc<Settings>,
    cancellation_token: CancellationToken,
) -> Result<(), Error> {
    let nm_proxy = NetworkManagerProxy::new(&connection)
        .await
        .map_err(Error::DbusError)?;

    let mut device_added = nm_proxy.receive_device_added().await?;
    let mut device_removed = nm_proxy.receive_device_removed().await?;
    let mut active_connections_changed = nm_proxy.receive_active_connections_changed().await;

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("NetworkMonitoring device monitoring cancelled");
                    return;
                }
                Some(signal) = device_added.next() => {
                    let Ok(args) = signal.args() else { continue };
                    debug!(path = %args.device_path, "Network device added");

                    try_initialize_wifi(&connection, &wifi, &settings, &cancellation_token).await;
                    sync_wired(&connection, &wired, &cancellation_token).await;
                }
                Some(signal) = device_removed.next() => {
                    let Ok(args) = signal.args() else { continue };
                    debug!(path = %args.device_path, "Network device removed");

                    handle_wifi_removed(&args.device_path, &wifi);
                    handle_wired_removed(&args.device_path, &wired);
                    sync_wired(&connection, &wired, &cancellation_token).await;
                }
                Some(_) = active_connections_changed.next() => {
                    sync_wired(&connection, &wired, &cancellation_token).await;
                }
            }
        }
    });

    Ok(())
}

async fn try_initialize_wifi(
    connection: &Connection,
    wifi: &Property<Option<Arc<Wifi>>>,
    settings: &Arc<Settings>,
    cancellation_token: &CancellationToken,
) {
    if wifi.get().is_some() {
        return;
    }

    let Some(path) = NetworkServiceDiscovery::wifi_device_path(connection)
        .await
        .ok()
        .flatten()
    else {
        return;
    };

    match Wifi::get_live(LiveWifiParams {
        connection,
        device_path: path.clone(),
        cancellation_token,
        settings: settings.clone(),
    })
    .await
    {
        Ok(new_wifi) => {
            debug!(path = %path, "WiFi device initialized");
            wifi.set(Some(new_wifi));
        }
        Err(err) => {
            warn!(error = %err, path = %path, "Failed to initialize WiFi device");
        }
    }
}

/// Binds `wired` to the ethernet device NetworkManager is actually using.
///
/// Keeps the current device while it has an active connection, otherwise
/// switches to the first ethernet device with one (e.g. a dock or USB
/// adapter that appeared after startup), falling back to any ethernet device.
async fn sync_wired(
    connection: &Connection,
    wired: &Property<Option<Arc<Wired>>>,
    cancellation_token: &CancellationToken,
) {
    let current = wired.get();

    if let Some(current) = &current {
        let current_path = &current.device.core.object_path;
        if NetworkServiceDiscovery::has_active_connection(connection, current_path)
            .await
            .unwrap_or(false)
        {
            return;
        }
    }

    let Some(path) = NetworkServiceDiscovery::wired_device_path(connection)
        .await
        .ok()
        .flatten()
    else {
        return;
    };

    if current
        .as_ref()
        .is_some_and(|current| current.device.core.object_path == path)
    {
        return;
    }

    match Wired::get_live(LiveWiredParams {
        connection,
        device_path: path.clone(),
        cancellation_token,
    })
    .await
    {
        Ok(new_wired) => {
            debug!(path = %path, "Wired device initialized");
            wired.set(Some(new_wired));

            if let Some(old_token) = current
                .as_ref()
                .and_then(|old| old.device.core.cancellation_token.as_ref())
            {
                old_token.cancel();
            }
        }
        Err(err) => {
            warn!(error = %err, path = %path, "Failed to initialize wired device");
        }
    }
}

fn handle_wifi_removed(device_path: &str, wifi: &Property<Option<Arc<Wifi>>>) {
    let Some(current) = wifi.get() else { return };

    if current.device.core.object_path.as_str() == device_path {
        debug!(path = %device_path, "WiFi device removed");
        wifi.set(None);
    }
}

fn handle_wired_removed(device_path: &str, wired: &Property<Option<Arc<Wired>>>) {
    let Some(current) = wired.get() else { return };

    if current.device.core.object_path.as_str() == device_path {
        debug!(path = %device_path, "Wired device removed");
        wired.set(None);
    }
}

fn update_primary_connection(nm_type: &str, primary: &Property<ConnectionType>) {
    let connection_type = ConnectionType::from_nm_type(nm_type);
    debug!(?connection_type, "Primary connection type resolved");
    primary.set(connection_type);
}
