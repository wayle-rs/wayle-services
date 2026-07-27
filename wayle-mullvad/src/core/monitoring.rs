//! Background monitoring: applies backend events to the model's properties.

use std::{
    sync::{Arc, Weak},
    time::Duration,
};

use futures::{StreamExt, stream::BoxStream};
use tokio_util::sync::CancellationToken;
use wayle_traits::ModelMonitoring;

use crate::{
    backend::{BackendEvent, MullvadBackend},
    core::Mullvad,
    error::Error,
};

/// Delay before re-subscribing after the event stream ends or errors.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

impl ModelMonitoring for Mullvad {
    type Error = Error;

    async fn start_monitoring(self: Arc<Self>) -> Result<(), Self::Error> {
        let Some(token) = self.cancellation_token.clone() else {
            return Err(Error::MissingCancellationToken);
        };
        let backend = Arc::clone(&self.backend);
        let weak = Arc::downgrade(&self);

        tokio::spawn(run_event_loop(weak, backend, token));
        Ok(())
    }
}

/// Result of consuming a single event stream to completion.
enum StreamOutcome {
    /// The token was cancelled (or the model was dropped) — stop entirely.
    Stop,
    /// The stream ended on its own — reconnect.
    Ended,
}

/// Subscribes to backend events and applies them until cancelled or the model
/// is dropped, re-subscribing whenever the stream ends.
async fn run_event_loop(
    weak: Weak<Mullvad>,
    backend: Arc<dyn MullvadBackend>,
    token: CancellationToken,
) {
    loop {
        // Subscribe under the token so a stalled handshake doesn't pin the task
        // and block shutdown.
        let Some(subscribed) = token.run_until_cancelled(backend.events()).await else {
            return;
        };

        match subscribed {
            Ok(mut stream) => {
                // Re-sync a fresh snapshot right after (re)subscribing: the
                // daemon's event stream only pushes future events, so on a
                // reconnect any state that changed while the stream was down
                // would otherwise stay stale until the next event. Subscribing
                // first means events fired during the fetch are buffered in the
                // stream and applied by `consume_stream` afterwards. The resync
                // RPCs are also raced against the token.
                if token
                    .run_until_cancelled(resync(&weak, &backend))
                    .await
                    .is_none()
                {
                    return;
                }

                match consume_stream(&weak, &mut stream, &token).await {
                    StreamOutcome::Stop => return,
                    StreamOutcome::Ended => {}
                }
            }
            Err(error) => {
                tracing::warn!(%error, "failed to subscribe to Mullvad events; retrying");
            }
        }

        if wait_or_cancel(&token).await {
            return;
        }
    }
}

/// Fetches a fresh snapshot of all reactive state and applies it. Best-effort:
/// individual fetch failures are ignored since the event stream will still
/// deliver subsequent updates.
async fn resync(weak: &Weak<Mullvad>, backend: &Arc<dyn MullvadBackend>) {
    let Some(model) = weak.upgrade() else {
        return;
    };

    if let Ok(status) = backend.tunnel_status().await {
        model.connection_state.set(status.state);
        model.connected_network.set(status.network);
    }
    if let Ok(logged_in) = backend.logged_in().await {
        model.logged_in.set(logged_in);
    }
    if let Ok(networks) = backend.networks().await {
        model.networks.set(networks);
    }
}

/// Applies events from `stream` until it ends or the token is cancelled.
async fn consume_stream(
    weak: &Weak<Mullvad>,
    stream: &mut BoxStream<'static, BackendEvent>,
    token: &CancellationToken,
) -> StreamOutcome {
    loop {
        tokio::select! {
            () = token.cancelled() => return StreamOutcome::Stop,
            event = stream.next() => match event {
                Some(event) => {
                    let Some(model) = weak.upgrade() else {
                        return StreamOutcome::Stop;
                    };
                    apply_event(&model, event);
                }
                None => return StreamOutcome::Ended,
            },
        }
    }
}

/// Sleeps for [`RECONNECT_DELAY`], returning `true` if cancelled during the wait.
async fn wait_or_cancel(token: &CancellationToken) -> bool {
    tokio::select! {
        () = token.cancelled() => true,
        () = tokio::time::sleep(RECONNECT_DELAY) => false,
    }
}

/// Applies a single backend event to the model's reactive properties.
fn apply_event(model: &Mullvad, event: BackendEvent) {
    match event {
        BackendEvent::Tunnel(status) => {
            model.connection_state.set(status.state);
            model.connected_network.set(status.network);
        }
        BackendEvent::LoggedIn(logged_in) => model.logged_in.set(logged_in),
        BackendEvent::Networks(networks) => model.networks.set(networks),
    }
}
