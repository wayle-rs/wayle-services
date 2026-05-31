//! Reactive wrapper for a niri window.

use niri_ipc::{Timestamp, Window as NiriWindow, WindowLayout};
use wayle_core::Property;

/// A niri toplevel window with reactive state.
///
/// Fields mirror [`niri_ipc::Window`]. Instances from
/// [`NiriService`](crate::NiriService) fields update in place as niri emits
/// events, so watching any field reflects live state.
#[derive(Debug, Clone)]
pub struct Window {
    /// Stable id for the window's lifetime.
    pub id: Property<u64>,
    /// Window title if set by the application.
    pub title: Property<Option<String>>,
    /// Wayland application id if set.
    pub app_id: Property<Option<String>>,
    /// PID of the process that created the Wayland connection.
    ///
    /// `None` when niri can't determine it (e.g. xdg-desktop-portal-gnome windows).
    pub pid: Property<Option<i32>>,
    /// Id of the workspace this window is on.
    ///
    /// Can briefly reference a workspace that has already been removed.
    pub workspace_id: Property<Option<u64>>,
    /// Whether this window has input focus.
    ///
    /// At most one window is focused globally; can be zero when a layer-shell
    /// surface holds focus.
    pub is_focused: Property<bool>,
    /// Whether this window is floating.
    ///
    /// `false` means the window is in the tiling layout.
    pub is_floating: Property<bool>,
    /// Whether the window has signalled urgency.
    pub is_urgent: Property<bool>,
    /// Position and size metadata.
    pub layout: Property<WindowLayout>,
    /// Monotonic timestamp of the most recent focus, debounced for MRU switchers.
    pub focus_timestamp: Property<Option<Timestamp>>,
}

impl Window {
    pub(crate) fn from_niri(window: NiriWindow) -> Self {
        Self {
            id: Property::new(window.id),
            title: Property::new(window.title),
            app_id: Property::new(window.app_id),
            pid: Property::new(window.pid),
            workspace_id: Property::new(window.workspace_id),
            is_focused: Property::new(window.is_focused),
            is_floating: Property::new(window.is_floating),
            is_urgent: Property::new(window.is_urgent),
            layout: Property::new(window.layout),
            focus_timestamp: Property::new(window.focus_timestamp),
        }
    }

    pub(crate) fn refresh_from_niri(&self, window: NiriWindow) {
        self.id.set(window.id);
        self.title.set(window.title);
        self.app_id.set(window.app_id);
        self.pid.set(window.pid);
        self.workspace_id.set(window.workspace_id);
        self.is_focused.set(window.is_focused);
        self.is_floating.set(window.is_floating);
        self.is_urgent.set(window.is_urgent);
        self.layout.set(window.layout);
        self.focus_timestamp.set(window.focus_timestamp);
    }
}

/// Keyed on `id`. Two windows are equal iff they share an id, regardless of
/// field content, so collection-level `PartialEq` compares set-membership.
impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        self.id.get() == other.id.get()
    }
}
