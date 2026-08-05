//! IWD signal-level agent.
//!
//! Instead of polling RSSI, IWD pushes the connected link's bucketed signal
//! level via a registered `net.connman.iwd.SignalLevelAgent`. We register one
//! per station with [`SIGNAL_STRENGTH_THRESHOLDS`](crate::types::SIGNAL_STRENGTH_THRESHOLDS)
//! so each `Changed` maps straight to a [`SignalStrength`] bucket.

use tracing::debug;
use wayle_core::Property;
use zbus::{interface, zvariant::OwnedObjectPath};

use crate::types::SignalStrength;

/// Object path at which our signal-level agent is served.
pub(crate) const SIGNAL_LEVEL_AGENT_PATH: &str = "/wayle/iwd/signal_level_agent";

/// Served implementation of `net.connman.iwd.SignalLevelAgent`. Publishes the
/// connected link's bucketed strength to the station's reactive `strength`.
pub(crate) struct SignalLevelAgent {
    strength: Property<Option<SignalStrength>>,
}

impl SignalLevelAgent {
    pub(crate) fn new(strength: Property<Option<SignalStrength>>) -> Self {
        Self { strength }
    }
}

#[interface(name = "net.connman.iwd.SignalLevelAgent")]
impl SignalLevelAgent {
    /// Called by IWD when the agent is unregistered.
    fn release(&self, _device: OwnedObjectPath) {
        debug!("iwd signal-level agent released");
    }

    /// Called when the connected link's RSSI crosses a registered threshold, and
    /// once immediately after registration. `level` is `0` (strongest) to `N`
    /// (weakest), where `N` is the number of registered thresholds.
    fn changed(&self, _device: OwnedObjectPath, level: u8) {
        self.strength.set(Some(SignalStrength::from_level(level)));
    }
}
