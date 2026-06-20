//! The wayland `Dispatch` target: owns the authoritative local maps and
//! republishes them to the public [`Property`](wayle_core::Property) fields
//! once per `done` event, matching the protocol's atomic-update boundary.

use std::{collections::HashMap, sync::Arc};

use wayland_client::Connection;
use wayland_protocols::ext::workspace::v1::client::ext_workspace_manager_v1::ExtWorkspaceManagerV1;
use wayle_core::Property;

use crate::core::{Workspace, WorkspaceGroup};

pub(crate) struct State {
    #[allow(dead_code)]
    pub(crate) connection: Connection,
    #[allow(dead_code)]
    pub(crate) manager: ExtWorkspaceManagerV1,

    pub(crate) local_workspaces: HashMap<u32, Arc<Workspace>>,
    pub(crate) local_groups: HashMap<u32, Arc<WorkspaceGroup>>,

    pub(crate) workspaces: Property<HashMap<u32, Arc<Workspace>>>,
    pub(crate) groups: Property<HashMap<u32, Arc<WorkspaceGroup>>>,
}

impl State {
    pub(crate) fn publish(&self) {
        self.workspaces.replace(self.local_workspaces.clone());
        self.groups.replace(self.local_groups.clone());
    }
}
