//! The [`NiriService`] type: reactive compositor state plus every public
//! method for reading, watching, and commanding niri.

use std::{collections::HashMap, sync::Arc};

use derive_more::Debug;
use futures::Stream;
use niri_ipc::{
    Action, Event, KeyboardLayouts, LayoutSwitchTarget, Request, Response, WorkspaceReferenceArg,
};
use tokio::sync::broadcast;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use wayle_core::Property;
use wayle_traits::ServiceMonitoring;

use crate::{
    constants::EVENT_CHANNEL_CAPACITY,
    core::{Window, Workspace},
    error::{Error, Result},
    ipc::NiriCommandClient,
};

/// Reactive bindings to the niri compositor. See [crate-level docs](crate).
///
/// All public fields are [`Property`] values that update as niri emits events.
/// Every method on this struct either reads a cached snapshot or calls the
/// compositor over the persistent command socket.
#[derive(Debug)]
pub struct NiriService {
    #[debug(skip)]
    pub(crate) cancellation_token: CancellationToken,
    #[debug(skip)]
    pub(crate) command_client: Arc<NiriCommandClient>,
    #[debug(skip)]
    pub(crate) inbound_event_tx: broadcast::Sender<Event>,
    #[debug(skip)]
    pub(crate) public_event_tx: broadcast::Sender<Event>,

    /// All workspaces keyed by id. Iteration order is undefined; sort by
    /// `(output, idx)` at the call site when ordered display is needed.
    pub workspaces: Property<HashMap<u64, Arc<Workspace>>>,

    /// All open toplevel windows keyed by id.
    pub windows: Property<HashMap<u64, Arc<Window>>>,

    /// Configured keyboard layouts and the active index.
    ///
    /// `None` until niri sends the first [`Event::KeyboardLayoutsChanged`].
    pub keyboard_layouts: Property<Option<KeyboardLayouts>>,

    /// Id of the currently focused window, or `None` while focus is held by a
    /// layer-shell surface.
    pub focused_window_id: Property<Option<u64>>,

    /// Whether the overview is visible.
    pub overview_open: Property<bool>,

    /// Whether niri's most recent configuration load failed. Reset to `false`
    /// once a later reload succeeds.
    pub config_failed: Property<bool>,
}

impl NiriService {
    /// Connects to niri, subscribes to the event stream, and returns a ready
    /// service.
    ///
    /// # Errors
    ///
    /// - [`Error::NiriNotRunning`] if `$NIRI_SOCKET` is unset.
    /// - [`Error::IpcConnectionFailed`] if either the command or event-stream
    ///   socket fails to connect.
    #[instrument(err)]
    pub async fn new() -> Result<Arc<Self>> {
        let cancellation_token = CancellationToken::new();
        let command_client = NiriCommandClient::connect().await?;

        let (inbound_event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let (public_event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

        let service = Arc::new(Self {
            cancellation_token,
            command_client: Arc::new(command_client),
            inbound_event_tx,
            public_event_tx,
            workspaces: Property::new(HashMap::new()),
            windows: Property::new(HashMap::new()),
            keyboard_layouts: Property::new(None),
            focused_window_id: Property::new(None),
            overview_open: Property::new(false),
            config_failed: Property::new(false),
        });

        service.start_monitoring().await?;
        Ok(service)
    }

    /// Looks up a window by id in the current [`NiriService::windows`] snapshot.
    ///
    /// Returns `None` when the id is not present, including the brief
    /// transient window where niri's event stream references a just-deleted
    /// entity.
    pub fn window(&self, id: u64) -> Option<Arc<Window>> {
        self.windows.get().get(&id).cloned()
    }

    /// Looks up a workspace by id in the current [`NiriService::workspaces`]
    /// snapshot.
    ///
    /// Returns `None` when the id is not present.
    pub fn workspace(&self, id: u64) -> Option<Arc<Workspace>> {
        self.workspaces.get().get(&id).cloned()
    }

    /// Returns a stream of every [`Event`] niri emits, in order.
    ///
    /// Subscribers that fall more than the internal channel capacity behind
    /// observe [`tokio::sync::broadcast::error::RecvError::Lagged`]; those are
    /// filtered out here so the caller sees a plain `Stream<Item = Event>`.
    pub fn events(&self) -> impl Stream<Item = Event> + Send + 'static {
        let receiver = self.public_event_tx.subscribe();
        BroadcastStream::new(receiver).filter_map(|received| received.ok())
    }

    /// Returns the niri version string reported over IPC.
    ///
    /// # Errors
    ///
    /// Surfaces any transport error, and [`Error::UnexpectedResponse`] if niri
    /// replies with the wrong [`Response`] variant.
    #[instrument(skip(self), err)]
    pub async fn version(&self) -> Result<String> {
        self.command_client.query_version().await
    }

    /// Dispatches a typed [`Action`] to the compositor.
    ///
    /// # Errors
    ///
    /// Surfaces any transport error, and [`Error::NiriRejected`] when niri
    /// replies with `Reply::Err`.
    #[instrument(skip(self), fields(action = ?action), err)]
    pub async fn dispatch_action(&self, action: Action) -> Result<()> {
        match self.command_client.request(Request::Action(action)).await? {
            Response::Handled => Ok(()),
            _ => Err(Error::UnexpectedResponse { request: "action" }),
        }
    }

    /// Focuses a window by id.
    ///
    /// # Errors
    /// See [`NiriService::dispatch_action`].
    pub async fn focus_window(&self, id: u64) -> Result<()> {
        self.dispatch_action(Action::FocusWindow { id }).await
    }

    /// Closes a window. `None` closes the focused window.
    ///
    /// # Errors
    /// See [`NiriService::dispatch_action`].
    pub async fn close_window(&self, id: Option<u64>) -> Result<()> {
        self.dispatch_action(Action::CloseWindow { id }).await
    }

    /// Focuses a workspace by id, index, or name.
    ///
    /// # Errors
    /// See [`NiriService::dispatch_action`].
    pub async fn focus_workspace(&self, reference: WorkspaceReferenceArg) -> Result<()> {
        self.dispatch_action(Action::FocusWorkspace { reference })
            .await
    }

    /// Focuses a column by 1-based index within the active workspace.
    ///
    /// # Errors
    /// See [`NiriService::dispatch_action`].
    pub async fn focus_column(&self, column_index: usize) -> Result<()> {
        self.dispatch_action(Action::FocusColumn {
            index: column_index,
        })
        .await
    }

    /// Spawns a command. The first element is the executable, the rest are
    /// arguments. For shell interpretation use [`Action::SpawnSh`] via
    /// [`NiriService::dispatch_action`].
    ///
    /// # Errors
    /// See [`NiriService::dispatch_action`].
    pub async fn spawn(&self, command: Vec<String>) -> Result<()> {
        self.dispatch_action(Action::Spawn { command }).await
    }

    /// Switches keyboard layout to the next/previous one or to an explicit
    /// index.
    ///
    /// # Errors
    /// See [`NiriService::dispatch_action`].
    pub async fn switch_layout(&self, layout: LayoutSwitchTarget) -> Result<()> {
        self.dispatch_action(Action::SwitchLayout { layout }).await
    }
}

impl Drop for NiriService {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
    }
}
