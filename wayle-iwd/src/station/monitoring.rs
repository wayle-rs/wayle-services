//! Background property monitoring for a [`Station`].

use std::sync::{Arc, Weak};

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use wayle_core::Property;
use wayle_traits::ModelMonitoring;
use zbus::{
    Connection,
    zvariant::{ObjectPath, OwnedObjectPath},
};

use super::Station;
use crate::{
    discovery::{NETWORK_INTERFACE, STATION_INTERFACE},
    error::Error,
    proxy::{
        device::DeviceProxy, diagnostic::StationDiagnosticProxy, object_manager::ObjectManagerProxy,
        station::StationProxy,
    },
    signal_agent::{SIGNAL_LEVEL_AGENT_PATH, SignalLevelAgent},
    types::{ConnectionState, SIGNAL_STRENGTH_THRESHOLDS, SignalStrength},
};

impl ModelMonitoring for Station {
    type Error = Error;

    async fn start_monitoring(self: Arc<Self>) -> Result<(), Self::Error> {
        let Some(ref cancellation_token) = self.cancellation_token else {
            return Err(Error::MissingCancellationToken);
        };
        let cancel = cancellation_token.clone();

        let device_proxy = DeviceProxy::new(&self.zbus_connection, self.object_path.clone())
            .await
            .map_err(Error::DbusError)?;
        let object_manager = ObjectManagerProxy::new(&self.zbus_connection)
            .await
            .map_err(Error::DbusError)?;

        let connection = self.zbus_connection.clone();
        let object_path = self.object_path.clone();
        let weak_self = Arc::downgrade(&self);
        tokio::spawn(async move {
            monitor(
                weak_self,
                device_proxy,
                object_manager,
                connection,
                object_path,
                cancel,
            )
            .await;
        });

        Ok(())
    }
}

/// Whether `candidate` is an object underneath `parent` (i.e. a network object
/// belonging to our device).
fn is_descendant(candidate: &OwnedObjectPath, parent: &OwnedObjectPath) -> bool {
    candidate
        .as_str()
        .strip_prefix(parent.as_str())
        .is_some_and(|rest| rest.starts_with('/'))
}

