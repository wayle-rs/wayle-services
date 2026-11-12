use std::{collections::HashSet, sync::Arc};

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use wayle_core::Property;
use wayle_traits::ServiceMonitoring;
use zbus::{Connection, object_server::Interface};

use crate::{
    backends::{freedesktop::FdoNotificationDaemon, gtk::GtkNotificationsDaemon},
    core::{
        notification::Notification,
        types::{Alert, Image},
    },
    error::Error,
    events::NotificationEvent,
    image_cache,
    persistence::{NotificationStore, StoredNotification},
    popup_timer::PopupTimerManager,
    service::NotificationService,
    types::dbus::{
        GTK_SERVICE_NAME, GTK_SERVICE_PATH, SERVICE_NAME, SERVICE_PATH, WAYLE_SERVICE_NAME,
        WAYLE_SERVICE_PATH,
    },
    wayle_daemon::WayleDaemon,
};

const EVENT_CHANNEL_CAPACITY: usize = 10_000;

/// Builder for configuring and creating a NotificationService instance.
///
/// Allows customization of popup duration, do-not-disturb mode, and
/// automatic removal of expired notifications.
#[derive(Debug)]
pub struct NotificationServiceBuilder {
    popup_duration: Property<u32>,
    dnd: Property<bool>,
    remove_expired: Property<bool>,
    blocklist: Property<Vec<String>>,
    register_wayle_daemon: bool,
}

impl Default for NotificationServiceBuilder {
    fn default() -> Self {
        Self {
            popup_duration: Property::new(5000),
            dnd: Property::new(false),
            remove_expired: Property::new(true),
            blocklist: Property::new(vec![]),
            register_wayle_daemon: false,
        }
    }
}

impl NotificationServiceBuilder {
    /// Creates a new NotificationServiceBuilder with default values.
    pub fn new() -> Self {
        Self::default()
    }
    /// Sets the duration in milliseconds for how long popups should be displayed.
    pub fn popup_duration(self, duration: u32) -> Self {
        self.popup_duration.set(duration);
        self
    }

    /// Configures the Do Not Disturb mode.
    ///
    /// When enabled, new notifications won't appear as popups but will still
    /// be added to the notification list.
    pub fn dnd(self, dnd: bool) -> Self {
        self.dnd.set(dnd);
        self
    }

    /// Sets whether to automatically remove expired notifications.
    pub fn remove_expired(self, remove: bool) -> Self {
        self.remove_expired.set(remove);
        self
    }

    /// Sets glob patterns for blocking notifications by app name.
    ///
    /// Notifications from matching apps are silently dropped.
    /// Patterns support `*` and `?` wildcards.
    pub fn blocklist(self, patterns: Property<Vec<String>>) -> Self {
        Self {
            blocklist: patterns,
            ..self
        }
    }

    /// Enables the Wayle D-Bus daemon for CLI control.
    ///
    /// When enabled, the service registers at `com.wayle.Notifications1`,
    /// allowing CLI tools to control notifications (dismiss, toggle DND, etc.).
    pub fn with_daemon(mut self) -> Self {
        self.register_wayle_daemon = true;
        self
    }

