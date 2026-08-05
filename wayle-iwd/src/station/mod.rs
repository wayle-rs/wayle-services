//! WiFi station facade (the analogue of `wayle-network`'s `Wifi`).
//!
//! A single IWD device object implements the `Device`, `Station`, and
//! `StationDiagnostic` interfaces. [`Station`] wraps that object, exposing
//! reactive [`Property`] state and connection controls.

mod monitoring;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use derive_more::Debug;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use wayle_core::{NULL_PATH, Property};
use wayle_traits::{ModelMonitoring, Reactive};
use zbus::{Connection, zvariant::OwnedObjectPath};

use crate::{
    agent::PassphraseStore,
    error::Error,
    network::Network,
    proxy::{
        adapter::AdapterProxy, device::DeviceProxy, network::NetworkProxy, station::StationProxy,
    },
    types::{ConnectionState, SignalStrength},
};

#[doc(hidden)]
pub struct StationParams<'a> {
    pub(crate) connection: &'a Connection,
    pub(crate) device_path: OwnedObjectPath,
    pub(crate) passphrases: Arc<PassphraseStore>,
}

#[doc(hidden)]
pub struct LiveStationParams<'a> {
    pub(crate) connection: &'a Connection,
    pub(crate) device_path: OwnedObjectPath,
    pub(crate) cancellation_token: &'a CancellationToken,
    pub(crate) passphrases: Arc<PassphraseStore>,
}

pub(crate) fn is_real_path(path: &OwnedObjectPath) -> bool {
    let s = path.as_str();
    !s.is_empty() && s != NULL_PATH
}

/// Whether a D-Bus error is the named IWD method error (e.g.
/// `net.connman.iwd.Failed`).
fn is_iwd_error(err: &zbus::Error, name: &str) -> bool {
    matches!(err, zbus::Error::MethodError(error_name, _, _) if error_name.as_str() == name)
}

/// RAII marker that a foreground [`Station::connect`] is in progress. While any
/// guard is alive the background monitor refrains from writing
/// [`Station::connection`] (see [`Station::observe_connection`]), so the
/// foreground attempt fully owns that state and transient IWD signals during a
/// network switch cannot clobber the in-flight target. The count handles
/// overlapping attempts (a new connect superseding a pending one).
struct AttemptGuard(Arc<AtomicUsize>);

impl AttemptGuard {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter.clone())
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A WiFi station: connection state, scan results, and controls.
#[derive(Clone, Debug)]
pub struct Station {
    #[debug(skip)]
    zbus_connection: Connection,
    object_path: OwnedObjectPath,
    #[debug(skip)]
    cancellation_token: Option<CancellationToken>,
    #[debug(skip)]
    passphrases: Arc<PassphraseStore>,
    /// Number of foreground [`connect`](Self::connect) attempts in progress.
    /// While non-zero the monitor leaves [`connection`](Self::connection) to the
    /// foreground attempt; see [`AttemptGuard`].
    pending_connects: Arc<AtomicUsize>,
    /// Whether the underlying device is powered on (the WiFi enable toggle).
    pub powered: Property<bool>,
    /// Attempt-aware connection state: the active or in-progress connection and
    /// its target SSID. The single source of truth for the "active connection"
    /// UI, reconciled from IWD's `Station.State` + `ConnectedNetwork` and from
    /// foreground [`connect`](Self::connect) attempts.
    pub connection: Property<ConnectionState>,
    /// Whether a scan is in progress.
    pub scanning: Property<bool>,
    /// Bucketed signal strength of the connected link. Pushed by IWD's
    /// `SignalLevelAgent` as the RSSI crosses thresholds, plus a snapshot read
    /// when a connection comes up.
    pub strength: Property<Option<SignalStrength>>,
    /// Frequency of the connected link in MHz, from diagnostics.
    pub frequency: Property<Option<u32>>,
    /// Visible networks, ordered strongest-first.
    pub networks: Property<Vec<Arc<Network>>>,
}

impl PartialEq for Station {
    fn eq(&self, other: &Self) -> bool {
        self.object_path == other.object_path
    }
}

impl Reactive for Station {
    type Context<'a> = StationParams<'a>;
    type LiveContext<'a> = LiveStationParams<'a>;
    type Error = Error;

    async fn get(params: Self::Context<'_>) -> Result<Self, Self::Error> {
        Self::from_path(
            params.connection,
            params.device_path,
            None,
            params.passphrases,
        )
        .await
    }

