//! Precondition checks for events before feeding them into
//! [`EventStreamStatePart::apply`](niri_ipc::state::EventStreamStatePart::apply).
//!
//! niri's state machine panics on several precondition misses (e.g. closing
//! a window id we never saw), which would take down the dispatcher task.
//! Every such site has a matching `require_*` check here so the dispatcher
//! can log and skip instead of crash.

use std::fmt::{self, Display, Formatter};

use niri_ipc::{Event, WindowLayout, state::EventStreamState};

/// Why a given [`Event`] cannot be safely applied to our copy of the state.
#[derive(Debug, Clone, Copy)]
pub(super) enum DesyncReason {
    /// The event references a workspace id we have not seen yet.
    WorkspaceMissing {
        /// The unknown workspace id.
        workspace_id: u64,
    },
    /// The event references a window id we have not seen yet.
    WindowMissing {
        /// The unknown window id.
        window_id: u64,
    },
    /// A layout switch arrived before any layout snapshot.
    KeyboardLayoutsNotLoaded,
}

impl Display for DesyncReason {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkspaceMissing { workspace_id } => {
                write!(formatter, "workspace {workspace_id} not in local state")
            }
            Self::WindowMissing { window_id } => {
                write!(formatter, "window {window_id} not in local state")
            }
            Self::KeyboardLayoutsNotLoaded => {
                formatter.write_str("keyboard-layouts snapshot not received yet")
            }
        }
    }
}

/// Returns the reason an event must be skipped, or `Ok(())` when applying it
/// would not violate niri's `EventStreamState` invariants.
pub(super) fn verify_preconditions(
    event: &Event,
    state: &EventStreamState,
) -> Result<(), DesyncReason> {
    match event {
        Event::WorkspaceActivated { id, .. } => require_workspace_in_state(state, *id),
        Event::WorkspaceActiveWindowChanged { workspace_id, .. } => {
            require_workspace_in_state(state, *workspace_id)
        }
        Event::WindowClosed { id } => require_window_in_state(state, *id),
        Event::WindowLayoutsChanged { changes } => require_all_windows_in_state(state, changes),
        Event::KeyboardLayoutSwitched { .. } => require_keyboard_layouts_loaded(state),
        _ => Ok(()),
    }
}

fn require_workspace_in_state(
    state: &EventStreamState,
    workspace_id: u64,
) -> Result<(), DesyncReason> {
    if state.workspaces.workspaces.contains_key(&workspace_id) {
        return Ok(());
    }
    Err(DesyncReason::WorkspaceMissing { workspace_id })
}

fn require_window_in_state(state: &EventStreamState, window_id: u64) -> Result<(), DesyncReason> {
    if state.windows.windows.contains_key(&window_id) {
        return Ok(());
    }
    Err(DesyncReason::WindowMissing { window_id })
}

fn require_all_windows_in_state(
    state: &EventStreamState,
    changes: &[(u64, WindowLayout)],
) -> Result<(), DesyncReason> {
    for (window_id, _layout) in changes {
        require_window_in_state(state, *window_id)?;
    }
    Ok(())
}

fn require_keyboard_layouts_loaded(state: &EventStreamState) -> Result<(), DesyncReason> {
    if state.keyboard_layouts.keyboard_layouts.is_some() {
        return Ok(());
    }
    Err(DesyncReason::KeyboardLayoutsNotLoaded)
}
