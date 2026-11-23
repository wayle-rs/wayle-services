//! `Dispatch` implementations translating raw protocol events into updates
//! on [`State`]'s local map.

use std::sync::Arc;

use tracing::warn;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::GlobalListContents,
    protocol::{wl_output, wl_registry, wl_seat},
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use super::state::State;
use crate::core::Toplevel;

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

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    // `event_created_child` has no fallible return path; panicking here only
    // fires for an opcode outside the protocol spec, i.e. a compositor bug.
    #[allow(clippy::panic)]
    fn event_created_child(
        opcode: u16,
        qh: &QueueHandle<Self>,
    ) -> Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            0 => qh.make_data::<ZwlrForeignToplevelHandleV1, ()>(()),
            _ => panic!("unexpected new_id opcode {opcode} on zwlr_foreign_toplevel_manager_v1"),
        }
    }

    fn event(
        state: &mut Self,
        _proxy: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                let key = toplevel.id().protocol_id();
                state
                    .local_toplevels
                    .insert(key, Arc::new(Toplevel::new(toplevel, key)));
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                warn!("compositor closed the zwlr_foreign_toplevel_manager_v1 object");
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let key = proxy.id().protocol_id();
        let Some(toplevel) = state.local_toplevels.get(&key).cloned() else {
            return;
        };

        match event {
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                toplevel.title.set(Some(title));
            }
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                toplevel.app_id.set(Some(app_id));
            }
            zwlr_foreign_toplevel_handle_v1::Event::OutputEnter { output } => {
                let output_id = output.id().protocol_id();
                let mut outputs = toplevel.outputs.get();
                if !outputs.contains(&output_id) {
                    outputs.push(output_id);
                    toplevel.outputs.set(outputs);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::OutputLeave { output } => {
                let output_id = output.id().protocol_id();
                let mut outputs = toplevel.outputs.get();
                outputs.retain(|id| *id != output_id);
                toplevel.outputs.set(outputs);
            }
            zwlr_foreign_toplevel_handle_v1::Event::State { state: bits } => {
                toplevel.state.set(decode_state_array(&bits));
            }
            zwlr_foreign_toplevel_handle_v1::Event::Parent { parent } => {
                toplevel
                    .parent
                    .set(parent.map(|handle| handle.id().protocol_id()));
            }
            // `done` marks one toplevel's batch of changes as atomic (unlike
            // ext-workspace-v1, where `done` is a single event on the
            // manager covering every workspace at once) - publish here.
            zwlr_foreign_toplevel_handle_v1::Event::Done => state.publish(),
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                state.local_toplevels.remove(&key);
                state.publish();
            }
            _ => {}
        }
    }
}

/// Decodes the `state` event's `array` argument: a sequence of u32 (native
/// endian) enum values (`maximized=0, minimized=1, activated=2,
/// fullscreen=3`), combined into a single local bitmask.
fn decode_state_array(bytes: &[u8]) -> u32 {
    use crate::core::toplevel::{
        STATE_ACTIVATED, STATE_FULLSCREEN, STATE_MAXIMIZED, STATE_MINIMIZED,
    };

    let mut bits = 0u32;
    for chunk in bytes.chunks_exact(4) {
        let value = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        bits |= match value {
            0 => STATE_MAXIMIZED,
            1 => STATE_MINIMIZED,
            2 => STATE_ACTIVATED,
            3 => STATE_FULLSCREEN,
            _ => 0,
        };
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::toplevel::{STATE_ACTIVATED, STATE_MAXIMIZED, STATE_MINIMIZED};

    #[test]
    fn decodes_empty_state() {
        assert_eq!(decode_state_array(&[]), 0);
    }

    #[test]
    fn decodes_single_state() {
        let bytes = 2u32.to_ne_bytes();
        assert_eq!(decode_state_array(&bytes), STATE_ACTIVATED);
    }

    #[test]
    fn decodes_combined_states() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u32.to_ne_bytes());
        bytes.extend_from_slice(&2u32.to_ne_bytes());
        assert_eq!(
            decode_state_array(&bytes),
            STATE_MAXIMIZED | STATE_ACTIVATED
        );
    }

    #[test]
    fn ignores_unknown_state_value() {
        let bytes = 99u32.to_ne_bytes();
        assert_eq!(decode_state_array(&bytes), 0);
    }

    #[test]
    fn decodes_minimized() {
        let bytes = 1u32.to_ne_bytes();
        assert_eq!(decode_state_array(&bytes), STATE_MINIMIZED);
    }
}
