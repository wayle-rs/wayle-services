//! Reactive wrapper for an `ext_workspace_handle_v1` object.

use derive_more::Debug;
use wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1::ExtWorkspaceHandleV1;
use wayle_core::Property;

/// Bit set on [`Workspace::state`] when the workspace is currently displayed.
pub const STATE_ACTIVE: u32 = 1;
/// Bit set on [`Workspace::state`] when the workspace requests attention.
pub const STATE_URGENT: u32 = 2;
/// Bit set on [`Workspace::state`] when the workspace is not currently visible.
pub const STATE_HIDDEN: u32 = 4;

/// A workspace advertised by `ext_workspace_manager_v1`.
///
/// Fields update in place as the compositor sends events. Identity (the
/// [`Workspace::key`]) is the wayland object id and is stable for the
/// lifetime of the handle, but is not preserved across reconnects.
#[derive(Debug)]
pub struct Workspace {
    #[debug(skip)]
    pub(crate) handle: ExtWorkspaceHandleV1,

    /// Local, process-lifetime-stable key (the wayland object id).
    pub key: u32,
    /// Compositor-assigned stable id, if the compositor sends one.
    pub id: Property<Option<String>>,
    /// Human-readable workspace name.
    pub name: Property<Option<String>>,
    /// N-dimensional coordinates, if the compositor arranges workspaces
    /// geometrically. Empty when the compositor does not send this event.
    pub coordinates: Property<Vec<u32>>,
    /// Bitmask of `STATE_*` flags.
    pub state: Property<u32>,
    /// Bitmask of capability flags advertised by the compositor.
    pub capabilities: Property<u32>,
}

impl Workspace {
    pub(crate) fn new(handle: ExtWorkspaceHandleV1, key: u32) -> Self {
        Self {
            handle,
            key,
            id: Property::new(None),
            name: Property::new(None),
            coordinates: Property::new(Vec::new()),
            state: Property::new(0),
            capabilities: Property::new(0),
        }
    }

    /// Whether the workspace is currently displayed.
    pub fn is_active(&self) -> bool {
        self.state.get() & STATE_ACTIVE != 0
    }

    /// Whether the workspace requests attention.
    pub fn is_urgent(&self) -> bool {
        self.state.get() & STATE_URGENT != 0
    }

    /// Whether the workspace is hidden from its workspace group.
    pub fn is_hidden(&self) -> bool {
        self.state.get() & STATE_HIDDEN != 0
    }

    /// Queues a request that the compositor activate this workspace.
    ///
    /// Requests are only applied once the connection is committed - see
    /// [`ExtWorkspaceService::activate_workspace`](crate::ExtWorkspaceService::activate_workspace)
    /// for the usual entry point.
    pub fn activate(&self) {
        self.handle.activate();
    }

    /// Queues a request that the compositor deactivate this workspace.
    pub fn deactivate(&self) {
        self.handle.deactivate();
    }
}

impl PartialEq for Workspace {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
