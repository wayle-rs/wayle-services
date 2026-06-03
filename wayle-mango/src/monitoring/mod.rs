//! Wires the `watch` streams into the reactive [`Property`] fields.
//!
//! Two streams run in parallel: `all-monitors` (tags, focused client, keyboard
//! layout, key mode) and `all-clients` (windows). Each spawned [`watch_loop`]
//! reads one frame per line and hands it to [`refresh`], which replaces the
//! [`Property`] fields. Mango sends full snapshots, so there is no diff state to
//! maintain.

mod refresh;

use derive_more::Debug;
use tokio_util::sync::CancellationToken;
use tracing::{instrument, warn};
use wayle_core::Property;
use wayle_traits::ServiceMonitoring;

use crate::{
    constants::{WATCH_ALL_CLIENTS, WATCH_ALL_MONITORS},
    core::{Client, Monitor},
    error::Error,
    ipc::{WatchStream, connect_watch_stream},
    service::MangoService,
    types::FocusedClient,
};

/// Clones of the service's [`Property`] fields, refreshed by the watch loops on
/// each frame.
#[derive(Debug, Clone)]
pub(crate) struct MonitoringHandles {
    pub(crate) monitors: Property<Vec<Monitor>>,
    pub(crate) clients: Property<Vec<Client>>,
    pub(crate) focused_client: Property<Option<FocusedClient>>,
    pub(crate) keyboard_layout: Property<Option<String>>,
    pub(crate) keymode: Property<Option<String>>,
}

impl ServiceMonitoring for MangoService {
    type Error = Error;

    #[instrument(skip(self), err)]
    async fn start_monitoring(&self) -> Result<(), Error> {
        let monitor_stream = connect_watch_stream(WATCH_ALL_MONITORS).await?;
        let client_stream = connect_watch_stream(WATCH_ALL_CLIENTS).await?;

        let handles = MonitoringHandles {
            monitors: self.monitors.clone(),
            clients: self.clients.clone(),
            focused_client: self.focused_client.clone(),
            keyboard_layout: self.keyboard_layout.clone(),
            keymode: self.keymode.clone(),
        };

        tokio::spawn(monitor_loop(
            monitor_stream,
            handles.clone(),
            self.cancellation_token.clone(),
        ));
        tokio::spawn(client_loop(
            client_stream,
            handles,
            self.cancellation_token.clone(),
        ));

        Ok(())
    }
}

async fn monitor_loop(
    stream: WatchStream,
    handles: MonitoringHandles,
    cancellation_token: CancellationToken,
) {
    watch_loop(stream, cancellation_token, "monitor", |line| {
        refresh::apply_monitor_frame(&handles, line);
    })
    .await;
}

async fn client_loop(
    stream: WatchStream,
    handles: MonitoringHandles,
    cancellation_token: CancellationToken,
) {
    watch_loop(stream, cancellation_token, "client", |line| {
        refresh::apply_client_frame(&handles, line);
    })
    .await;
}

async fn watch_loop(
    mut stream: WatchStream,
    cancellation_token: CancellationToken,
    kind: &'static str,
    mut on_frame: impl FnMut(&str),
) {
    loop {
        tokio::select! {
            _ = cancellation_token.cancelled() => return,
            next_line = stream.next_line() => match next_line {
                Ok(Some(line)) => on_frame(&line),
                Ok(None) => {
                    warn!(kind, "mango watch stream closed");
                    return;
                }
                Err(err) => {
                    warn!(error = %err, kind, "mango watch stream read error");
                    return;
                }
            }
        }
    }
}
