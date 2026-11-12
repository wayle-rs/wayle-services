#![allow(missing_docs)]
pub(crate) mod discovery;

use tracing::{info, instrument};
use zbus::{fdo, message::Header, names::OwnedUniqueName, object_server::SignalEmitter};

use crate::{registrar::RegistrarHandle, types::PROTOCOL_VERSION};

/// The `org.kde.StatusNotifierWatcher` D-Bus object.
///
/// It is a thin front end: `RegisterStatusNotifierItem` forwards to the [`RegistrarHandle`]
/// (the single owner of tray state) and the `RegisteredStatusNotifierItems` property is a
/// query against it. The `StatusNotifierItem(Un)Registered` signals are emitted by the
/// registrar itself, since it is what decides when an item truly enters or leaves the set.
#[derive(Clone)]
pub(crate) struct StatusNotifierWatcher {
    registrar: RegistrarHandle,
}

impl StatusNotifierWatcher {
    pub(crate) fn new(registrar: RegistrarHandle) -> Self {
        Self { registrar }
    }
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl StatusNotifierWatcher {
    #[instrument(skip(self, header), fields(service = %service))]
    async fn register_status_notifier_item(
        &self,
        #[zbus(header)] header: Header<'_>,
        service: String,
    ) -> fdo::Result<()> {
        // The sender is the item's live owning connection — a provably-alive unique name,
        // so no owner resolution is needed and the register-after-death race is avoided.
        let sender = header
            .sender()
            .and_then(|name| OwnedUniqueName::try_from(name.as_str()).ok());

        info!(service = %service, sender = ?sender, "registering StatusNotifierItem");
        self.registrar.register(service, sender);
        Ok(())
    }

    #[instrument(skip(self, emitter))]
    async fn register_status_notifier_host(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        _service: String,
    ) -> fdo::Result<()> {
        // We report as always host-registered (see `is_status_notifier_host_registered`):
        // the service consuming this watcher's items is itself a host, so one always
        // exists. Emit the signal so any item gating on host availability proceeds.
        Self::status_notifier_host_registered(&emitter).await?;
        Ok(())
    }

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        self.registrar.registered_services().await
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        PROTOCOL_VERSION
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        ctx: &SignalEmitter<'_>,
        service: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        ctx: &SignalEmitter<'_>,
        service: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(ctx: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_unregistered(ctx: &SignalEmitter<'_>) -> zbus::Result<()>;
}
