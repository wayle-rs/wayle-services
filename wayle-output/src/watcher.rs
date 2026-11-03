use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use smithay_client_toolkit::{
    delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        client::{
            Connection, Dispatch, Proxy, QueueHandle,
            backend::ObjectId,
            event_created_child,
            globals::{GlobalList, registry_queue_init},
            protocol::wl_output::WlOutput,
        },
        protocols_wlr::output_management::v1::client::{
            zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
            zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
            zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};
use wayle_core::{Property, monitor::MonitorInfo};

use crate::{
    OutputEvent,
    registry::{self, Head},
};

const OUTPUT_MANAGER_VERSION: u32 = 4;
// head.release and mode.release exist since v3
const RELEASE_SINCE: u32 = 3;

/// Starts the watcher thread. The returned receiver fires once the initial
/// output list is published.
pub(crate) fn spawn(
    outputs: Property<Vec<MonitorInfo>>,
    events: broadcast::Sender<OutputEvent>,
    shutdown: Arc<AtomicBool>,
) -> Option<mpsc::Receiver<()>> {
    let conn = match Connection::connect_to_env() {
        Ok(conn) => conn,
        Err(err) => {
            warn!(error = %err, "cannot connect to Wayland display");
            return None;
        }
    };

    let (ready_tx, ready_rx) = mpsc::channel();

    let spawned = thread::Builder::new()
        .name("wayland-outputs".into())
        .spawn(move || run(&conn, &outputs, &events, &shutdown, &ready_tx));

    if let Err(err) = spawned {
        warn!(error = %err, "cannot spawn Wayland output thread");
        return None;
    }

    Some(ready_rx)
}

#[allow(clippy::cognitive_complexity)]
fn run(
    conn: &Connection,
    outputs: &Property<Vec<MonitorInfo>>,
    events: &broadcast::Sender<OutputEvent>,
    shutdown: &AtomicBool,
    ready: &mpsc::Sender<()>,
) {
    let (globals, mut queue) = match registry_queue_init(conn) {
        Ok(result) => result,
        Err(err) => {
            warn!(error = %err, "cannot initialize Wayland registry");
            return;
        }
    };

    let qh = queue.handle();
    let mut state = State::new(&globals, &qh);

    // first roundtrip announces outputs and heads, second delivers their details
    for _ in 0..2 {
        if let Err(err) = queue.roundtrip(&mut state) {
            warn!(error = %err, "Wayland roundtrip failed");
            return;
        }
    }

    publish(&state, outputs, events);
    let _ = ready.send(());

    while !shutdown.load(Ordering::Relaxed) {
        if let Err(err) = queue.blocking_dispatch(&mut state) {
            warn!(error = %err, "Wayland dispatch error");
            return;
        }
        publish(&state, outputs, events);
    }
}

fn publish(
    state: &State,
    outputs: &Property<Vec<MonitorInfo>>,
    events: &broadcast::Sender<OutputEvent>,
) {
    let next = state.snapshot();
    let changes = registry::diff(&outputs.get(), &next);
    if changes.is_empty() {
        return;
    }

    outputs.set(next);

    for change in changes {
        match &change {
            OutputEvent::Added(monitor) => {
                info!(connector = %monitor.connector, description = %monitor.description(), "output added");
            }
            OutputEvent::Changed(monitor) => {
                info!(connector = %monitor.connector, description = %monitor.description(), "output changed");
            }
            OutputEvent::Removed(connector) => info!(%connector, "output removed"),
        }
        let _ = events.send(change);
    }
}

struct State {
    registry_state: RegistryState,
    output_state: OutputState,
    heads: HashMap<ObjectId, Head>,
}

impl State {
    fn new(globals: &GlobalList, qh: &QueueHandle<Self>) -> Self {
        match globals.bind::<ZwlrOutputManagerV1, _, _>(qh, 1..=OUTPUT_MANAGER_VERSION, ()) {
            Ok(_) => debug!("bound zwlr_output_manager_v1"),
            Err(err) => {
                debug!(error = %err, "no zwlr_output_manager_v1, serials from descriptions only")
            }
        }

        Self {
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            heads: HashMap::new(),
        }
    }

    fn snapshot(&self) -> Vec<MonitorInfo> {
        let outputs: Vec<registry::WlOutput> = self
            .output_state
            .outputs()
            .filter_map(|output| self.output_state.info(&output))
            .filter_map(|info| {
                Some(registry::WlOutput {
                    name: info.name?,
                    make: info.make,
                    model: info.model,
                    description: info.description.unwrap_or_default(),
                })
            })
            .collect();

        registry::merge(&outputs, self.heads.values())
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

impl Dispatch<ZwlrOutputManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state.heads.insert(head.id(), Head::default());
            }
            zwlr_output_manager_v1::Event::Finished => state.heads.clear(),
            _ => {}
        }
    }

    event_created_child!(State, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for State {
    fn event(
        state: &mut Self,
        head: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_output_head_v1::Event::Finished = event {
            state.heads.remove(&head.id());
            if head.version() >= RELEASE_SINCE {
                head.release();
            }
            return;
        }

        let Some(entry) = state.heads.get_mut(&head.id()) else {
            return;
        };

        match event {
            zwlr_output_head_v1::Event::Name { name } => entry.name = Some(name),
            zwlr_output_head_v1::Event::Make { make } => entry.make = Some(make),
            zwlr_output_head_v1::Event::Model { model } => entry.model = Some(model),
            zwlr_output_head_v1::Event::SerialNumber { serial_number } => {
                entry.serial = Some(serial_number);
            }
            _ => {}
        }
    }

    event_created_child!(State, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for State {
    fn event(
        _: &mut Self,
        mode: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_output_mode_v1::Event::Finished = event
            && mode.version() >= RELEASE_SINCE
        {
            mode.release();
        }
    }
}

delegate_registry!(State);
delegate_output!(State);