    /// Builds and initializes the NotificationService.
    ///
    /// Establishes a D-Bus connection, registers the notification daemon,
    /// restores persisted notifications, and starts monitoring for events.
    ///
    /// # Errors
    /// Returns error if D-Bus connection fails, service registration fails,
    /// or monitoring cannot be started.
    pub async fn build(self) -> Result<Arc<NotificationService>, Error> {
        let connection = Connection::session().await.map_err(|err| {
            Error::ServiceInitializationFailed(format!("D-Bus connection failed: {err}"))
        })?;
        let (notif_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let cancellation_token = CancellationToken::new();

        let store = init_store();

        // The current session bus GUID stamps each freedesktop notification's dispatch, so a
        // restart can tell which restored notifications belong to this session (wire ids and
        // owners still live) versus a prior one (history only). Empty if it can't be read,
        // which no persisted session id matches, so those owners are treated as stale.
        let session_id = match zbus::fdo::DBusProxy::new(&connection).await {
            Ok(proxy) => match proxy.get_id().await {
                Ok(guid) => guid.to_string(),
                Err(err) => {
                    warn!(error = %err, "cannot read session bus GUID; treating persisted owners as stale");
                    String::new()
                }
            },
            Err(err) => {
                warn!(error = %err, "cannot create DBus proxy; treating persisted owners as stale");
                String::new()
            }
        };

        let stored_notifications =
            load_stored_notifications(&store, self.remove_expired.get(), &connection, &notif_tx);

        // Drop cached image/sound blobs no longer referenced by any restored notification,
        // bounding the content-addressed cache (a dropped blob is re-created on demand).
        let mut referenced = HashSet::new();
        for notif in &stored_notifications {
            if let Some(Image::Path(path)) = notif.view.get().origin.icon {
                referenced.insert(path);
            }
            if let Some(Image::Path(path)) = notif.view.get().image {
                referenced.insert(path);
            }
            if let Alert::File(path) = notif.view.get().alert {
                referenced.insert(path);
            }
        }
        image_cache::prune(&referenced);

        // The freedesktop backend re-establishes its own wire-id space (coalescing slots, wire
        // counter, stale-session owner clearing) from the restored notifications.
        let freedesktop_daemon = FdoNotificationDaemon::new(
            connection.clone(),
            notif_tx.clone(),
            self.blocklist.clone(),
            session_id,
            &stored_notifications,
        );

        register_dbus_object(&connection, SERVICE_PATH, freedesktop_daemon).await?;
        register_dbus_name(&connection, SERVICE_NAME).await?;
        info!("Notification daemon registered at {SERVICE_NAME}");

        // GTK notification bridge (`org.gtk.Notifications`) so GApplication/GNotification
        // apps route here and get persistent, cold-launchable actions. Best-effort: if the
        // name can't be acquired (e.g. another daemon owns it), log and carry on rather
        // than failing the whole service.
        let gtk_daemon = GtkNotificationsDaemon::new(
            connection.clone(),
            notif_tx.clone(),
            self.blocklist.clone(),
            &stored_notifications,
        );
        register_dbus_object(&connection, GTK_SERVICE_PATH, gtk_daemon).await?;
        match register_dbus_name(&connection, GTK_SERVICE_NAME).await {
            Ok(()) => info!("GTK notification bridge registered at {GTK_SERVICE_NAME}"),
            Err(err) => warn!(error = %err, "cannot acquire {GTK_SERVICE_NAME}; GTK notifications disabled"),
        }

        let popups = Property::new(vec![]);
        let popup_timers = Arc::new(PopupTimerManager::new(popups.clone()));

        let service = Arc::new(NotificationService {
            cancellation_token,
            notif_tx,
            store,
            connection: connection.clone(),
            notifications: Property::new(stored_notifications),
            popups,
            popup_duration: self.popup_duration,
            dnd: self.dnd,
            remove_expired: self.remove_expired,
            blocklist: self.blocklist,
            popup_timers,
        });

        service.start_monitoring().await?;

        if self.register_wayle_daemon {
            let wayle_daemon = WayleDaemon {
                service: Arc::clone(&service),
            };
            register_dbus_object(&connection, WAYLE_SERVICE_PATH, wayle_daemon).await?;
            register_dbus_name(&connection, WAYLE_SERVICE_NAME).await?;
            info!("Wayle notification extensions registered at {WAYLE_SERVICE_NAME}");
        }

        Ok(service)
    }
}

fn init_store() -> Option<NotificationStore> {
    match NotificationStore::new() {
        Ok(store) => {
            info!("Notification persistence enabled");
            Some(store)
        }
        Err(e) => {
            error!(error = %e, "cannot initialize notification store");
            error!("notifications will not persist across restarts");
            None
        }
    }
}

fn load_stored_notifications(
    store: &Option<NotificationStore>,
    remove_expired: bool,
    connection: &Connection,
    notif_tx: &broadcast::Sender<NotificationEvent>,
) -> Vec<Arc<Notification>> {
    store
        .as_ref()
        .and_then(|store| store.load_all(remove_expired).ok())
        .map(|stored| {
            stored
                .into_iter()
                .map(|notification| {
                    stored_to_notification(notification, connection.clone(), notif_tx.clone())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn stored_to_notification(
    stored: StoredNotification,
    connection: Connection,
    notif_tx: broadcast::Sender<NotificationEvent>,
) -> Arc<Notification> {
    Arc::new(Notification::from_stored(stored, connection, notif_tx))
}

async fn register_dbus_object<T: Interface>(
    connection: &Connection,
    path: &str,
    object: T,
) -> Result<(), Error> {
    connection
        .object_server()
        .at(path, object)
        .await
        .map_err(|err| {
            Error::ServiceInitializationFailed(format!(
                "cannot register D-Bus object at '{path}': {err}"
            ))
        })?;
    Ok(())
}

async fn register_dbus_name(connection: &Connection, name: &str) -> Result<(), Error> {
    connection.request_name(name).await.map_err(|err| {
        Error::ServiceInitializationFailed(format!("cannot acquire D-Bus name '{name}': {err}"))
    })
}
