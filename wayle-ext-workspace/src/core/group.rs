//! Reactive wrapper for an `ext_workspace_group_handle_v1` object.

use derive_more::Debug;
use wayland_protocols::ext::workspace::v1::client::ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1;
use wayle_core::Property;

/// A workspace group advertised by `ext_workspace_manager_v1`.
///
/// Compositors typically advertise one group per output, but the protocol
/// allows a single group to span several outputs (or none).
#[derive(Debug)]
pub struct WorkspaceGroup {
    #[debug(skip)]
    #[allow(dead_code)]
    pub(crate) handle: ExtWorkspaceGroupHandleV1,

    /// Local, process-lifetime-stable key (the wayland object id).
    pub key: u32,
    /// Wayland object ids of the outputs this group is assigned to.
    ///
    /// Exposed as raw object ids rather than connector names; see the
    /// crate-level limitations note.
    pub outputs: Property<Vec<u32>>,
    /// Local keys of workspaces currently assigned to this group, in
    /// arrival order.
    pub workspaces: Property<Vec<u32>>,
    /// Bitmask of capability flags advertised by the compositor.
    pub capabilities: Property<u32>,
}

impl WorkspaceGroup {
    pub(crate) fn new(handle: ExtWorkspaceGroupHandleV1, key: u32) -> Self {
        Self {
            handle,
            key,
            outputs: Property::new(Vec::new()),
            workspaces: Property::new(Vec::new()),
            capabilities: Property::new(0),
        }
    }
}

impl PartialEq for WorkspaceGroup {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
