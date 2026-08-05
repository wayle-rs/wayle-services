//! IWD Station interface (`net.connman.iwd.Station`).

use zbus::{
    proxy,
    zvariant::{ObjectPath, OwnedObjectPath},
};

#[proxy(
    default_service = "net.connman.iwd",
    interface = "net.connman.iwd.Station"
)]
pub(crate) trait Station {
    /// Begin a scan for networks. Resolves once the scan request is accepted;
    /// completion is signalled via the `Scanning` property returning to false.
    fn scan(&self) -> zbus::Result<()>;

    /// Register a `SignalLevelAgent` to receive bucketed RSSI level changes for
    /// the connected network. `levels` are dBm thresholds in descending order.
    fn register_signal_level_agent(
        &self,
        path: &ObjectPath<'_>,
        levels: &[i16],
    ) -> zbus::Result<()>;

    /// Disconnect from the current network and disable auto-connect until the
    /// next explicit connect.
    fn disconnect(&self) -> zbus::Result<()>;

    /// Returns visible networks ordered by signal strength (strongest first),
    /// each as a tuple of `(network object path, signal strength in 100 x dBm)`.
    fn get_ordered_networks(&self) -> zbus::Result<Vec<(OwnedObjectPath, i16)>>;

    /// Current station state: `connected`, `disconnected`, `connecting`,
    /// `disconnecting`, or `roaming`.
    #[zbus(property)]
    fn state(&self) -> zbus::Result<String>;

    /// Whether a scan is currently in progress.
    #[zbus(property)]
    fn scanning(&self) -> zbus::Result<bool>;

    /// Object path of the connected network, when connected.
    #[zbus(property)]
    fn connected_network(&self) -> zbus::Result<OwnedObjectPath>;
}
