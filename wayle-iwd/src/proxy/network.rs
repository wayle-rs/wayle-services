//! IWD Network interface (`net.connman.iwd.Network`).

use zbus::{proxy, zvariant::OwnedObjectPath};

#[proxy(
    default_service = "net.connman.iwd",
    interface = "net.connman.iwd.Network"
)]
pub(crate) trait Network {
    /// Connect to this network. For secured networks without saved
    /// credentials, IWD requests the passphrase from the registered agent.
    fn connect(&self) -> zbus::Result<()>;

    /// Network name (SSID).
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;

    /// Security type: `open`, `wep`, `psk`, or `8021x`.
    #[zbus(property, name = "Type")]
    fn network_type(&self) -> zbus::Result<String>;

    /// Object path of the matching known network, if this network is saved.
    #[zbus(property)]
    fn known_network(&self) -> zbus::Result<OwnedObjectPath>;
}
