//! IWD KnownNetwork interface (`net.connman.iwd.KnownNetwork`).

use zbus::proxy;

#[proxy(
    default_service = "net.connman.iwd",
    interface = "net.connman.iwd.KnownNetwork"
)]
pub(crate) trait KnownNetwork {
    /// Forget this known network, removing its saved credentials.
    fn forget(&self) -> zbus::Result<()>;
}
