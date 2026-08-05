//! IWD Device interface (`net.connman.iwd.Device`).

use zbus::{proxy, zvariant::OwnedObjectPath};

#[proxy(
    default_service = "net.connman.iwd",
    interface = "net.connman.iwd.Device"
)]
pub(crate) trait Device {
    /// Whether the device is powered on (the WiFi enable toggle).
    #[zbus(property)]
    fn powered(&self) -> zbus::Result<bool>;

    /// Set the device's powered state.
    #[zbus(property)]
    fn set_powered(&self, value: bool) -> zbus::Result<()>;

    /// Object path of the [`Adapter`](super::adapter) this device belongs to.
    /// A device cannot be powered on while its adapter is powered off.
    #[zbus(property)]
    fn adapter(&self) -> zbus::Result<OwnedObjectPath>;
}
