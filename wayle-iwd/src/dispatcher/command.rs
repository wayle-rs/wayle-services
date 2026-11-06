//! Requests queued to the dispatcher by the public API.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::debug;

use crate::{network::Network, station::Station};

/// Everything the service, its station and its networks can ask of IWD.
///
/// Commands are processed one at a time by the dispatcher, in the order they
/// were queued, which is also the order their requests reach IWD. A command
/// for a station or network applies to that instance only, and is dropped once
/// it has been replaced.
#[derive(Debug)]
pub(crate) enum Command {
    Connect(Arc<Network>),
    Forget(Arc<Network>),
    Disconnect(Arc<Station>),
    Scan(Arc<Station>),
    SetPowered(Arc<Station>, bool),
    DismissError(Arc<Station>),
    /// Answer IWD's pending passphrase request.
    ProvidePassphrase(String),
    /// Turn IWD's pending passphrase request down.
    CancelPassphrase,
}

/// The dispatcher's command queue, held by the service and every model. It
/// doesn't keep the D-Bus connection open: only the dispatcher holds that.
#[derive(Debug, Clone)]
pub(crate) struct Commands(mpsc::UnboundedSender<Command>);

impl Commands {
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<Command>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self(sender), receiver)
    }

    /// Queues `command`. Only fails once the service is gone, when there is
    /// nothing left to act on.
    pub(crate) fn send(&self, command: Command) {
        if let Err(err) = self.0.send(command) {
            debug!(command = ?err.0, "iwd service is gone; dropping command");
        }
    }
}
