//! The reactive Mullvad model: observable state plus connect/disconnect controls.

mod monitoring;

use std::sync::Arc;

use derive_more::Debug;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use wayle_core::Property;
use wayle_traits::ModelMonitoring;

use crate::{
    backend::MullvadBackend,
    error::Error,
    types::{ConnectedNetwork, ConnectionState, NetworkCountry, NetworkTarget},
};

/// A control request queued for serialized execution against the daemon.
#[derive(Debug)]
enum ControlAction {
    Connect(NetworkTarget),
    Reconnect,
    Disconnect,
}

/// Reactive Mullvad VPN state and controls.
///
/// State is exposed as [`Property`] fields — read a snapshot with
/// [`Property::get`], or observe changes with [`Property::watch`]. The tunnel is
/// driven with [`connect`](Self::connect) / [`reconnect`](Self::reconnect) /
/// [`disconnect`](Self::disconnect), which are non-blocking: they queue a
/// request that a single worker runs to completion in order, so daemon calls
/// from different callers never interleave. Observe the outcome via the
/// reactive state.
#[derive(Debug, Clone)]
pub struct Mullvad {
    #[debug(skip)]
    backend: Arc<dyn MullvadBackend>,
    #[debug(skip)]
    cancellation_token: Option<CancellationToken>,
    #[debug(skip)]
    action_tx: UnboundedSender<ControlAction>,

    /// Whether an account is currently logged in.
    pub logged_in: Property<bool>,
    /// The current tunnel connection state.
    pub connection_state: Property<ConnectionState>,
    /// The relay currently in use, when connecting or connected.
    pub connected_network: Property<Option<ConnectedNetwork>>,
    /// The available networks, as a country → city → network tree.
    pub networks: Property<Vec<NetworkCountry>>,
}

impl Mullvad {
    /// Builds a live model from `backend`, fetching an initial snapshot and
    /// starting background monitoring + the control-action worker under
    /// `cancellation_token`.
    ///
    /// # Errors
    /// Returns an error if any initial state fetch fails, or if monitoring
    /// cannot start.
    pub(crate) async fn connect_live(
        backend: Arc<dyn MullvadBackend>,
        cancellation_token: CancellationToken,
    ) -> Result<Arc<Self>, Error> {
        let status = backend.tunnel_status().await?;
        let logged_in = backend.logged_in().await?;
        let networks = backend.networks().await?;

        // Serialize all control actions through a single worker so daemon calls
        // from different UI handlers can never interleave.
        let (action_tx, action_rx) = mpsc::unbounded_channel();
        spawn_action_worker(Arc::clone(&backend), action_rx, cancellation_token.clone());

        let model = Arc::new(Self {
            backend,
            cancellation_token: Some(cancellation_token),
            action_tx,
            logged_in: Property::new(logged_in),
            connection_state: Property::new(status.state),
            connected_network: Property::new(status.network),
            networks: Property::new(networks),
        });

        Arc::clone(&model).start_monitoring().await?;
        Ok(model)
    }

    /// Queues a connect to `target`, changing only the exit location and
    /// preserving the daemon's other relay constraints. Non-blocking.
    pub fn connect(&self, target: &NetworkTarget) {
        let _ = self.action_tx.send(ControlAction::Connect(target.clone()));
    }

    /// Queues a reconnect using the daemon's current relay settings, without
    /// changing the selected relay. Non-blocking.
    pub fn reconnect(&self) {
        let _ = self.action_tx.send(ControlAction::Reconnect);
    }

    /// Queues a disconnect. Non-blocking.
    pub fn disconnect(&self) {
        let _ = self.action_tx.send(ControlAction::Disconnect);
    }
}

/// Drains queued [`ControlAction`]s, running each against the backend to
/// completion before the next so two callers' daemon calls never interleave.
/// Stops when the token is cancelled or the queue is closed (model dropped).
fn spawn_action_worker(
    backend: Arc<dyn MullvadBackend>,
    mut rx: UnboundedReceiver<ControlAction>,
    token: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let action = tokio::select! {
                () = token.cancelled() => return,
                action = rx.recv() => match action {
                    Some(action) => action,
                    None => return,
                },
            };

            let result = match action {
                ControlAction::Connect(target) => backend.connect(&target).await,
                ControlAction::Reconnect => backend.reconnect().await,
                ControlAction::Disconnect => backend.disconnect().await,
            };
            if let Err(error) = result {
                tracing::warn!(%error, "mullvad control action failed");
            }
        }
    });
}
