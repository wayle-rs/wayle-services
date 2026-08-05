//! Service-level monitoring: keep the WiFi station in sync as IWD comes and
//! goes, fully event-driven.
//!
//! IWD's bus-name ownership is watched via a `NameOwnerChanged` subscription on
//! the bus daemon (reliable across IWD restarts, the analogue of iwgtk's
//! `g_bus_watch_name`). On each appearance the ObjectManager signal subscription
//! is rebuilt against the new owner's *unique* bus name. This is the crux:
//! subscribing by the well-known name makes zbus track the owner behind a
//! `NameOwnerChanged` race that can drop the new owner's signals right after a
//! restart, so the device's `InterfacesAdded` is never delivered and the station
//! stays absent. Binding to the unique name matches `sender == owner` directly,
//! with no tracking, so the device is picked up reactively. Method calls
//! (`GetManagedObjects`) are one-shot RPC and stay on the well-known name.

use std::sync::Arc;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use wayle_core::Property;
use wayle_traits::ServiceMonitoring;
use zbus::{Connection, Proxy, fdo, names::BusName};

use crate::{
    agent::PassphraseStore,
    discovery::{DEVICE_INTERFACE, IwdDiscovery},
    error::Error,
    proxy::object_manager::ObjectManagerProxy,
    service::{IwdService, build_station, register_agent_with_iwd},
    station::Station,
};

/// IWD's well-known bus name.
const IWD_BUS_NAME: &str = "net.connman.iwd";

impl ServiceMonitoring for IwdService {
    type Error = Error;

    async fn start_monitoring(&self) -> Result<(), Self::Error> {
        spawn_station_monitoring(
            self.zbus_connection.clone(),
            self.station.clone(),
            self.passphrases.clone(),
            self.cancellation_token.child_token(),
        )
        .await
    }
}

/// Discover the IWD device and build a live [`Station`]. Returns `None` when no
/// device is present yet (IWD still exporting objects after a restart, or no
/// WiFi hardware).
///
/// A present device means IWD is fully initialised — including its
/// `AgentManager` — so this is also where the passphrase agent is (re)registered
/// with the current IWD instance. Registering here, rather than the instant the
/// bus name appears, avoids racing IWD's object export after a restart.
async fn discover_and_build(
    connection: &Connection,
    cancellation_token: &CancellationToken,
    passphrases: &Arc<PassphraseStore>,
) -> Option<Arc<Station>> {
    let path = match IwdDiscovery::device_path(connection).await {
        Ok(Some(path)) => path,
        Ok(None) => return None,
        Err(err) => {
            debug!(error = %err, "cannot enumerate iwd objects");
            return None;
        }
    };

    register_agent_with_iwd(connection).await;
    build_station(connection, path, cancellation_token, passphrases.clone()).await
}

/// An ObjectManager proxy whose destination is the owner's *unique* name, so its
/// signal streams match `sender == owner` with no well-known-name owner tracking.
async fn object_manager_for(
    connection: &Connection,
    owner: &str,
) -> Result<ObjectManagerProxy<'static>, Error> {
    ObjectManagerProxy::builder(connection)
        .destination(owner.to_owned())
        .map_err(Error::DbusError)?
        .build()
        .await
        .map_err(Error::DbusError)
}

