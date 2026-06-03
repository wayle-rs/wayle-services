//! Applies `all-monitors` and `all-clients` frames to the reactive [`Property`]
//! fields.
//!
//! Each frame rebuilds its list wholesale, so any change Mango reports
//! propagates to watchers. Keyboard layout and key mode are global in Mango, so
//! they are read off the active monitor rather than stored per monitor.

use tracing::warn;

use super::MonitoringHandles;
use crate::{
    core::{Client, Monitor},
    types::{ClientList, MonitorList},
};

/// Parses an `all-monitors` frame and refreshes the affected [`Property`] fields.
pub(super) fn apply_monitor_frame(handles: &MonitoringHandles, line: &str) {
    match serde_json::from_str::<MonitorList>(line) {
        Ok(list) => apply_monitors(handles, list),
        Err(err) => warn!(error = %err, line, "cannot parse mango monitor frame"),
    }
}

/// Parses an `all-clients` frame and refreshes the clients [`Property`].
pub(super) fn apply_client_frame(handles: &MonitoringHandles, line: &str) {
    match serde_json::from_str::<ClientList>(line) {
        Ok(list) => {
            let clients = list
                .clients
                .into_iter()
                .map(Client::from_snapshot)
                .collect();
            handles.clients.set(clients);
        }
        Err(err) => warn!(error = %err, line, "cannot parse mango client frame"),
    }
}

fn apply_monitors(handles: &MonitoringHandles, list: MonitorList) {
    refresh_globals(handles, &list);

    let monitors = list
        .monitors
        .into_iter()
        .map(Monitor::from_snapshot)
        .collect();
    handles.monitors.set(monitors);
}

fn refresh_globals(handles: &MonitoringHandles, list: &MonitorList) {
    let active = list.monitors.iter().find(|monitor| monitor.active);

    let focused_client = active.and_then(|monitor| monitor.active_client.clone().into_focused());
    let keyboard_layout = active.and_then(|monitor| non_empty(&monitor.keyboardlayout));
    let keymode = active.and_then(|monitor| non_empty(&monitor.keymode));

    handles.focused_client.set(focused_client);
    handles.keyboard_layout.set(keyboard_layout);
    handles.keymode.set(keymode);
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }

    Some(value.to_owned())
}
