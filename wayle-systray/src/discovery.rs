use tracing::{debug, instrument};
use zbus::Connection;

use crate::{
    error::Error, proxy::status_notifier_watcher::StatusNotifierWatcherProxy,
    registrar::RegistrarHandle,
};

/// Host-mode helpers for interacting with an existing external watcher.
pub(crate) struct SystemTrayServiceDiscovery;

impl SystemTrayServiceDiscovery {
    /// Registers this service as a `StatusNotifierHost` with the active watcher.
    #[instrument(skip(connection), fields(host_name = %host_name), err)]
    pub async fn register_as_host(connection: &Connection, host_name: &str) -> Result<(), Error> {
        let watcher = StatusNotifierWatcherProxy::new(connection).await?;
        watcher.register_status_notifier_host(host_name).await?;

        debug!("Registered as StatusNotifierHost");
        Ok(())
    }

    /// Seeds the registrar with the items the external watcher already knows about, so a
    /// host that starts after items are present still shows them.
    #[instrument(skip(connection, registrar), err)]
    pub async fn seed_from_watcher(
        connection: &Connection,
        registrar: &RegistrarHandle,
    ) -> Result<(), Error> {
        let watcher = StatusNotifierWatcherProxy::new(connection).await?;
        let services = watcher.registered_status_notifier_items().await?;

        debug!(count = services.len(), "seeding host from existing watcher items");
        for service in services {
            registrar.register(service, None);
        }

        Ok(())
    }
}
