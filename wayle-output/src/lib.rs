//! Connected Wayland outputs with a stable identity.
//!
//! Connector names like `DP-1` can change between hotplugs on docks and
//! MST hubs. [`OutputService`] pairs each connector with make, model and
//! serial so outputs can be targeted with [`wayle_core::monitor::MonitorSelector`].
//!
//! Serials come from `zwlr_output_manager_v1` when the compositor offers it
//! (Hyprland, sway and other wlroots compositors, niri, COSMIC), otherwise
//! they are parsed from the `wl_output` description.

mod registry;
mod watcher;

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::sync::broadcast;
use tracing::warn;
pub use wayle_core::monitor::MonitorInfo;
use wayle_core::{Property, monitor::MonitorSelector};

const READY_TIMEOUT: Duration = Duration::from_secs(2);
const EVENT_CAPACITY: usize = 64;

/// Change to the set of connected outputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputEvent {
    /// Output connected.
    Added(MonitorInfo),
    /// Identity of a connected output changed, e.g. its serial arrived late
    /// or another monitor now uses the same connector.
    Changed(MonitorInfo),
    /// Output with this connector disconnected.
    Removed(String),
}

/// Tracks connected outputs on a background Wayland connection.
pub struct OutputService {
    /// Connected outputs in compositor order.
    pub outputs: Property<Vec<MonitorInfo>>,
    events: broadcast::Sender<OutputEvent>,
    shutdown: Arc<AtomicBool>,
}

impl OutputService {
    /// Connects to the compositor and waits for the initial output list.
    ///
    /// Returns `None` if there is no Wayland display.
    pub fn start() -> Option<Arc<Self>> {
        let outputs = Property::new(Vec::new());
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let shutdown = Arc::new(AtomicBool::new(false));

        let ready = watcher::spawn(outputs.clone(), events.clone(), shutdown.clone())?;
        if ready.recv_timeout(READY_TIMEOUT).is_err() {
            warn!("timed out waiting for initial Wayland outputs");
        }

        Some(Arc::new(Self {
            outputs,
            events,
            shutdown,
        }))
    }

    /// Output with this connector name.
    pub fn find(&self, connector: &str) -> Option<MonitorInfo> {
        self.outputs
            .get()
            .into_iter()
            .find(|monitor| monitor.connector == connector)
    }

    /// Outputs matching `selector`, in compositor order.
    pub fn matching(&self, selector: &MonitorSelector) -> Vec<MonitorInfo> {
        self.outputs
            .get()
            .into_iter()
            .filter(|monitor| selector.matches(monitor))
            .collect()
    }

    /// Receiver for output changes after this call.
    pub fn events(&self) -> broadcast::Receiver<OutputEvent> {
        self.events.subscribe()
    }
}

impl fmt::Debug for OutputService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputService")
            .field("outputs", &self.outputs.get())
            .finish_non_exhaustive()
    }
}

impl Drop for OutputService {
    fn drop(&mut self) {
        // The thread blocks in dispatch; it exits on its next wakeup.
        self.shutdown.store(true, Ordering::Relaxed);
    }
}
