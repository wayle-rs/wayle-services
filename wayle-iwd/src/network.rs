//! A network visible to a station.

use std::sync::Arc;

use derive_more::Debug;
use wayle_core::Property;
use zbus::zvariant::OwnedObjectPath;

use crate::{
    dispatcher::command::{Command, Commands},
    types::SecurityType,
};

/// A network visible to a station (an IWD `net.connman.iwd.Network`).
///
/// Each network is a single live instance, shared as an `Arc`, and networks
/// compare equal only if they are the same instance: the `Arc` is the
/// network's identity. Unlike an SSID (shared by networks of different
/// security) or an object path (reused when a network comes back, or IWD
/// restarts), an instance is never reused, so it can't name another network. Its state follows IWD's
/// signals; actions return nothing, and their outcome shows up as state.
#[derive(Debug)]
pub struct Network {
    #[debug(skip)]
    commands: Commands,
    object_path: OwnedObjectPath,
    /// Network name (SSID).
    pub ssid: Property<String>,
    /// Signal of the network's strongest access point in IWD's latest scan,
    /// in dBm (`0` is the strongest, `-100` very weak). Bucket it with
    /// [`SignalStrength::from_dbm`](crate::SignalStrength::from_dbm).
    pub signal: Property<i16>,
    /// Security classification derived from `Network.Type`.
    pub security: Property<SecurityType>,
    /// Whether IWD has this network's credentials saved (a `KnownNetwork`).
    pub known: Property<bool>,
}

impl PartialEq for Network {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for Network {}

impl Network {
    pub(crate) fn new(commands: &Commands, object_path: OwnedObjectPath) -> Self {
        Self {
            commands: commands.clone(),
            object_path,
            ssid: Property::new(String::new()),
            signal: Property::new(-100),
            security: Property::new(SecurityType::None),
            known: Property::new(false),
        }
    }

    /// D-Bus object path of this network.
    pub fn object_path(&self) -> &OwnedObjectPath {
        &self.object_path
    }

    /// Connects to this network.
    ///
    /// For a secured network without saved credentials, IWD asks for the
    /// passphrase, published as the service's
    /// [`passphrase_request`](crate::IwdService::passphrase_request). If IWD
    /// rejects the passphrase given, the service connects again, so IWD asks
    /// again (with [`rejected`](crate::PassphraseRequest::rejected) set).
    ///
    /// Progress shows in the station's
    /// [`connection`](crate::Station::connection); a failure is recorded in its
    /// [`last_error`](crate::Station::last_error).
    pub fn connect(self: &Arc<Self>) {
        self.commands.send(Command::Connect(Arc::clone(self)));
    }

    /// Forgets this network's saved credentials, if IWD has any.
    ///
    /// [`known`](Self::known) follows once IWD reports it; a failure is
    /// recorded in the station's [`last_error`](crate::Station::last_error).
    pub fn forget(self: &Arc<Self>) {
        self.commands.send(Command::Forget(Arc::clone(self)));
    }
}
