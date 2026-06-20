//! `Dispatch` implementations translating raw protocol events into updates
//! on [`State`]'s local maps.

use std::sync::Arc;

use tracing::warn;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    globals::GlobalListContents,
    protocol::{wl_output, wl_registry},
};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
    ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};

use super::state::State;
use crate::core::{Workspace, WorkspaceGroup};

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for State {
    // `event_created_child` has no fallible return path; panicking here only
    // fires for an opcode outside the protocol spec, i.e. a compositor bug.
    #[allow(clippy::panic)]
    fn event_created_child(
        opcode: u16,
        qh: &QueueHandle<Self>,
    ) -> Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            0 => qh.make_data::<ExtWorkspaceGroupHandleV1, ()>(()),
            1 => qh.make_data::<ExtWorkspaceHandleV1, ()>(()),
            _ => panic!("unexpected new_id opcode {opcode} on ext_workspace_manager_v1"),
        }
    }

    fn event(
        state: &mut Self,
        _proxy: &ExtWorkspaceManagerV1,
        event: ext_workspace_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_workspace_manager_v1::Event::WorkspaceGroup { workspace_group } => {
                let key = workspace_group.id().protocol_id();
                state
                    .local_groups
                    .insert(key, Arc::new(WorkspaceGroup::new(workspace_group, key)));
            }
            ext_workspace_manager_v1::Event::Workspace { workspace } => {
                let key = workspace.id().protocol_id();
                state
                    .local_workspaces
                    .insert(key, Arc::new(Workspace::new(workspace, key)));
            }
            ext_workspace_manager_v1::Event::Done => state.publish(),
            ext_workspace_manager_v1::Event::Finished => {
                warn!("compositor closed the ext_workspace_manager_v1 object");
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ExtWorkspaceGroupHandleV1,
        event: ext_workspace_group_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let key = proxy.id().protocol_id();
        let Some(group) = state.local_groups.get(&key).cloned() else {
            return;
        };

        match event {
            ext_workspace_group_handle_v1::Event::Capabilities { capabilities } => {
                group.capabilities.set(enum_bits(capabilities));
            }
            ext_workspace_group_handle_v1::Event::OutputEnter { output } => {
                let output_id = output.id().protocol_id();
                let mut outputs = group.outputs.get();
                if !outputs.contains(&output_id) {
                    outputs.push(output_id);
                    group.outputs.set(outputs);
                }
            }
            ext_workspace_group_handle_v1::Event::OutputLeave { output } => {
                let output_id = output.id().protocol_id();
                let mut outputs = group.outputs.get();
                outputs.retain(|id| *id != output_id);
                group.outputs.set(outputs);
            }
            ext_workspace_group_handle_v1::Event::WorkspaceEnter { workspace } => {
                let workspace_key = workspace.id().protocol_id();
                let mut workspaces = group.workspaces.get();
                if !workspaces.contains(&workspace_key) {
                    workspaces.push(workspace_key);
                    group.workspaces.set(workspaces);
                }
            }
            ext_workspace_group_handle_v1::Event::WorkspaceLeave { workspace } => {
                let workspace_key = workspace.id().protocol_id();
                let mut workspaces = group.workspaces.get();
                workspaces.retain(|id| *id != workspace_key);
                group.workspaces.set(workspaces);
            }
            ext_workspace_group_handle_v1::Event::Removed => {
                state.local_groups.remove(&key);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtWorkspaceHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ExtWorkspaceHandleV1,
        event: ext_workspace_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let key = proxy.id().protocol_id();
        let Some(workspace) = state.local_workspaces.get(&key).cloned() else {
            return;
        };

        match event {
            ext_workspace_handle_v1::Event::Id { id } => workspace.id.set(Some(id)),
            ext_workspace_handle_v1::Event::Name { name } => workspace.name.set(Some(name)),
            ext_workspace_handle_v1::Event::Coordinates { coordinates } => {
                let parsed = coordinates
                    .chunks_exact(4)
                    .map(|chunk| u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect();
                workspace.coordinates.set(parsed);
            }
            ext_workspace_handle_v1::Event::State { state: bits } => {
                workspace.state.set(enum_bits(bits));
            }
            ext_workspace_handle_v1::Event::Capabilities { capabilities } => {
                workspace.capabilities.set(enum_bits(capabilities));
            }
            ext_workspace_handle_v1::Event::Removed => {
                state.local_workspaces.remove(&key);
            }
            _ => {}
        }
    }
}

fn enum_bits<T>(value: WEnum<T>) -> u32
where
    T: Into<u32>,
{
    match value {
        WEnum::Value(inner) => inner.into(),
        WEnum::Unknown(raw) => raw,
    }
}
