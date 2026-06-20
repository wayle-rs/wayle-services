//! The [`ExtWorkspaceService`] type: connects to the compositor, runs the
//! protocol's event loop on a dedicated thread, and exposes the resulting
//! state through [`Property`](wayle_core::Property) fields.

use std::{collections::HashMap, sync::Arc};

use wayland_client::{Connection, globals::registry_queue_init};
use wayland_protocols::ext::workspace::v1::client::ext_workspace_manager_v1::ExtWorkspaceManagerV1;
use wayle_core::Property;

use crate::{
    core::{Workspace, WorkspaceGroup},
    error::Result,
    wayland::state::State,
};

/// Reactive bindings to any compositor implementing `ext-workspace-v1`.
///
/// Unlike compositor-specific services (`wayle-niri`, `wayle-hyprland`),
/// this works on any Wayland compositor that advertises the
/// `ext_workspace_manager_v1` global, including wlroots-based compositors
/// such as Sway.
pub struct ExtWorkspaceService {
    manager: ExtWorkspaceManagerV1,
    connection: Connection,

    /// All workspace groups, keyed by their wayland object id.
    pub groups: Property<HashMap<u32, Arc<WorkspaceGroup>>>,
    /// All workspaces, keyed by their wayland object id.
    pub workspaces: Property<HashMap<u32, Arc<Workspace>>>,
}

impl ExtWorkspaceService {
    /// Connects to the compositor and binds `ext_workspace_manager_v1`.
    ///
    /// # Errors
    ///
    /// - [`Error::ConnectionFailed`](crate::Error::ConnectionFailed) if no
    ///   Wayland display is reachable.
    /// - [`Error::NotSupported`](crate::Error::NotSupported) if the
    ///   compositor does not advertise `ext_workspace_manager_v1`.
    /// - [`Error::Dispatch`](crate::Error::Dispatch) if the initial
    ///   round-trip fails.
    /// - [`Error::ThreadSpawnFailed`](crate::Error::ThreadSpawnFailed) if the
    ///   background event-loop thread cannot be spawned.
    pub fn new() -> Result<Arc<Self>> {
        let connection = Connection::connect_to_env()?;
        let (globals, mut event_queue) = registry_queue_init::<State>(&connection)?;
        let qh = event_queue.handle();

        let manager: ExtWorkspaceManagerV1 = globals.bind(&qh, 1..=1, ())?;

        let workspaces = Property::new(HashMap::new());
        let groups = Property::new(HashMap::new());

        let mut state = State {
            connection: connection.clone(),
            manager: manager.clone(),
            local_workspaces: HashMap::new(),
            local_groups: HashMap::new(),
            workspaces: workspaces.clone(),
            groups: groups.clone(),
        };

        // Drive the initial registry/manager round-trip synchronously so the
        // first snapshot is available as soon as `new()` returns.
        event_queue.roundtrip(&mut state)?;

        std::thread::Builder::new()
            .name("wayle-ext-workspace".into())
            .spawn(move || {
                loop {
                    if event_queue.blocking_dispatch(&mut state).is_err() {
                        break;
                    }
                }
            })?;

        Ok(Arc::new(Self {
            manager,
            connection,
            groups,
            workspaces,
        }))
    }

    /// Looks up a workspace by its wayland object id.
    pub fn workspace(&self, key: u32) -> Option<Arc<Workspace>> {
        self.workspaces.get().get(&key).cloned()
    }

    /// Looks up a workspace group by its wayland object id.
    pub fn group(&self, key: u32) -> Option<Arc<WorkspaceGroup>> {
        self.groups.get().get(&key).cloned()
    }

    /// Requests that the compositor activate the given workspace, then
    /// commits the request and flushes the connection.
    ///
    /// There is no guarantee the compositor honors the request; watch
    /// [`Workspace::state`](crate::core::Workspace::state) to observe the
    /// actual outcome.
    pub fn activate_workspace(&self, key: u32) {
        let Some(workspace) = self.workspace(key) else {
            return;
        };
        workspace.activate();
        self.commit();
    }

    fn commit(&self) {
        self.manager.commit();
        let _ = self.connection.flush();
    }
}
