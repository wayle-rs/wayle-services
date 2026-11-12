use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{info, instrument};
use wayle_core::Property;
use zbus::{
    Connection,
    fdo::{RequestNameFlags, RequestNameReply},
};

use crate::{
    core::item::TrayItem,
    dbus::{SERVICE_NAME, SERVICE_PATH, SystemTrayDaemon},
    discovery::SystemTrayServiceDiscovery,
    error::Error,
    monitoring::spawn_host_listeners,
    proxy::status_notifier_watcher::StatusNotifierWatcherProxy,
    registrar::spawn_registrar,
    service::SystemTrayService,
    types::{TrayMode, WATCHER_BUS_NAME, WATCHER_OBJECT_PATH},
    watcher::{StatusNotifierWatcher, discovery::spawn_startup_discovery},
};

/// Builder for configuring a [`SystemTrayService`].
pub struct SystemTrayServiceBuilder {
    mode: TrayMode,
    register_daemon: bool,
}

impl SystemTrayServiceBuilder {
    /// Creates a new builder with default configuration.
    pub fn new() -> Self {
        Self {
            mode: TrayMode::Auto,
            register_daemon: false,
        }
    }

    /// Sets the operating mode for the service.
    ///
    /// - `TrayMode::Watcher` - Act as the StatusNotifierWatcher registry
    /// - `TrayMode::Host` - Act as a StatusNotifierHost consumer
    /// - `TrayMode::Auto` - Auto-detect based on name availability (default)
    pub fn mode(mut self, mode: TrayMode) -> Self {
        self.mode = mode;
        self
    }

    /// Enables the Wayle D-Bus daemon for CLI control.
    ///
    /// When enabled, the service registers at `com.wayle.SystemTray1`, allowing CLI tools
    /// to list and activate tray items.
    pub fn with_daemon(mut self) -> Self {
        self.register_daemon = true;
        self
    }

    /// Builds the [`SystemTrayService`].
    ///
    /// # Errors
    /// Returns an error if D-Bus connection or service initialization fails.
    #[instrument(skip(self), fields(mode = ?self.mode), err)]
    pub async fn build(self) -> Result<Arc<SystemTrayService>, Error> {
        let connection = Connection::session().await?;
        let cancellation_token = CancellationToken::new();
        let items: Property<Vec<Arc<TrayItem>>> = Property::new(Vec::new());

        let (is_watcher, watcher_conn) = match self.mode {
            TrayMode::Host => {
                setup_host(&connection, &items, &cancellation_token).await?;
                (false, None)
            }
            TrayMode::Watcher | TrayMode::Auto => {
                match setup_watcher(&connection, &items, &cancellation_token).await? {
                    Some(watcher_conn) => (true, Some(watcher_conn)),
                    None => {
                        if self.mode == TrayMode::Watcher {
                            return Err(Error::WatcherRegistration(format!(
                                "D-Bus name '{WATCHER_BUS_NAME}' is already owned"
                            )));
                        }
                        setup_host(&connection, &items, &cancellation_token).await?;
                        (false, None)
                    }
                }
            }
        };

        let service = Arc::new(SystemTrayService {
            cancellation_token,
            connection,
            watcher_connection: watcher_conn,
            is_watcher,
            items,
        });

        if self.register_daemon {
            register_daemon(&service).await?;
        }

        Ok(service)
    }
}