    async fn get_live(params: Self::LiveContext<'_>) -> Result<Arc<Self>, Self::Error> {
        let station = Self::from_path(
            params.connection,
            params.device_path,
            Some(params.cancellation_token.child_token()),
            params.passphrases,
        )
        .await?;
        let station = Arc::new(station);
        station.clone().start_monitoring().await?;
        Ok(station)
    }
}

impl Station {
    /// D-Bus object path of the station device.
    pub(crate) fn object_path(&self) -> &OwnedObjectPath {
        &self.object_path
    }

    /// Cancel this station's background monitor. Called when the station is being
    /// replaced (e.g. IWD restarted, or the device was removed) so the old
    /// monitor task exits promptly instead of lingering with dead signal streams.
    pub(crate) fn shutdown(&self) {
        if let Some(token) = &self.cancellation_token {
            token.cancel();
        }
    }

    /// Enable or disable the WiFi device (`Device.Powered`).
    ///
    /// A device cannot be powered on while its parent adapter is powered off, so
    /// when enabling we first power the adapter on (a no-op if it already is).
    /// This makes the toggle recover an adapter that was switched off (e.g. via
    /// `iwctl adapter ... set-property Powered off`) rather than failing.
    ///
    /// # Errors
    /// Returns [`Error::OperationFailed`] if the D-Bus call fails.
    pub async fn set_powered(&self, on: bool) -> Result<(), Error> {
        let device = DeviceProxy::new(&self.zbus_connection, self.object_path.clone())
            .await
            .map_err(|e| Error::OperationFailed {
                operation: "create device proxy",
                source: e.into(),
            })?;

        if on {
            self.power_on_adapter(&device).await?;
        }

        device.set_powered(on).await.map_err(|e| Error::OperationFailed {
            operation: "set device powered",
            source: e.into(),
        })
    }

    /// Ensure this device's parent adapter is powered on. A no-op if the adapter
    /// is already on (or its path/state cannot be read — in which case the
    /// subsequent device power-on surfaces any real error).
    async fn power_on_adapter(&self, device: &DeviceProxy<'_>) -> Result<(), Error> {
        let Ok(adapter_path) = device.adapter().await else {
            return Ok(());
        };

        let adapter = AdapterProxy::new(&self.zbus_connection, adapter_path)
            .await
            .map_err(|e| Error::OperationFailed {
                operation: "create adapter proxy",
                source: e.into(),
            })?;

        if adapter.powered().await.unwrap_or(true) {
            return Ok(());
        }

        adapter.set_powered(true).await.map_err(|e| Error::OperationFailed {
            operation: "set adapter powered",
            source: e.into(),
        })
    }

    /// Request a scan for networks.
    ///
    /// # Errors
    /// Returns [`Error::OperationFailed`] if the D-Bus call fails.
    pub async fn scan(&self) -> Result<(), Error> {
        let station = self.station_proxy().await?;
        station.scan().await.map_err(|e| Error::OperationFailed {
            operation: "scan",
            source: e.into(),
        })
    }

    /// Disconnect from the current network.
    ///
    /// Cancelling an in-flight [`connect`](Self::connect) is just a disconnect, so
    /// on success this publishes [`Idle`](ConnectionState::Idle) immediately rather
    /// than waiting for the pending `connect` to reconcile — the UI never lingers
    /// on "Connecting" for a connection the user already cancelled.
    ///
    /// # Errors
    /// Returns [`Error::OperationFailed`] if the D-Bus call fails.
    pub async fn disconnect(&self) -> Result<(), Error> {
        let station = self.station_proxy().await?;
        let result = station.disconnect().await.map_err(|e| Error::OperationFailed {
            operation: "disconnect",
            source: e.into(),
        });
        if result.is_ok() {
            self.connection.set(ConnectionState::Idle);
        }
        result
    }

