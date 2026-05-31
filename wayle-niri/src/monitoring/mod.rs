//! Wires the event-stream socket into the reactive [`Property`] fields.
//!
//! [`ipc::subscribe_events`](crate::ipc::subscribe_events) owns the socket
//! and pushes each [`Event`] into the inbound broadcast channel. The
//! [`dispatcher`] task drains a pre-subscribed receiver, runs
//! [`precondition`] checks, feeds niri's
//! [`EventStreamState`](niri_ipc::state::EventStreamState), refreshes the
//! relevant [`Property`] fields via [`property_refresh`], and re-broadcasts
//! the event on `public_event_tx` so external [`NiriService::events`]
//! subscribers see events only after Properties have been updated.

mod dispatcher;
mod precondition;
mod property_refresh;

use std::{collections::HashMap, sync::Arc};

use derive_more::Debug;
use niri_ipc::{Event, KeyboardLayouts};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use wayle_core::Property;
use wayle_traits::ServiceMonitoring;

use crate::{
    core::{Window, Workspace},
    error::Error,
    ipc::subscribe_events,
    service::NiriService,
};

/// Everything the dispatcher task needs to do its job.
///
/// Built once in [`NiriService::start_monitoring`] and moved into the task.
/// `inbound_event_rx` is subscribed by the caller *before* the read task
/// starts, so the dispatcher cannot miss the first burst of snapshot events.
#[derive(Debug)]
pub(crate) struct DispatcherInputs {
    #[debug(skip)]
    pub(crate) inbound_event_rx: broadcast::Receiver<Event>,
    #[debug(skip)]
    pub(crate) public_event_tx: broadcast::Sender<Event>,
    #[debug(skip)]
    pub(crate) cancellation_token: CancellationToken,

    pub(crate) workspaces: Property<HashMap<u64, Arc<Workspace>>>,
    pub(crate) windows: Property<HashMap<u64, Arc<Window>>>,
    pub(crate) keyboard_layouts: Property<Option<KeyboardLayouts>>,
    pub(crate) focused_window_id: Property<Option<u64>>,
    pub(crate) overview_open: Property<bool>,
    pub(crate) config_failed: Property<bool>,
}

impl ServiceMonitoring for NiriService {
    type Error = Error;

    #[instrument(skip(self), err)]
    async fn start_monitoring(&self) -> Result<(), Error> {
        let inbound_event_rx = self.inbound_event_tx.subscribe();

        subscribe_events(
            self.inbound_event_tx.clone(),
            self.cancellation_token.clone(),
        )
        .await?;

        dispatcher::spawn(DispatcherInputs {
            inbound_event_rx,
            public_event_tx: self.public_event_tx.clone(),
            cancellation_token: self.cancellation_token.clone(),
            workspaces: self.workspaces.clone(),
            windows: self.windows.clone(),
            keyboard_layouts: self.keyboard_layouts.clone(),
            focused_window_id: self.focused_window_id.clone(),
            overview_open: self.overview_open.clone(),
            config_failed: self.config_failed.clone(),
        });

        Ok(())
    }
}