/// Attempts to become the `StatusNotifierWatcher`.
///
/// On success returns the dedicated connection owning the watcher name (kept alive by the
/// service). Returns `None` if the name is already owned. The watcher interface runs on
/// its own connection: a bus connection's object server consumes every incoming method
/// call, so isolating it keeps registration traffic off the item-call / daemon connection.
#[instrument(skip(connection, items, cancellation_token), err)]
async fn setup_watcher(
    connection: &Connection,
    items: &Property<Vec<Arc<TrayItem>>>,
    cancellation_token: &CancellationToken,
) -> Result<Option<Connection>, Error> {
    let watcher_conn = Connection::session().await?;

    let registrar_token = cancellation_token.child_token();
    let registrar = spawn_registrar(
        connection.clone(),
        Some(watcher_conn.clone()),
        items.clone(),
        registrar_token.clone(),
    );

    let watcher = StatusNotifierWatcher::new(registrar.clone());
    watcher_conn
        .object_server()
        .at(WATCHER_OBJECT_PATH, watcher)
        .await?;

    // `DoNotQueue` so a contended name yields `Exists` (not `InQueue`): plain
    // `request_name` uses empty flags, which would silently queue us to steal the name
    // later and report success — never falling back to host. We match the reply
    // explicitly rather than on Ok/Err, since `request_name` maps `InQueue` to `Ok`.
    let acquired = matches!(
        watcher_conn
            .request_name_with_flags(WATCHER_BUS_NAME, RequestNameFlags::DoNotQueue.into())
            .await,
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner)
    );

    if acquired {
        info!("Operating as StatusNotifierWatcher");
        spawn_startup_discovery(
            connection.clone(),
            registrar,
            own_unique_names(connection, &watcher_conn),
        );
        Ok(Some(watcher_conn))
    } else {
        // Someone else owns the name (or the request failed); abandon this attempt.
        registrar_token.cancel();
        let _ = watcher_conn
            .object_server()
            .remove::<StatusNotifierWatcher, _>(WATCHER_OBJECT_PATH)
            .await;
        info!("StatusNotifierWatcher already present; connecting as host");
        Ok(None)
    }
}

/// Wires up host mode: register with the external watcher, seed current items, and forward
/// its future registration signals into our registrar.
#[instrument(skip(connection, items, cancellation_token), err)]
async fn setup_host(
    connection: &Connection,
    items: &Property<Vec<Arc<TrayItem>>>,
    cancellation_token: &CancellationToken,
) -> Result<(), Error> {
    // Ensure a watcher actually exists before we try to consume from it.
    StatusNotifierWatcherProxy::new(connection).await.map_err(|_| {
        Error::ServiceInitialization("no StatusNotifierWatcher available to connect to".to_string())
    })?;

    let host_name = connection
        .unique_name()
        .map(|name| name.as_str().to_string())
        .unwrap_or_default();
    SystemTrayServiceDiscovery::register_as_host(connection, &host_name).await?;

    let registrar = spawn_registrar(
        connection.clone(),
        None,
        items.clone(),
        cancellation_token.child_token(),
    );

    // Start listening before seeding, so no registration that races the seed is lost.
    spawn_host_listeners(connection, registrar.clone(), cancellation_token.child_token()).await?;
    SystemTrayServiceDiscovery::seed_from_watcher(connection, &registrar).await?;

    Ok(())
}

async fn register_daemon(service: &Arc<SystemTrayService>) -> Result<(), Error> {
    let daemon = SystemTrayDaemon {
        service: Arc::clone(service),
    };

    service
        .connection
        .object_server()
        .at(SERVICE_PATH, daemon)
        .await
        .map_err(|err| {
            Error::ServiceInitialization(format!(
                "cannot register D-Bus object at '{SERVICE_PATH}': {err}"
            ))
        })?;

    service
        .connection
        .request_name(SERVICE_NAME)
        .await
        .map_err(|err| {
            Error::ServiceInitialization(format!("cannot acquire D-Bus name '{SERVICE_NAME}': {err}"))
        })?;

    info!("System tray service registered at {SERVICE_NAME}");
    Ok(())
}

/// Unique names of our own connections, excluded from startup discovery.
fn own_unique_names(connection: &Connection, watcher_conn: &Connection) -> Vec<String> {
    [connection.unique_name(), watcher_conn.unique_name()]
        .into_iter()
        .flatten()
        .map(|name| name.as_str().to_string())
        .collect()
}

impl Default for SystemTrayServiceBuilder {
    fn default() -> Self {
        Self::new()
    }
}
