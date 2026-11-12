//! Host-mode producers.
//!
//! In watcher mode the registrar is fed by the watcher interface and startup discovery.
//! In host mode we instead consume an *external* watcher's registration signals and
//! forward them to the registrar. This is the only place that knows about the external
//! watcher; the registrar stays mode-agnostic.

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::info;
use zbus::Connection;

use crate::{
    error::Error, proxy::status_notifier_watcher::StatusNotifierWatcherProxy,
    registrar::RegistrarHandle,
};

/// Subscribes to the external watcher's item registration signals and forwards them.
///
/// Registered items carry a raw service string whose owner the registrar resolves.
/// Unregistered items are correlated by that exact string, because an external watcher
/// emits the string it stored *after* the owner has died (so the owner is no longer
/// resolvable). Each item additionally arms its own owner-death watch inside the
/// registrar, so removal still happens even if the external `Unregistered` is missed.
pub(crate) async fn spawn_host_listeners(
    connection: &Connection,
    registrar: RegistrarHandle,
    cancellation_token: CancellationToken,
) -> Result<(), Error> {
    let watcher = StatusNotifierWatcherProxy::new(connection).await?;
    let mut registered = watcher.receive_status_notifier_item_registered().await?;
    let mut unregistered = watcher.receive_status_notifier_item_unregistered().await?;

    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancellation_token.cancelled() => {
                    info!("Systray host listeners cancelled");
                    return;
                }
                Some(signal) = registered.next() => {
                    if let Ok(args) = signal.args() {
                        registrar.register(args.service.to_string(), None);
                    }
                }
                Some(signal) = unregistered.next() => {
                    if let Ok(args) = signal.args() {
                        registrar.remove_by_service(args.service.to_string());
                    }
                }
            }
        }
    });

    Ok(())
}