async fn monitor(
    weak_station: Weak<Station>,
    device_proxy: DeviceProxy<'static>,
    object_manager: ObjectManagerProxy<'static>,
    connection: Connection,
    object_path: OwnedObjectPath,
    cancellation_token: CancellationToken,
) {
    // Device-level streams persist across power toggles: the `Device` interface
    // (the Powered switch) and the ObjectManager survive while the `Station`
    // interface comes and goes.
    let mut powered_changed = device_proxy.receive_powered_changed().await;
    let mut interfaces_added = match object_manager.receive_interfaces_added().await {
        Ok(stream) => stream,
        Err(err) => {
            debug!(error = %err, "cannot watch interfaces added");
            return;
        }
    };
    let mut interfaces_removed = match object_manager.receive_interfaces_removed().await {
        Ok(stream) => stream,
        Err(err) => {
            debug!(error = %err, "cannot watch interfaces removed");
            return;
        }
    };

    // Outer loop: (re)bind the Station-interface property streams. Mirrors iwgtk,
    // which recreates its Station object — a fresh proxy and a fresh property
    // subscription — every time the `Station` interface (re)appears. The previous
    // subscription does not survive the interface being removed on power-off, so
    // re-creating it here is what makes the post-power-on autoconnect/scan
    // transitions visible. `continue 'session` rebinds when the interface
    // reappears (handled in the `interfaces_added` arm below).
    'session: loop {
        let station_proxy = match StationProxy::new(&connection, object_path.clone()).await {
            Ok(proxy) => proxy,
            Err(err) => {
                debug!(error = %err, "cannot create station proxy");
                return;
            }
        };
        let mut state_changed = station_proxy.receive_state_changed().await;
        let mut scanning_changed = station_proxy.receive_scanning_changed().await;
        let mut connected_changed = station_proxy.receive_connected_network_changed().await;

        // Resync from the freshly-bound interface: register the signal-level
        // agent and seed scanning/connection/diagnostics/networks from the live
        // state. A no-op (read fails gracefully) while the interface is absent
        // (device powered off) — we then wait for `interfaces_added` to rebind.
        match weak_station.upgrade() {
            Some(station) => resync(&station, &station_proxy).await,
            None => return,
        }

        loop {
            let Some(station) = weak_station.upgrade() else {
                return;
            };

            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("station monitor cancelled");
                    return;
                }

                // Reflect the Powered switch. State (re)sync is driven by the
                // Station interface appearing/disappearing, not by this property.
                Some(change) = powered_changed.next() => {
                    if let Ok(powered) = change.get().await {
                        station.powered.set(powered);
                    }
                }

                // `State` and `ConnectedNetwork` are reconciled identically — both
                // re-read a coherent live snapshot rather than trusting the
                // (possibly already-stale) signal payload, so the published state
                // is never torn between the two properties.
                Some(_) = state_changed.next() => {
                    reconcile_connection(&station).await;
                }

                Some(change) = scanning_changed.next() => {
                    if let Ok(scanning) = change.get().await {
                        station.scanning.set(scanning);
                        // A finished scan means fresh ordered-network results.
                        if !scanning {
                            station.refresh_networks().await;
                        }
                    }
                }

                Some(_) = connected_changed.next() => {
                    reconcile_connection(&station).await;
                }

                Some(signal) = interfaces_added.next() => {
                    let Ok(args) = signal.args() else { continue };

                    // The Station interface (re)appeared on our device (power-on):
                    // rebind its property streams against the live interface so the
                    // ensuing autoconnect/scan transitions are observed.
                    if args.interfaces.contains_key(STATION_INTERFACE)
                        && args.object_path.as_str() == object_path.as_str()
                    {
                        continue 'session;
                    }

                    // During a scan IWD adds network objects in a rapid burst;
                    // refreshing per-object would churn the list (and the open
                    // dropdown) many times a second. Skip while scanning — the
                    // `scanning -> false` edge above refreshes once when results
                    // are final. Only out-of-scan appearances refresh here.
                    if station.powered.get()
                        && !station.scanning.get()
                        && args.interfaces.contains_key(NETWORK_INTERFACE)
                        && is_descendant(&args.object_path, &object_path)
                    {
                        station.refresh_networks().await;
                    }
                }

                Some(signal) = interfaces_removed.next() => {
                    let Ok(args) = signal.args() else { continue };

                    // The Station interface went away (power-off): clear all
                    // station-derived state. The stale streams are left until the
                    // interface reappears and rebinds them.
                    if args.interfaces.iter().any(|iface| iface.as_str() == STATION_INTERFACE)
                        && args.object_path.as_str() == object_path.as_str()
                    {
                        station.connection.set(ConnectionState::Idle);
                        station.scanning.set(false);
                        station.strength.set(None);
                        station.frequency.set(None);
                        station.networks.replace(Vec::new());
                    } else if station.powered.get()
                        && !station.scanning.get()
                        && args.interfaces.iter().any(|iface| iface.as_str() == NETWORK_INTERFACE)
                        && is_descendant(&args.object_path, &object_path)
                    {
                        station.refresh_networks().await;
                    }
                }

                else => {
                    return;
                }
            }
        }
    }
}

