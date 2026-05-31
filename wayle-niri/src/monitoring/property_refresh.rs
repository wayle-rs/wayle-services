//! Refreshes the service's reactive [`Property`](wayle_core::Property) fields
//! from the current [`EventStreamState`].
//!
//! [`refresh_properties_for_event`] is the entry point; the per-target
//! `refresh_*` helpers below preserve [`Arc`] identity for entities that are
//! still present so that per-field watchers on a [`Window`] or [`Workspace`]
//! only fire when that specific field changes.

use std::{collections::HashMap, sync::Arc};

use niri_ipc::{Event, state::EventStreamState};

use super::DispatcherInputs;
use crate::core::{Window, Workspace};

/// Refreshes whichever Property fields this event could have affected.
pub(super) fn refresh_properties_for_event(
    inputs: &DispatcherInputs,
    state: &EventStreamState,
    event: &Event,
) {
    match event {
        Event::WorkspacesChanged { .. }
        | Event::WorkspaceUrgencyChanged { .. }
        | Event::WorkspaceActivated { .. }
        | Event::WorkspaceActiveWindowChanged { .. } => {
            refresh_workspaces(inputs, state);
        }

        Event::WindowsChanged { .. }
        | Event::WindowOpenedOrChanged { .. }
        | Event::WindowClosed { .. }
        | Event::WindowFocusChanged { .. }
        | Event::WindowUrgencyChanged { .. }
        | Event::WindowLayoutsChanged { .. } => {
            refresh_windows(inputs, state);
            refresh_focused_window_id(inputs, state);
        }

        Event::WindowFocusTimestampChanged { .. } => {
            refresh_windows(inputs, state);
        }

        Event::KeyboardLayoutsChanged { .. } | Event::KeyboardLayoutSwitched { .. } => {
            refresh_keyboard_layouts(inputs, state);
        }

        Event::OverviewOpenedOrClosed { .. } => refresh_overview_open(inputs, state),
        Event::ConfigLoaded { .. } => refresh_config_failed(inputs, state),

        Event::ScreenshotCaptured { .. } => {}
    }
}

fn refresh_windows(inputs: &DispatcherInputs, state: &EventStreamState) {
    let current = inputs.windows.get();
    let mut updated: HashMap<u64, Arc<Window>> =
        HashMap::with_capacity(state.windows.windows.len());

    for (window_id, niri_window) in &state.windows.windows {
        match current.get(window_id) {
            Some(existing_window) => {
                existing_window.refresh_from_niri(niri_window.clone());
                updated.insert(*window_id, Arc::clone(existing_window));
            }
            None => {
                updated.insert(*window_id, Arc::new(Window::from_niri(niri_window.clone())));
            }
        }
    }

    inputs.windows.set(updated);
}

fn refresh_workspaces(inputs: &DispatcherInputs, state: &EventStreamState) {
    let current = inputs.workspaces.get();
    let mut updated: HashMap<u64, Arc<Workspace>> =
        HashMap::with_capacity(state.workspaces.workspaces.len());

    for (workspace_id, niri_workspace) in &state.workspaces.workspaces {
        match current.get(workspace_id) {
            Some(existing_workspace) => {
                existing_workspace.refresh_from_niri(niri_workspace.clone());
                updated.insert(*workspace_id, Arc::clone(existing_workspace));
            }
            None => {
                updated.insert(
                    *workspace_id,
                    Arc::new(Workspace::from_niri(niri_workspace.clone())),
                );
            }
        }
    }

    inputs.workspaces.set(updated);
}

fn refresh_focused_window_id(inputs: &DispatcherInputs, state: &EventStreamState) {
    let focused_window_id = state
        .windows
        .windows
        .values()
        .find(|window| window.is_focused)
        .map(|window| window.id);
    inputs.focused_window_id.set(focused_window_id);
}

fn refresh_overview_open(inputs: &DispatcherInputs, state: &EventStreamState) {
    inputs.overview_open.set(state.overview.is_open);
}

fn refresh_config_failed(inputs: &DispatcherInputs, state: &EventStreamState) {
    inputs.config_failed.set(state.config.failed);
}

fn refresh_keyboard_layouts(inputs: &DispatcherInputs, state: &EventStreamState) {
    inputs
        .keyboard_layouts
        .set(state.keyboard_layouts.keyboard_layouts.clone());
}
