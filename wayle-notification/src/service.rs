use std::sync::Arc;

use derive_more::Debug;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{instrument, warn};
use wayle_core::Property;
use zbus::Connection;

use crate::{
    backends::portal::PortalNotificationsDaemon,
    builder::NotificationServiceBuilder,
    core::{notification::Notification, types::PORTAL_OBJECT_PATH},
    error::Error,
    events::NotificationEvent,
    persistence::NotificationStore,
    popup_timer::PopupTimerManager,
    types::ClosedReason,
};

/// Desktop notification service. See [crate-level docs](crate) for usage.
#[derive(Debug)]
pub struct NotificationService {
    #[debug(skip)]
    pub(crate) cancellation_token: CancellationToken,
    #[debug(skip)]
    pub(crate) notif_tx: broadcast::Sender<NotificationEvent>,
    #[debug(skip)]
    pub(crate) store: Option<NotificationStore>,
    #[debug(skip)]
    pub(crate) connection: Connection,

    /// All received notifications.
    pub notifications: Property<Vec<Arc<Notification>>>,
    /// Currently visible popups.
    pub popups: Property<Vec<Arc<Notification>>>,
    /// Popup display duration in milliseconds.
    pub popup_duration: Property<u32>,
    /// Do Not Disturb mode; suppresses popups when true.
    pub dnd: Property<bool>,
    /// Auto-remove expired notifications.
    pub remove_expired: Property<bool>,
    /// Glob patterns for blocking notifications by app name.
    pub blocklist: Property<Vec<String>>,
    #[debug(skip)]
    pub(crate) popup_timers: Arc<PopupTimerManager>,
}

impl NotificationService {
    /// Creates a new notification service instance.
    ///
    /// # Errors
    /// Returns error if D-Bus connection fails or service registration fails.
    #[instrument(name = "NotificationService::new", err)]
    pub async fn new() -> Result<Arc<Self>, Error> {
        Self::builder().build().await
    }

    /// Creates a builder for configuring a NotificationService.
    pub fn builder() -> NotificationServiceBuilder {
        NotificationServiceBuilder::new()
    }

    /// The `org.freedesktop.impl.portal.*` interface(s) this service serves, for building
    /// the shell's `.portal` manifest (see `wayle-portal`'s `PortalManifest`).
    pub const PORTAL_INTERFACES: &'static [&'static str] =
        &["org.freedesktop.impl.portal.Notification"];

    /// Serves `org.freedesktop.impl.portal.Notification` on `connection` at
    /// `/org/freedesktop/portal/desktop`, letting xdg-desktop-portal forward sandboxed
    /// apps' notifications into this service.
    ///
    /// `connection` is the shared portal-backend connection (the one that owns
    /// `org.freedesktop.impl.portal.desktop.<shell>`); register the interface *before* that
    /// connection requests its well-known name. Unlike the freedesktop and GTK backends —
    /// which own their own bus names and are always on — the portal backend is enabled
    /// purely by calling this, since it is a guest on a name the shell owns.
    ///
    /// # Errors
    /// Returns an error if the interface cannot be registered on `connection`.
    #[instrument(skip(self, connection), err)]
    pub async fn attach_portal(&self, connection: &Connection) -> Result<(), Error> {
        // The portal backend re-seeds its own (app_id, portal_id) → identity map from the
        // restored notifications, so a re-send replaces in place and RemoveNotification can
        // still find them across a restart.
        let daemon = PortalNotificationsDaemon::new(
            connection.clone(),
            self.notif_tx.clone(),
            self.blocklist.clone(),
            &self.notifications.get(),
        );

        connection
            .object_server()
            .at(PORTAL_OBJECT_PATH, daemon)
            .await
            .map_err(|err| {
                Error::ServiceInitializationFailed(format!(
                    "cannot register portal notification backend: {err}"
                ))
            })?;

        Ok(())
    }

    /// Dismisses several notifications at once as a single atomic batch — one store
    /// delete and one reactive list update for the whole set, rather than per id. Takes the
    /// notifications themselves (the caller holds `Arc`s; the service reads their ids), so no
    /// caller needs the raw id. Empty input is a no-op.
    pub fn dismiss_many(&self, notifications: &[Arc<Notification>]) {
        if notifications.is_empty() {
            return;
        }

        let ids = notifications.iter().map(|notif| notif.id).collect();
        if let Err(error) = self.notif_tx.send(NotificationEvent::RemoveMany(
            ids,
            ClosedReason::DismissedByUser,
        )) {
            warn!(error = %error, "cannot dismiss notifications");
        }
    }

    /// Dismisses all notifications and emits `NotificationClosed` for each.
    ///
    /// # Errors
    /// Returns error if the event channel is closed.
    #[instrument(skip(self), err)]
    pub async fn dismiss_all(&self) -> Result<(), Error> {
        self.dismiss_many(&self.notifications.get());
        Ok(())
    }

    /// Sets the Do Not Disturb mode.
    ///
    /// When enabled, new notifications will not appear as popups but will
    /// still be added to the notification list.
    pub fn set_dnd(&self, dnd: bool) {
        self.dnd.set(dnd)
    }

    /// Sets the duration for how long popup notifications are displayed.
    pub fn set_popup_duration(&self, duration: u32) {
        self.popup_duration.set(duration)
    }

    /// Replaces the blocklist patterns.
    pub fn set_blocklist(&self, patterns: Vec<String>) {
        self.blocklist.set(patterns)
    }
}

impl Drop for NotificationService {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
    }
}