    /// Connect to a network by object path.
    ///
    /// For secured networks, stage the `passphrase` (delivered to IWD via the
    /// agent's `RequestPassphrase`). For open or already-known networks, pass
    /// `None`. Resolves once IWD reports success, or returns an error (e.g. on
    /// a wrong passphrase).
    ///
    /// For the duration of the call this attempt owns [`connection`](Self::connection)
    /// (the monitor steps back — see [`observe_connection`](Self::observe_connection)),
    /// publishing the target optimistically and reconciling to the true live
    /// state on completion.
    ///
    /// # Errors
    /// Returns [`Error::ConnectionFailed`] on a rejected passphrase,
    /// [`Error::ConnectionAborted`] if cancelled/superseded, or
    /// [`Error::OperationFailed`] for any other failure.
    pub async fn connect(
        &self,
        network_path: OwnedObjectPath,
        passphrase: Option<String>,
    ) -> Result<(), Error> {
        // Take ownership of `connection` for the lifetime of the attempt so the
        // monitor's transient signals during a network switch cannot clobber the
        // in-flight target. Reconciliation to the true state happens below,
        // before the guard drops.
        let _attempt = AttemptGuard::new(&self.pending_connects);

        if let Some(passphrase) = passphrase {
            self.passphrases.insert(network_path.clone(), passphrase);
        }

        let proxy = match NetworkProxy::new(&self.zbus_connection, network_path.clone()).await {
            Ok(proxy) => proxy,
            Err(err) => {
                self.passphrases.discard(&network_path);
                self.reconcile_connection_from_live().await;
                return Err(Error::OperationFailed {
                    operation: "create network proxy",
                    source: err.into(),
                });
            }
        };

        // Resolve the target SSID (cached scan list first, else the proxy name)
        // and publish the in-flight target immediately for instant, flicker-free
        // UI. Set unconditionally so the most recent attempt's target always wins.
        let target_ssid = match self.network_ssid(&network_path) {
            Some(ssid) => Some(ssid),
            None => proxy.name().await.ok(),
        };
        if let Some(ssid) = target_ssid.clone() {
            self.connection.set(ConnectionState::Connecting { ssid });
        }

        let result = proxy.connect().await;
        self.passphrases.discard(&network_path);

        // Publish the terminal state — but only when no *other* attempt is still
        // in flight, so a superseded attempt finishing first cannot clobber a
        // newer attempt's in-flight target (the newer attempt owns `connection`
        // and will publish its own outcome).
        if !self.other_attempt_in_flight() {
            match (&result, target_ssid) {
                // Success: we are connected to the target — publish it directly,
                // no live read required.
                (Ok(()), Some(ssid)) => {
                    self.connection.set(ConnectionState::Connected { ssid });
                }
                // Otherwise reconcile to whatever IWD actually settled on: success
                // with an unknown target, a rejected passphrase that left us on the
                // previous network, or a plain failure.
                _ => self.reconcile_connection_from_live().await,
            }
        }

        result.map_err(|e| {
            if is_iwd_error(&e, "net.connman.iwd.Failed") {
                Error::ConnectionFailed
            } else if is_iwd_error(&e, "net.connman.iwd.Aborted") {
                Error::ConnectionAborted
            } else {
                Error::OperationFailed {
                    operation: "connect to network",
                    source: e.into(),
                }
            }
        })
    }

    /// SSID of a network in the current scan list, by object path.
    fn network_ssid(&self, network_path: &OwnedObjectPath) -> Option<String> {
        self.networks
            .get()
            .iter()
            .find(|network| network.object_path() == network_path)
            .map(|network| network.ssid.get())
    }

    /// Whether a foreground [`connect`](Self::connect) currently owns
    /// [`connection`](Self::connection).
    fn connecting_in_flight(&self) -> bool {
        self.pending_connects.load(Ordering::SeqCst) > 0
    }

    /// Whether a foreground [`connect`](Self::connect) *other than the caller's*
    /// is in flight. The caller holds its own [`AttemptGuard`], so a count above
    /// one means a newer attempt is running and owns [`connection`](Self::connection).
    fn other_attempt_in_flight(&self) -> bool {
        self.pending_connects.load(Ordering::SeqCst) > 1
    }

    /// Publish [`connection`](Self::connection) from an observed raw `Station.State`
    /// and resolved SSID — the authoritative driver for connections from any
    /// client (including external ones such as `iwctl`).
    ///
    /// A no-op while a foreground [`connect`](Self::connect) is in flight: that
    /// attempt owns `connection` and the coarse signals seen here during a
    /// network switch would otherwise clobber its in-flight target.
    pub(crate) fn observe_connection(&self, state: &str, connected_ssid: Option<String>) {
        if self.connecting_in_flight() {
            return;
        }
        self.connection
            .set(ConnectionState::from_raw_state(state, connected_ssid));
    }

    /// Reconcile [`connection`](Self::connection) to the live `Station.State` and
    /// `ConnectedNetwork`. Called by [`connect`](Self::connect) on completion to
    /// publish the true outcome, bypassing the in-flight guard (it *is* the owner
    /// finishing). Falls back to [`Idle`](ConnectionState::Idle) if the live state
    /// cannot be read — a just-failed attempt with an unreadable interface is
    /// treated as disconnected.
    async fn reconcile_connection_from_live(&self) {
        let connection = match self.read_station_snapshot().await {
            Some((state, connected_ssid)) => {
                ConnectionState::from_raw_state(&state, connected_ssid)
            }
            None => ConnectionState::Idle,
        };
        self.connection.set(connection);
    }

