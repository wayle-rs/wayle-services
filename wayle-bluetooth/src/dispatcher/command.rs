//! Requests queued to the dispatcher by the public API.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::sync::{mpsc, oneshot};
use tracing::debug;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use super::request::Call;
use crate::{
    core::{adapter::Adapter, device::Device},
    error::Error,
    types::{adapter::AdapterAction, device::DeviceAction},
};

/// Everything the service, its devices and its adapters can ask of BlueZ.
///
/// Commands are processed one at a time by the dispatcher, in the order they
/// were queued, which is also the order their requests reach BlueZ. Requests
/// for a device or adapter apply to that instance only, and are dropped once
/// it has been replaced.
#[derive(Debug)]
pub(crate) enum Command {
    /// Send `call`, the request for `action` on `device`.
    Device {
        device: Arc<Device>,
        action: DeviceAction,
        call: Call,
    },
    /// Stop reporting `device`'s most recent failure.
    DismissDeviceError(Arc<Device>),
    /// Send `call`, the request for `action` on `adapter`.
    Adapter {
        adapter: Arc<Adapter>,
        action: AdapterAction,
        call: Call,
    },
    /// Stop reporting `adapter`'s most recent failure.
    DismissAdapterError(Arc<Adapter>),
    /// Start (`start`) or stop this service's discovery on `adapter`.
    Discover { adapter: Arc<Adapter>, start: bool },
    /// Turn Bluetooth on or off.
    SetEnabled(bool),
    /// Discover on the primary adapter; with a `timeout`, stop after it.
    StartDiscovery { timeout: Option<Duration> },
    /// Stop every discovery this service started.
    StopDiscovery,
    /// Answer the pending pairing request.
    Pairing(PairingResponse),
    /// Ask BlueZ something whose answer goes back to the caller.
    Query(Query),
}

#[derive(Debug)]
pub(crate) enum PairingResponse {
    Pin(String),
    Passkey(u32),
    Confirmation(bool),
    Authorization(bool),
    ServiceAuthorization(bool),
    Cancel,
}

/// A question to BlueZ whose answer is returned rather than published.
#[derive(Debug)]
pub(crate) enum Query {
    ServiceRecords {
        device: OwnedObjectPath,
        reply: oneshot::Sender<Result<Vec<Vec<u8>>, Error>>,
    },
    DiscoveryFilters {
        adapter: OwnedObjectPath,
        reply: oneshot::Sender<Result<Vec<String>, Error>>,
    },
    ConnectDevice {
        adapter: OwnedObjectPath,
        properties: HashMap<String, OwnedValue>,
        reply: oneshot::Sender<Result<OwnedObjectPath, Error>>,
    },
}

/// The dispatcher's command queue, held by the service and every model.
///
/// It doesn't keep the D-Bus connection open: only the dispatcher (and the
/// tasks it runs, which end with it) holds that. So once the service is
/// dropped the connection closes, and BlueZ drops this client's discovery
/// sessions and agent, even while devices and adapters are still held. A query
/// still awaiting BlueZ's answer then fails with
/// [`Error::ServiceStopped`].
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
            debug!(command = ?err.0, "bluetooth service is gone; dropping command");
        }
    }

    /// Queues the query `make` builds around a reply channel, and waits for
    /// its answer.
    ///
    /// # Errors
    ///
    /// The query's own error, or [`Error::ServiceStopped`] if the service is
    /// gone.
    pub(crate) async fn query<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T, Error>>) -> Query,
    ) -> Result<T, Error> {
        let (reply, answer) = oneshot::channel();
        self.0
            .send(Command::Query(make(reply)))
            .map_err(|_| Error::ServiceStopped)?;

        answer.await.map_err(|_| Error::ServiceStopped)?
    }
}
