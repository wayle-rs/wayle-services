//! The [`MangoService`] type: reactive compositor state plus every public
//! method for reading, watching, and commanding Mango.

use std::sync::Arc;

use derive_more::Debug;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use wayle_core::Property;
use wayle_traits::ServiceMonitoring;

use crate::{
    core::{Client, Monitor},
    error::Result,
    ipc::MangoCommandClient,
    types::{ClientId, FocusedClient, TagId},
};

/// Reactive bindings to the MangoWM compositor. See [crate-level docs](crate).
///
/// All public fields are [`Property`] values that update as Mango pushes new
/// frames. Every method either reads a cached snapshot or sends a one-shot
/// request over the command socket.
#[derive(Debug)]
pub struct MangoService {
    #[debug(skip)]
    pub(crate) cancellation_token: CancellationToken,
    #[debug(skip)]
    pub(crate) command_client: Arc<MangoCommandClient>,

    /// All monitors, in Mango's order. Rebuilt on every change.
    pub monitors: Property<Vec<Monitor>>,

    /// All clients (windows) and the tags they occupy. Rebuilt on every change.
    pub clients: Property<Vec<Client>>,

    /// The focused client on the active monitor.
    ///
    /// `None` when no client holds focus, or before the first frame arrives.
    pub focused_client: Property<Option<FocusedClient>>,

    /// The active XKB keyboard layout name.
    ///
    /// Global across monitors. `None` before the first frame arrives.
    pub keyboard_layout: Property<Option<String>>,

    /// The active key mode, for example `default`.
    ///
    /// Global across monitors. `None` before the first frame arrives.
    pub keymode: Property<Option<String>>,
}

impl MangoService {
    /// Connects to Mango, subscribes to the watch stream, and returns a ready
    /// service.
    ///
    /// # Errors
    ///
    /// - [`Error::MangoNotRunning`](crate::Error::MangoNotRunning) if
    ///   `$MANGO_INSTANCE_SIGNATURE` is unset.
    /// - [`Error::IpcConnectionFailed`](crate::Error::IpcConnectionFailed) if
    ///   the watch socket fails to connect.
    #[instrument(err)]
    pub async fn new() -> Result<Arc<Self>> {
        let cancellation_token = CancellationToken::new();
        let command_client = Arc::new(MangoCommandClient::connect()?);

        let service = Arc::new(Self {
            cancellation_token,
            command_client,
            monitors: Property::new(Vec::new()),
            clients: Property::new(Vec::new()),
            focused_client: Property::new(None),
            keyboard_layout: Property::new(None),
            keymode: Property::new(None),
        });

        service.start_monitoring().await?;
        Ok(service)
    }

    /// Looks up a monitor by connector name in the current snapshot.
    ///
    /// Returns `None` when the name is not present.
    pub fn monitor(&self, name: &str) -> Option<Monitor> {
        self.monitors
            .get()
            .into_iter()
            .find(|monitor| monitor.name == name)
    }

    /// Returns the Mango version string reported over IPC.
    ///
    /// # Errors
    ///
    /// Surfaces any transport error, and
    /// [`Error::UnexpectedResponse`](crate::Error::UnexpectedResponse) if the
    /// reply has no version field.
    #[instrument(skip(self), err)]
    pub async fn version(&self) -> Result<String> {
        self.command_client.query_version().await
    }

    /// Sends a raw `dispatch` command, for example `view,3` or
    /// `togglefloating`.
    ///
    /// # Errors
    ///
    /// Surfaces any transport error, and
    /// [`Error::MangoRejected`](crate::Error::MangoRejected) when Mango does not
    /// recognize the command.
    #[instrument(skip(self), fields(command = %command), err)]
    pub async fn dispatch(&self, command: &str) -> Result<()> {
        self.command_client.dispatch(command).await
    }

    /// Switches the active monitor to the given tag.
    ///
    /// # Errors
    /// See [`MangoService::dispatch`].
    pub async fn view_tag(&self, tag: TagId) -> Result<()> {
        self.dispatch(&format!("view,{tag}")).await
    }

    /// Switches the active monitor to the tag to the left of the current one.
    ///
    /// # Errors
    /// See [`MangoService::dispatch`].
    pub async fn view_left(&self) -> Result<()> {
        self.dispatch("viewtoleft").await
    }

    /// Switches the active monitor to the tag to the right of the current one.
    ///
    /// # Errors
    /// See [`MangoService::dispatch`].
    pub async fn view_right(&self) -> Result<()> {
        self.dispatch("viewtoright").await
    }

    /// Focuses the client with the given id.
    ///
    /// # Errors
    /// See [`MangoService::dispatch`].
    pub async fn focus_window(&self, id: ClientId) -> Result<()> {
        self.dispatch(&format!("focusid client,{id}")).await
    }
}

impl Drop for MangoService {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
    }
}
