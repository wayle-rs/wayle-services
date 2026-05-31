//! Event loop: drain the pre-subscribed inbound receiver, run preconditions,
//! refresh [`Property`](wayle_core::Property) fields, re-broadcast on the
//! public channel.

use niri_ipc::{
    Event,
    state::{EventStreamState, EventStreamStatePart},
};
use tokio::sync::broadcast::error::RecvError;
use tracing::{error, instrument, warn};

use super::{DispatcherInputs, precondition, property_refresh};

/// Spawns the dispatcher task on the current tokio runtime.
pub(super) fn spawn(inputs: DispatcherInputs) {
    tokio::spawn(event_loop(inputs));
}

#[instrument(skip(inputs))]
async fn event_loop(mut inputs: DispatcherInputs) {
    let mut state = EventStreamState::default();

    loop {
        tokio::select! {
            _ = inputs.cancellation_token.cancelled() => return,
            received = inputs.inbound_event_rx.recv() => match received {
                Ok(event) => apply_event(&inputs, &mut state, event),
                Err(RecvError::Lagged(dropped_events)) => {
                    error!(
                        dropped_events,
                        "dispatcher lagged behind event stream; Properties are now stale until the shell restarts",
                    );
                }
                Err(RecvError::Closed) => return,
            }
        }
    }
}

fn apply_event(inputs: &DispatcherInputs, state: &mut EventStreamState, event: Event) {
    if let Err(desync_reason) = precondition::verify_preconditions(&event, state) {
        warn!(desync_reason = %desync_reason, event = ?event, "skipping desynced niri event");
        broadcast_event(inputs, event);
        return;
    }

    let unconsumed = state.apply(event.clone());
    property_refresh::refresh_properties_for_event(inputs, state, &event);

    if let Some(unconsumed_event) = unconsumed {
        warn!(unconsumed_event = ?unconsumed_event, "niri event not consumed by EventStreamState");
    }

    broadcast_event(inputs, event);
}

fn broadcast_event(inputs: &DispatcherInputs, event: Event) {
    let _ = inputs.public_event_tx.send(event);
}