async fn spawn_station_monitoring(
    connection: Connection,
    station: Property<Option<Arc<Station>>>,
    passphrases: Arc<PassphraseStore>,
    cancellation_token: CancellationToken,
) -> Result<(), Error> {
    // Watch IWD's bus-name ownership. This is a signal from the bus daemon (not
    // IWD), so it survives IWD restarts; created once.
    let iwd_peer = Proxy::new(&connection, IWD_BUS_NAME, "/", "org.freedesktop.DBus.Peer")
        .await
        .map_err(Error::DbusError)?;
    let mut owner_changed = iwd_peer.receive_owner_changed().await.map_err(Error::DbusError)?;

    // Current unique owner of net.connman.iwd (None if IWD is down). Bootstrapped
    // here because `receive_owner_changed` reports only *changes*, not the current
    // owner.
    let dbus = fdo::DBusProxy::new(&connection)
        .await
        .map_err(Error::DbusError)?;
    let iwd_bus_name = BusName::try_from(IWD_BUS_NAME)
        .map_err(|err| Error::ServiceInitializationFailed(format!("invalid iwd bus name: {err}")))?;
    let mut current_owner: Option<String> = dbus
        .get_name_owner(iwd_bus_name)
        .await
        .ok()
        .map(|owner| owner.to_string());

    tokio::spawn(async move {
        'session: loop {
            let Some(owner) = current_owner.clone() else {
                // IWD is down: clear the station and wait for it to return.
                if let Some(current) = station.get() {
                    current.shutdown();
                }
                station.replace(None);

                loop {
                    tokio::select! {
                        _ = cancellation_token.cancelled() => return,
                        changed = owner_changed.next() => {
                            let Some(new_owner) = changed else { return };
                            current_owner = new_owner.map(|owner| owner.to_string());
                            if current_owner.is_some() {
                                debug!("iwd appeared on the bus");
                                continue 'session;
                            }
                        }
                    }
                }
            };

            let object_manager = match object_manager_for(&connection, &owner).await {
                Ok(proxy) => proxy,
                Err(err) => {
                    warn!(error = %err, "cannot bind iwd ObjectManager; monitoring stopped");
                    return;
                }
            };

            // Subscribe before enumerating so a device that appears between the two
            // is still caught by the stream.
            let mut interfaces_added = match object_manager.receive_interfaces_added().await {
                Ok(stream) => stream,
                Err(err) => {
                    warn!(error = %err, "cannot watch iwd interfaces-added; monitoring stopped");
                    return;
                }
            };
            let mut interfaces_removed = match object_manager.receive_interfaces_removed().await {
                Ok(stream) => stream,
                Err(err) => {
                    warn!(error = %err, "cannot watch iwd interfaces-removed; monitoring stopped");
                    return;
                }
            };

            // Reconcile an already-present device, but run it CONCURRENTLY with
            // the stream handling below rather than awaiting it here. Right after
            // a restart IWD owns the name but may not yet answer method calls, so
            // `GetManagedObjects` can stall; awaiting it inline would block the
            // whole loop and starve `interfaces_added`, so the device's
            // `InterfacesAdded` would never be consumed. Whichever completes first
            // — this enumeration or an `InterfacesAdded` — builds the station.
            let mut initial_reconcile = std::pin::pin!(async {
                if station.get().is_some() {
                    None
                } else {
                    discover_and_build(&connection, &cancellation_token, &passphrases).await
                }
            });
            let mut initial_reconcile_done = false;

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("iwd station monitoring cancelled");
                        return;
                    }

                    built = &mut initial_reconcile, if !initial_reconcile_done => {
                        initial_reconcile_done = true;
                        if let Some(new_station) = built
                            && station.get().is_none()
                        {
                            debug!("iwd station built from initial scan");
                            station.replace(Some(new_station));
                        }
                    }

                    changed = owner_changed.next() => {
                        // The old station's signal streams are bound to the old
                        // owner and are now dead; tear it down and rebind.
                        if let Some(current) = station.get() {
                            current.shutdown();
                        }
                        let Some(new_owner) = changed else { return };
                        current_owner = new_owner.map(|owner| owner.to_string());
                        station.replace(None);
                        debug!(owner = ?current_owner, "iwd ownership changed");
                        continue 'session;
                    }

                    Some(signal) = interfaces_added.next() => {
                        let Ok(args) = signal.args() else { continue };

                        if !args.interfaces.contains_key(DEVICE_INTERFACE) || station.get().is_some() {
                            continue;
                        }

                        // Build directly from the signalled path — no need to
                        // re-enumerate via GetManagedObjects (which can stall right
                        // after a restart).
                        debug!(path = %args.object_path, "iwd device appeared");
                        register_agent_with_iwd(&connection).await;
                        match build_station(
                            &connection,
                            args.object_path.clone(),
                            &cancellation_token,
                            passphrases.clone(),
                        )
                        .await
                        {
                            Some(new_station) => station.replace(Some(new_station)),
                            None => warn!("iwd device appeared but could not be initialized"),
                        }
                    }

                    Some(signal) = interfaces_removed.next() => {
                        let Ok(args) = signal.args() else { continue };

                        let lost_device = args
                            .interfaces
                            .iter()
                            .any(|iface| iface.as_str() == DEVICE_INTERFACE);
                        if !lost_device {
                            continue;
                        }

                        if let Some(current) = station.get()
                            && current.object_path().as_str() == args.object_path.as_str()
                        {
                            debug!(path = %args.object_path, "iwd device removed");
                            current.shutdown();
                            station.replace(None);
                        }
                    }
                }
            }
        }
    });

    Ok(())
}