/// (Re)synchronise all station-derived state from a freshly-bound `Station`
/// interface: register the signal-level agent, seed `scanning`, and reconcile the
/// connection (plus diagnostics and the network list). Called once each time the
/// interface is (re)bound.
///
/// Does not initiate a scan: the network list is populated from IWD's cached
/// `GetOrderedNetworks` results, and IWD performs its own scans (e.g. for
/// autoconnect). Explicit scans are user-driven via the dropdown's scan button.
async fn resync(station: &Station, station_proxy: &StationProxy<'static>) {
    // Re-register the SignalLevelAgent so IWD pushes bucketed strength for the
    // (re)appeared interface. Best-effort: if it fails, strength still comes from
    // the connect-time diagnostics snapshot.
    setup_signal_level_agent(&station.zbus_connection, station_proxy, station.strength.clone()).await;

    station
        .scanning
        .set(station_proxy.scanning().await.unwrap_or(false));

    reconcile_connection(station).await;
}

/// Reconcile all monitor-derived state from a single coherent live snapshot:
/// publish the connection state (unless a foreground [`connect`](Station::connect)
/// owns it), keep diagnostics current for an active link or clear them otherwise,
/// and refresh the network list.
///
/// Skips entirely when the live state cannot be read, preserving the last known
/// state rather than flickering a live connection to disconnected — the failure
/// mode seen when signal streams go briefly stale across suspend/resume.
async fn reconcile_connection(station: &Station) {
    let Some((state, connected_ssid)) = station.read_station_snapshot().await else {
        return;
    };

    station.observe_connection(&state, connected_ssid);

    // Roaming is still an active link, so keep its diagnostics (strength/
    // frequency) current rather than clearing them.
    if state == "connected" || state == "roaming" {
        update_diagnostics(station).await;
    } else {
        station.strength.set(None);
        station.frequency.set(None);
    }

    station.refresh_networks().await;
}

/// Read diagnostics (RSSI -> strength, frequency) and publish them. Called from
/// the `state_changed` "connected" branch so frequency (which the
/// `SignalLevelAgent` does not report) and an initial strength appear as soon as
/// the link comes up; ongoing strength then arrives via the agent.
async fn update_diagnostics(station: &Station) {
    let Ok(proxy) = StationDiagnosticProxy::new(&station.zbus_connection, station.object_path.clone()).await
    else {
        return;
    };

    let Ok(diagnostics) = proxy.get_diagnostics().await else {
        // Diagnostics may require elevated privileges; treat as unavailable.
        return;
    };

    if let Some(rssi) = diagnostics.get("RSSI").and_then(|v| i16::try_from(v).ok()) {
        station.strength.set(Some(SignalStrength::from_dbm(rssi)));
    }

    if let Some(frequency) = diagnostics.get("Frequency").and_then(|v| u32::try_from(v).ok()) {
        station.frequency.set(Some(frequency));
    }
}

/// Serve and register a [`SignalLevelAgent`] so IWD pushes the connected link's
/// bucketed strength. Best-effort: any failure is logged and strength then
/// updates only via the connect-time snapshot. `remove`-before-`at` keeps
/// registration idempotent across a device re-plug (a stale object from a
/// previous station is cleared first).
pub(super) async fn setup_signal_level_agent(
    connection: &Connection,
    station_proxy: &StationProxy<'static>,
    strength: Property<Option<SignalStrength>>,
) {
    let Ok(path) = ObjectPath::try_from(SIGNAL_LEVEL_AGENT_PATH) else {
        return;
    };

    let server = connection.object_server();
    let _ = server
        .remove::<SignalLevelAgent, _>(SIGNAL_LEVEL_AGENT_PATH)
        .await;

    if let Err(err) = server
        .at(SIGNAL_LEVEL_AGENT_PATH, SignalLevelAgent::new(strength))
        .await
    {
        debug!(error = %err, "cannot serve iwd signal-level agent; strength updates on connect only");
        return;
    }

    if let Err(err) = station_proxy
        .register_signal_level_agent(&path, &SIGNAL_STRENGTH_THRESHOLDS)
        .await
    {
        debug!(error = %err, "cannot register iwd signal-level agent; strength updates on connect only");
        let _ = server
            .remove::<SignalLevelAgent, _>(SIGNAL_LEVEL_AGENT_PATH)
            .await;
    }
}
