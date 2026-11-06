//! The IWD service.

use std::sync::Arc;

use derive_more::Debug;
use tokio_util::sync::CancellationToken;
use wayle_core::Property;

use crate::{
    dispatcher::{
        self, Handle, Published,
        command::{Command, Commands},
    },
    error::Error,
    station::Station,
    types::PassphraseRequest,
};

/// WiFi management via IWD (`net.connman.iwd`).
///
/// State comes from IWD, published as reactive properties and kept in sync by
/// one background dispatcher, the only thing that changes it, so it reflects
/// changes made by any client (e.g. `iwctl`). Actions return nothing: they are
/// queued to the dispatcher, which sends them to IWD in call order, and their
/// outcome shows up as state.
#[derive(Debug)]
pub struct IwdService {
    #[debug(skip)]
    commands: Commands,
    #[debug(skip)]
    cancellation_token: CancellationToken,
    /// The WiFi station, while IWD has a device (live).
    pub station: Property<Option<Arc<Station>>>,
    /// The passphrase IWD is waiting for, while it waits: IWD asks this
    /// service's agent when it connects to a secured network it has no
    /// passphrase saved for.
    pub passphrase_request: Property<Option<PassphraseRequest>>,
}

impl IwdService {
    /// Connects to the system bus and, if IWD is running, enumerates its
    /// devices and networks and registers the passphrase agent. A background
    /// dispatcher then keeps the service in sync, including across IWD
    /// restarts; if IWD isn't running, the service starts empty and fills in
    /// once IWD appears.
    ///
    /// Must be called within a Tokio runtime, which runs the dispatcher.
    ///
    /// # Errors
    /// Returns error if the system bus connection fails.
    pub async fn new() -> Result<Self, Error> {
        let Handle {
            published,
            commands,
            cancellation_token,
        } = dispatcher::start().await?;
        let Published {
            station,
            passphrase_request,
        } = published;

        Ok(Self {
            commands,
            cancellation_token,
            station,
            passphrase_request,
        })
    }

    /// Answers the pending [`passphrase_request`](Self::passphrase_request).
    /// If IWD rejects the passphrase, it asks again (see
    /// [`PassphraseRequest::rejected`]). Ignored (and logged) if IWD isn't
    /// asking.
    pub fn provide_passphrase(&self, passphrase: String) {
        self.commands.send(Command::ProvidePassphrase(passphrase));
    }

    /// Turns the pending [`passphrase_request`](Self::passphrase_request)
    /// down, which ends the connection attempt (not recorded as a failure).
    pub fn cancel_passphrase_request(&self) {
        self.commands.send(Command::CancelPassphrase);
    }
}

impl Drop for IwdService {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
    }
}
