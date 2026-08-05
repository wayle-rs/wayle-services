//! IWD passphrase agent.
//!
//! Unlike NetworkManager (where secrets are passed to the activate call), IWD
//! requests secrets from a registered agent object. We serve a
//! `net.connman.iwd.Agent` and answer `RequestPassphrase` from a small store of
//! passphrases staged by [`Station::connect`](crate::station::Station::connect)
//! immediately before it calls `Network.Connect()`.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tracing::{debug, warn};
use zbus::{interface, zvariant::OwnedObjectPath};

/// Object path at which our agent is served.
pub(crate) const AGENT_PATH: &str = "/wayle/iwd/agent";

/// Thread-safe store of passphrases pending delivery to IWD, keyed by the
/// network object path being connected.
#[derive(Default)]
pub(crate) struct PassphraseStore {
    pending: Mutex<HashMap<OwnedObjectPath, String>>,
}

impl PassphraseStore {
    /// Stage a passphrase for an upcoming connection to `network`.
    pub(crate) fn insert(&self, network: OwnedObjectPath, passphrase: String) {
        if let Ok(mut guard) = self.pending.lock() {
            guard.insert(network, passphrase);
        }
    }

    /// Remove and return the staged passphrase for `network`, if any.
    pub(crate) fn take(&self, network: &OwnedObjectPath) -> Option<String> {
        self.pending.lock().ok()?.remove(network)
    }

    /// Discard any staged passphrase for `network` (e.g. after a failed connect).
    pub(crate) fn discard(&self, network: &OwnedObjectPath) {
        if let Ok(mut guard) = self.pending.lock() {
            guard.remove(network);
        }
    }
}

/// D-Bus error type for the agent, mapped to `net.connman.iwd.Agent.Error.*`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "net.connman.iwd.Agent.Error")]
pub(crate) enum AgentError {
    /// The request was cancelled / cannot be satisfied.
    Canceled(String),
}

/// Served implementation of `net.connman.iwd.Agent`.
pub(crate) struct Agent {
    store: Arc<PassphraseStore>,
}

impl Agent {
    pub(crate) fn new(store: Arc<PassphraseStore>) -> Self {
        Self { store }
    }
}

#[interface(name = "net.connman.iwd.Agent")]
impl Agent {
    /// Called by IWD when the agent is unregistered.
    fn release(&self) {
        debug!("iwd agent released");
    }

    /// Provide the passphrase for a (PSK/WEP) network connection.
    fn request_passphrase(&self, network: OwnedObjectPath) -> Result<String, AgentError> {
        self.store.take(&network).ok_or_else(|| {
            warn!(%network, "iwd requested a passphrase but none was staged");
            AgentError::Canceled(String::from("no passphrase available"))
        })
    }

    /// Private-key passphrases (8021x) are not supported in this version.
    fn request_private_key_passphrase(
        &self,
        _network: OwnedObjectPath,
    ) -> Result<String, AgentError> {
        Err(AgentError::Canceled(String::from("unsupported")))
    }

    /// Username/password auth (8021x) is not supported in this version.
    fn request_user_name_and_password(
        &self,
        _network: OwnedObjectPath,
    ) -> Result<(String, String), AgentError> {
        Err(AgentError::Canceled(String::from("unsupported")))
    }

    /// User-password auth (8021x) is not supported in this version.
    fn request_user_password(
        &self,
        _network: OwnedObjectPath,
        _user: String,
    ) -> Result<String, AgentError> {
        Err(AgentError::Canceled(String::from("unsupported")))
    }

    /// Called by IWD when an in-flight request is aborted.
    fn cancel(&self, reason: String) {
        debug!(%reason, "iwd agent request cancelled");
    }
}
