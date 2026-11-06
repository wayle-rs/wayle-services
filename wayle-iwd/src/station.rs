//! A WiFi station: an IWD device and its `Station` interface.

use std::sync::Arc;

use derive_more::Debug;
use wayle_core::Property;
use zbus::zvariant::OwnedObjectPath;

use crate::{
    dispatcher::command::{Command, Commands},
    network::Network,
    types::{ConnectionState, SignalStrength, StationError},
};

/// A WiFi station: connection state, scan results, and controls.
///
/// A single live instance per IWD device, which outlives powering the device
/// off and on. Its state follows IWD's signals, including changes made by any
/// other IWD client (e.g. `iwctl`). Actions return nothing: they are queued to
/// the service, which sends them to IWD in call order, and their outcome shows
/// up as state; a failure is recorded in [`last_error`](Self::last_error).
#[derive(Debug)]
pub struct Station {
    #[debug(skip)]
    commands: Commands,
    object_path: OwnedObjectPath,
    /// Whether WiFi is on: the device's adapter is powered (its radio isn't
    /// blocked by rfkill) and the device is up (`Device.Powered`).
    pub powered: Property<bool>,
    /// What the station is doing, and with which network.
    pub connection: Property<ConnectionState>,
    /// Whether a scan is in progress.
    pub scanning: Property<bool>,
    /// Signal strength of the connected link: pushed by IWD as it crosses
    /// thresholds, after a diagnostics reading when the link comes up.
    pub strength: Property<Option<SignalStrength>>,
    /// Frequency of the connected link in MHz, from IWD's diagnostics.
    pub frequency: Property<Option<u32>>,
    /// Visible networks, in IWD's order: the connected network first, then
    /// known networks (ranked mostly by how recently they were used), then
    /// the others (by IWD's rank of their access points). Watchers are
    /// notified when the list changes, and also when a listed network's
    /// [`signal`](Network::signal), [`ssid`](Network::ssid),
    /// [`security`](Network::security) or [`known`](Network::known) changes.
    pub networks: Property<Vec<Arc<Network>>>,
    /// The most recent failed request, if any. Cleared by the next request,
    /// when a connection starts, or by [`dismiss_error`](Self::dismiss_error).
    pub last_error: Property<Option<StationError>>,
}

impl PartialEq for Station {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for Station {}

impl Station {
    pub(crate) fn new(commands: &Commands, object_path: OwnedObjectPath) -> Self {
        Self {
            commands: commands.clone(),
            object_path,
            powered: Property::new(false),
            connection: Property::new(ConnectionState::Idle),
            scanning: Property::new(false),
            strength: Property::new(None),
            frequency: Property::new(None),
            networks: Property::new(Vec::new()),
            last_error: Property::new(None),
        }
    }

    /// D-Bus object path of the device.
    pub fn object_path(&self) -> &OwnedObjectPath {
        &self.object_path
    }

    /// Turns WiFi on or off, as desktops do: off blocks every adapter's radio
    /// (`Adapter.Powered`, which IWD implements as the rfkill soft block, so
    /// the block holds until lifted); on lifts the blocks, and has the device
    /// come up (`Device.Powered`) even if it was taken down on its own.
    ///
    /// The outcome shows in [`powered`](Self::powered); a failure is recorded
    /// in [`last_error`](Self::last_error), including a hardware block (see
    /// [`StationError::hardware_blocked`](crate::StationError::hardware_blocked)).
    pub fn set_powered(self: &Arc<Self>, powered: bool) {
        self.commands
            .send(Command::SetPowered(Arc::clone(self), powered));
    }

    /// Requests a scan. [`scanning`](Self::scanning) and
    /// [`networks`](Self::networks) follow. A scan IWD turns down because it is
    /// busy (e.g. already scanning) isn't recorded as a failure.
    pub fn scan(self: &Arc<Self>) {
        self.commands.send(Command::Scan(Arc::clone(self)));
    }

    /// Disconnects from the current network, or cancels a connection under
    /// way. IWD then stops connecting automatically until the next connect.
    pub fn disconnect(self: &Arc<Self>) {
        self.commands.send(Command::Disconnect(Arc::clone(self)));
    }

    /// Stops reporting the most recent failure.
    pub fn dismiss_error(self: &Arc<Self>) {
        self.commands.send(Command::DismissError(Arc::clone(self)));
    }
}
