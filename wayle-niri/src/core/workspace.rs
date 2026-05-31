//! Reactive wrapper for a niri workspace.

use niri_ipc::Workspace as NiriWorkspace;
use wayle_core::Property;

/// A niri workspace with reactive state.
///
/// Fields mirror [`niri_ipc::Workspace`]. Instances from
/// [`NiriService`](crate::NiriService) fields update in place as niri emits
/// events.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Stable id that does not change when the workspace moves between outputs.
    pub id: Property<u64>,
    /// Position of the workspace on its output.
    ///
    /// Re-ordering and workspace moves cause this to change. Not unique across
    /// outputs.
    pub idx: Property<u8>,
    /// Optional user-defined workspace name.
    pub name: Property<Option<String>>,
    /// Connector name of the output the workspace is on.
    ///
    /// `None` when no outputs are connected.
    pub output: Property<Option<String>>,
    /// Whether any window on this workspace requested attention.
    pub is_urgent: Property<bool>,
    /// Whether this is the currently visible workspace on its output.
    ///
    /// Exactly one workspace per output has this flag.
    pub is_active: Property<bool>,
    /// Whether this workspace holds global focus.
    ///
    /// Exactly one workspace across all outputs has this flag.
    pub is_focused: Property<bool>,
    /// Id of the currently focused window on this workspace.
    ///
    /// Can briefly reference a window that no longer exists.
    pub active_window_id: Property<Option<u64>>,
}

impl Workspace {
    pub(crate) fn from_niri(workspace: NiriWorkspace) -> Self {
        Self {
            id: Property::new(workspace.id),
            idx: Property::new(workspace.idx),
            name: Property::new(workspace.name),
            output: Property::new(workspace.output),
            is_urgent: Property::new(workspace.is_urgent),
            is_active: Property::new(workspace.is_active),
            is_focused: Property::new(workspace.is_focused),
            active_window_id: Property::new(workspace.active_window_id),
        }
    }

    pub(crate) fn refresh_from_niri(&self, workspace: NiriWorkspace) {
        self.id.set(workspace.id);
        self.idx.set(workspace.idx);
        self.name.set(workspace.name);
        self.output.set(workspace.output);
        self.is_urgent.set(workspace.is_urgent);
        self.is_active.set(workspace.is_active);
        self.is_focused.set(workspace.is_focused);
        self.active_window_id.set(workspace.active_window_id);
    }
}

/// Keyed on `id`. Two workspaces are equal iff they share an id, regardless
/// of field content, so collection-level `PartialEq` compares set-membership.
impl PartialEq for Workspace {
    fn eq(&self, other: &Self) -> bool {
        self.id.get() == other.id.get()
    }
}
