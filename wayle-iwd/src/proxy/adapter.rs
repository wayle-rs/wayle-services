//! IWD Adapter interface (`net.connman.iwd.Adapter`).

use zbus::proxy;

#[proxy(
    default_service = "net.connman.iwd",
    interface = "net.connman.iwd.Adapter"
)]
pub(crate) trait Adapter {
    /// Whether the adapter is powered on. While `false`, none of the adapter's
    /// devices can be powered on.
    #[zbus(property)]
    fn powered(&self) -> zbus::Result<bool>;

    /// Set the adapter's powered state.
    #[zbus(property)]
    fn set_powered(&self, value: bool) -> zbus::Result<()>;
}
