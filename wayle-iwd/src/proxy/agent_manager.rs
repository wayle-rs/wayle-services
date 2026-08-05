//! IWD AgentManager interface (`net.connman.iwd.AgentManager`).

use zbus::{proxy, zvariant::ObjectPath};

#[proxy(
    default_service = "net.connman.iwd",
    interface = "net.connman.iwd.AgentManager",
    default_path = "/net/connman/iwd"
)]
pub(crate) trait AgentManager {
    /// Register a passphrase agent served at the given object path.
    fn register_agent(&self, path: &ObjectPath<'_>) -> zbus::Result<()>;
}
