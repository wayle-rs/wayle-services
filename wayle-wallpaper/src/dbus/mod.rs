//! D-Bus interface for the wallpaper service. Handles registration,
//! signal emission, and exposes the client proxy for external consumers.

mod client;
mod server;

use std::sync::Arc;

pub use client::WallpaperProxy;
pub(crate) use server::WallpaperDaemon;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};
use zbus::Connection;

use crate::{error::Error, service::WallpaperService};

/// D-Bus service name.
pub const SERVICE_NAME: &str = "com.wayle.Wallpaper1";

/// D-Bus object path.
pub const SERVICE_PATH: &str = "/com/wayle/Wallpaper";

/// Registers the wallpaper D-Bus interface and starts forwarding
/// internal events as D-Bus signals.
pub(crate) async fn register(
    connection: &Connection,
    service: &Arc<WallpaperService>,
) -> Result<(), Error> {
    let daemon = WallpaperDaemon {
        service: Arc::clone(service),
    };

    connection
        .object_server()
        .at(SERVICE_PATH, daemon)
        .await
        .map_err(|err| {
            Error::ServiceInitializationFailed(format!(
                "cannot register D-Bus object at '{SERVICE_PATH}': {err}"
            ))
        })?;

    connection.request_name(SERVICE_NAME).await.map_err(|err| {
        Error::ServiceInitializationFailed(format!(
            "cannot acquire D-Bus name '{SERVICE_NAME}': {err}"
        ))
    })?;

    spawn_signal_bridge(connection, service);

    Ok(())
}

/// Forwards `extraction_complete` events to the `ColorsExtracted` D-Bus
/// signal so other processes know when to re-read the palette cache.
fn spawn_signal_bridge(connection: &Connection, service: &Arc<WallpaperService>) {
    let mut extraction_rx = service.extraction_complete.subscribe();
    let conn = connection.clone();

    tokio::spawn(async move {
        let Ok(emitter) = zbus::object_server::SignalEmitter::new(&conn, SERVICE_PATH) else {
            warn!("cannot create signal emitter, skipping signal bridge");
            return;
        };

        loop {
            match extraction_rx.recv().await {
                Ok(()) => {
                    if let Err(err) = WallpaperDaemon::colors_extracted(&emitter).await {
                        debug!(error = %err, "cannot emit colors_extracted signal");
                    }
                }

                Err(RecvError::Lagged(skipped)) => {
                    debug!(skipped, "signal bridge lagged, catching up");
                    if let Err(err) = WallpaperDaemon::colors_extracted(&emitter).await {
                        debug!(error = %err, "cannot emit colors_extracted signal");
                    }
                }

                Err(RecvError::Closed) => break,
            }
        }
    });
}