    /// Read a coherent snapshot of the live `Station.State` and connected SSID.
    ///
    /// Returns `None` only if the `Station` interface is transiently unavailable
    /// (e.g. across suspend/resume, or while IWD is restarting) so the background
    /// monitor can preserve the last known state instead of flickering a live
    /// connection to disconnected. See [`read_station`] for the coherence guarantee.
    pub(crate) async fn read_station_snapshot(&self) -> Option<(String, Option<String>)> {
        read_station(&self.zbus_connection, &self.object_path)
            .await
            .map(|(state, _scanning, connected_ssid)| (state, connected_ssid))
    }

    async fn station_proxy(&self) -> Result<StationProxy<'static>, Error> {
        StationProxy::new(&self.zbus_connection, self.object_path.clone())
            .await
            .map_err(Error::DbusError)
    }

    /// Re-fetch the ordered network list from IWD and publish it.
    ///
    /// When the device is powered off the `Station` interface is absent, so the
    /// list is simply cleared.
    pub(crate) async fn refresh_networks(&self) {
        if !self.powered.get() {
            self.networks.replace(Vec::new());
            return;
        }

        let Ok(station) = self.station_proxy().await else {
            return;
        };

        let ordered = match station.get_ordered_networks().await {
            Ok(ordered) => ordered,
            Err(err) => {
                debug!(error = %err, "cannot fetch ordered networks");
                return;
            }
        };

        let mut networks = Vec::with_capacity(ordered.len());
        for (path, signal) in ordered {
            // `GetOrderedNetworks` reports 100 * dBm; bucket the plain-dBm value.
            // Floor (not truncate-toward-zero) so a fractional negative RSSI like
            // -74.5 dBm buckets as the weaker -75, not the stronger -74.
            let strength = SignalStrength::from_dbm(signal.div_euclid(100));
            if let Ok(network) = Network::from_path(&self.zbus_connection, path, strength).await {
                networks.push(Arc::new(network));
            }
        }

        self.networks.replace(networks);
    }

    async fn from_path(
        connection: &Connection,
        path: OwnedObjectPath,
        cancellation_token: Option<CancellationToken>,
        passphrases: Arc<PassphraseStore>,
    ) -> Result<Self, Error> {
        // Presence is keyed on the Device interface, which survives power-off
        // (the Station interface is removed while powered down).
        let device_proxy = DeviceProxy::new(connection, path.clone())
            .await
            .map_err(Error::DbusError)?;

        let Ok(powered) = device_proxy.powered().await else {
            return Err(Error::ObjectNotFound(path.clone()));
        };

        let (state, scanning, connected_ssid) = if powered {
            read_station(connection, &path).await.unwrap_or_default()
        } else {
            (String::new(), false, None)
        };

        let connection_state = ConnectionState::from_raw_state(&state, connected_ssid);

        Ok(Self {
            zbus_connection: connection.clone(),
            object_path: path,
            cancellation_token,
            passphrases,
            pending_connects: Arc::new(AtomicUsize::new(0)),
            powered: Property::new(powered),
            connection: Property::new(connection_state),
            scanning: Property::new(scanning),
            strength: Property::new(None),
            frequency: Property::new(None),
            networks: Property::new(Vec::new()),
        })
    }
}

/// Read a coherent `(state, scanning, connected_ssid)` snapshot from a freshly
/// created `Station` proxy. Returns `None` when the `Station` interface is
/// unavailable (device powered off, or IWD restarting) so callers can preserve
/// the last known state instead of flickering; reading every field off one proxy
/// back-to-back keeps the tuple coherent.
async fn read_station(
    connection: &Connection,
    path: &OwnedObjectPath,
) -> Option<(String, bool, Option<String>)> {
    let proxy = StationProxy::new(connection, path.clone()).await.ok()?;
    let state = proxy.state().await.ok()?;
    let scanning = proxy.scanning().await.unwrap_or(false);
    let connected_ssid = resolve_connected_ssid(connection, &proxy).await;
    Some((state, scanning, connected_ssid))
}

/// Resolve the SSID of the station's connected network, if any, reusing an
/// existing [`StationProxy`] so the read stays coherent with the caller's other
/// reads off the same proxy.
async fn resolve_connected_ssid(
    connection: &Connection,
    station_proxy: &StationProxy<'_>,
) -> Option<String> {
    let path = station_proxy.connected_network().await.ok()?;
    if !is_real_path(&path) {
        return None;
    }
    let network = NetworkProxy::new(connection, path).await.ok()?;
    network.name().await.ok()
}
