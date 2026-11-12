use std::{cmp::PartialEq, collections::HashMap};

use chrono::{DateTime, Utc};
use derive_more::Debug;
use tokio::sync::broadcast;
use tracing::instrument;
use wayle_core::Property;
use zbus::{Connection, zvariant::OwnedValue};

use super::{
    backend::DispatchCtx,
    types::{
        Action, ActionId, Actions, Alert, Classification, Content, Image, InvokeSource, Lifecycle,
        NotificationId, NotificationProps, NotificationSource, Origin, Presentation,
    },
};
use crate::{
    error::Error,
    events::NotificationEvent,
    persistence::{StoredNotification, source_from_stored},
    types::ClosedReason,
};

/// An immutable snapshot of everything a shell renders for a notification — all display facets
/// in one value. Held in a single [`Property`] on [`Notification`] so a `replaces`/coalesce
/// update refreshes every facet **atomically** (one change notification, no torn read where a
/// widget sees the new body but the old priority), rather than as eight separate updates.
#[derive(Clone, Debug, PartialEq)]
pub struct NotificationView {
    /// The sending application (name / desktop id / header icon).
    pub origin: Origin,
    /// The renderable text (summary + optional body with markup mode).
    pub content: Content,
    /// Large content image (album art / screenshot / photo). Falls back to `origin.icon`.
    pub image: Option<Image>,
    /// The interaction surface (default/body action + buttons + icon-vs-text mode).
    pub actions: Actions,
    /// Audio-feedback intent.
    pub alert: Alert,
    /// Priority, category, and the transient/resident/coalescing axes.
    pub classification: Classification,
    /// Timeout policy + manual-dismiss lock.
    pub lifecycle: Lifecycle,
    /// Banner-vs-tray routing, lock-screen privacy, re-alert-on-replace.
    pub presentation: Presentation,
    /// When the daemon received it (used for the header time + history sort).
    pub received: DateTime<Utc>,
    /// Why it left the screen; `None` while live.
    pub close_reason: Option<ClosedReason>,
}

/// A desktop notification, fully abstracted over the delivering protocol.
///
/// A shell consuming this never learns whether it arrived via `org.freedesktop.Notifications`,
/// `org.gtk.Notifications`, or the XDG portal backend: the [`view`](Self::view) is pure
/// presentation, and every action reaches the originating app through [`invoke`](Self::invoke) /
/// [`dismiss`](Self::dismiss), which route internally on the private `dispatch`.
///
/// Each notification is allocated a session-unique [`NotificationId`]. A `replaces` update
/// mutates the [`view`](Self::view) in place so the shell's bound card keeps its `Arc` identity.
#[derive(Clone, Debug)]
pub struct Notification {
    #[debug(skip)]
    zbus_connection: Connection,
    #[debug(skip)]
    notif_tx: broadcast::Sender<NotificationEvent>,

    /// Session-unique server-allocated id — the notification's identity. Crate-internal: a
    /// consumer references a notification by its `Arc`/`PartialEq`, not this raw id. Kept in
    /// `Debug` output (not skipped) because it is useful in logs.
    pub(crate) id: NotificationId,

    /// Every display facet as one atomically-updated snapshot. Read `notif.view.get()`, and
    /// subscribe to `notif.view` for changes.
    pub view: Property<NotificationView>,

    /// How this notification's actions reach the originating app (and, for freedesktop, the
    /// owning connection's unique name in [`FreedesktopDispatch::owner`]). Deliberately not `pub`
    /// and carrying no protocol name: this is what keeps the shell protocol-agnostic.
    #[debug(skip)]
    pub(crate) dispatch: Property<NotificationSource>,
}

impl PartialEq for Notification {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

/// Opens a URI with the desktop's default handler via the XDG portal `OpenURI`, over an existing
/// connection with an empty parent window (`""`), so it never touches a Wayland surface. Shared
/// by the freedesktop `x-kde-urls` body-click open and [`Notification::open_uri`] (body links).
#[instrument(skip(connection), err)]
pub(crate) async fn open_uri_via_portal(connection: &Connection, uri: &str) -> Result<(), Error> {
    let options: HashMap<String, OwnedValue> = HashMap::new();
    connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.OpenURI"),
            "OpenURI",
            &("", uri, options),
        )
        .await?;
    Ok(())
}

impl Notification {
    pub(crate) fn new(
        props: NotificationProps,
        connection: Connection,
        notif_tx: broadcast::Sender<NotificationEvent>,
    ) -> Self {
        Self::from_props(props, connection, notif_tx)
    }

    /// Rebuilds a notification from its persisted form, wrapping the stored facets directly
    /// (they were already translated at ingest, so there is nothing to re-derive) and
    /// restoring the dispatch. The owner is restored as-is; the builder clears it afterward for
    /// freedesktop notifications from a prior session bus (whose owner must not be directed to).
    pub(crate) fn from_stored(
        stored: StoredNotification,
        connection: Connection,
        notif_tx: broadcast::Sender<NotificationEvent>,
    ) -> Notification {
        let received =
            DateTime::<Utc>::from_timestamp_millis(stored.received_ms).unwrap_or_else(Utc::now);
        Self {
            zbus_connection: connection,
            notif_tx,
            id: stored.id,
            view: Property::new(NotificationView {
                origin: stored.origin,
                content: stored.content,
                image: stored.image,
                actions: stored.actions,
                alert: stored.alert,
                classification: stored.classification,
                lifecycle: stored.lifecycle,
                presentation: stored.presentation,
                received,
                close_reason: stored.close_reason,
            }),
            dispatch: Property::new(source_from_stored(stored.dispatch)),
        }
    }

    /// Dismisses the notification (user-initiated close): removes it from history and lets the
    /// pipeline emit the appropriate closed signal to the owning app.
    #[instrument(skip(self), fields(notification_id = %self.id))]
    pub fn dismiss(&self) {
        let _ = self.notif_tx.send(NotificationEvent::Remove(
            self.id,
            ClosedReason::DismissedByUser,
        ));
    }

    /// Alias for [`dismiss`](Self::dismiss); closes the notification.
    pub fn close(&self) {
        self.dismiss();
    }

    /// Removes this notification from the visible popups only — it stays in history — and
    /// cancels its popup timer. The service's popup list/timers react to the emitted event.
    pub fn dismiss_popup(&self) {
        let _ = self.notif_tx.send(NotificationEvent::DismissPopup(self.id));
    }

    /// Pauses this notification's popup auto-dismiss timer (e.g. while the pointer hovers it),
    /// resumed by [`release_popup`](Self::release_popup).
    pub fn inhibit_popup(&self) {
        let _ = self.notif_tx.send(NotificationEvent::InhibitPopup(self.id));
    }

    /// Resumes this notification's popup auto-dismiss timer after
    /// [`inhibit_popup`](Self::inhibit_popup).
    pub fn release_popup(&self) {
        let _ = self.notif_tx.send(NotificationEvent::ReleasePopup(self.id));
    }

    /// Invokes the body-click / default action.
    ///
    /// `activation_token` is an `xdg-activation-v1` token the shell mints at click time so the
    /// app may raise its window; pass `None` if unavailable.
    ///
    /// # Errors
    /// Returns an error if dispatching the action to the app fails.
    pub async fn activate_default(
        &self,
        source: InvokeSource,
        activation_token: Option<&str>,
    ) -> Result<(), Error> {
        // Only a notification with a body-click action does anything on a body click. Without one
        // (`actions.default` is `None`), do nothing rather than fire a spurious `ActionInvoked` /
        // `Activate` to an app that never declared a default action — mirroring the backend's own
        // "no matching action ⇒ no-op". Keeps "you have a default to activate" a single invariant.
        if self.view.get().actions.default.is_none() {
            return Ok(());
        }
        self.dispatch_action(&ActionId::default_action(), activation_token)
            .await?;
        self.dismiss_after_action(source);
        Ok(())
    }

    /// Invokes an action button on the notification, routing to the originating backend
    /// internally, then dismissing per `source` (unless resident).
    ///
    /// The shell passes the whole [`Action`] (from [`Actions::buttons`]), where the click came
    /// from, and an `xdg-activation-v1` token it minted (so the app may raise); the dispatch key
    /// and the dismissal policy both stay inside the crate.
    ///
    /// # Errors
    /// Returns an error if dispatching the action fails (the notification is then *not*
    /// dismissed, so a failed action leaves it in place).
    pub async fn invoke(
        &self,
        action: &Action,
        source: InvokeSource,
        activation_token: Option<&str>,
    ) -> Result<(), Error> {
        self.dispatch_action(&action.id, activation_token).await?;
        self.dismiss_after_action(source);
        Ok(())
    }

    /// Dismisses the notification after a successful action, according to where it was invoked
    /// from and whether it is resident. Resident notifications (e.g. media controls) survive
    /// every activation; otherwise a history activation or a "remove" popup close removes it
    /// from history, while a "keep" popup close only dismisses the banner.
    fn dismiss_after_action(&self, source: InvokeSource) {
        if self.view.get().classification.resident {
            return;
        }
        let event = match source {
            InvokeSource::History
            | InvokeSource::Popup {
                remove_from_history: true,
            } => NotificationEvent::Remove(self.id, ClosedReason::Closed),
            InvokeSource::Popup {
                remove_from_history: false,
            } => NotificationEvent::DismissPopup(self.id),
        };
        let _ = self.notif_tx.send(event);
    }

    /// Forwards an action id to the originating backend (no dismissal — that is
    /// [`dismiss_after_action`](Self::dismiss_after_action)'s job) via the common
    /// [`Backend`] trait, so the core never matches on the protocol. Private: reached via
    /// [`invoke`](Self::invoke) (a button) or [`activate_default`](Self::activate_default) (the
    /// body click), so the reserved default key is never exposed to callers.
    ///
    /// # Errors
    /// Returns an error if dispatching the action fails.
    #[instrument(skip(self), fields(notification_id = %self.id, action = %action.as_str()), err)]
    async fn dispatch_action(
        &self,
        action: &ActionId,
        activation_token: Option<&str>,
    ) -> Result<(), Error> {
        // Bind the cloned facets so they outlive the borrowed `DispatchCtx` across the await.
        let actions = self.view.get().actions;
        let dispatch = self.dispatch.get();
        let ctx = DispatchCtx {
            connection: &self.zbus_connection,
            urls: &actions.urls,
            activation_token,
        };
        dispatch.backend().dispatch_action(&ctx, action).await
    }

    /// Sends an inline text reply, for a notification whose [`actions.reply`](Actions::reply)
    /// is `Some`. Routes to the backend internally: freedesktop emits `NotificationReplied` to
    /// the owner; portal emits the `im.reply-with-text` `ActionInvoked` with the text appended.
    /// GNotification cannot express inline reply, so this is a no-op there. Dismisses afterward
    /// per `source` (unless resident), like [`invoke`](Self::invoke).
    ///
    /// # Errors
    /// Returns an error if delivering the reply fails (the notification is then not dismissed).
    #[instrument(skip(self, text), fields(notification_id = %self.id), err)]
    pub async fn reply(&self, text: &str, source: InvokeSource) -> Result<(), Error> {
        let actions = self.view.get().actions;
        // Only deliver when this notification actually offers an inline-reply affordance. Without
        // one, the typed text has nowhere to go — return without delivering *or* dismissing,
        // rather than silently dropping the text and dismissing as if it had been sent.
        if actions.reply.is_none() {
            return Ok(());
        }
        let dispatch = self.dispatch.get();
        let ctx = DispatchCtx {
            connection: &self.zbus_connection,
            urls: &actions.urls,
            // A reply doesn't raise a window, so no activation token.
            activation_token: None,
        };
        // Only dismiss if the backend actually delivered the reply (a backend without a reply
        // mechanism reports `false`, so we neither claim success nor dismiss).
        if dispatch.backend().reply(&ctx, text).await? {
            self.dismiss_after_action(source);
        }
        Ok(())
    }

    /// Opens a URI with the desktop's default handler via the XDG portal `OpenURI`. Used for a
    /// clickable `<a href>` link in a notification body: the shell routes the link click here
    /// rather than letting `GtkLabel`'s default handler call `gtk_show_uri`, which would try to
    /// parent the request on the shell's layer-shell surface and crash with a Wayland protocol
    /// error. Does NOT dismiss the notification (a link click is not an action).
    ///
    /// # Errors
    /// Returns an error if the portal call fails.
    pub async fn open_uri(&self, uri: &str) -> Result<(), Error> {
        open_uri_via_portal(&self.zbus_connection, uri).await
    }

    /// Applies `incoming`'s values onto this notification in place, so a `replaces_id` update
    /// keeps a stable `Arc` identity and observers react to a SINGLE atomic `view` change. `id`
    /// and `received` are identity and preserved (`received` is copied back from the current
    /// view); everything else in the view is replaced.
    pub(crate) fn update_from(&self, incoming: &Notification) {
        let mut view = incoming.view.get();
        let current = self.view.get();
        view.received = current.received;
        view.close_reason = current.close_reason;
        self.view.set(view);
        // A replace/coalesce can change the dispatch: GTK/portal action names or targets, or the
        // freedesktop wire id + owner (a `stack_tag` supersede reuses the identity but the new
        // notification carries its own wire id and owner). Replacing the whole dispatch refreshes
        // the owner too, so signals reach the live client.
        self.dispatch.replace(incoming.dispatch.get());
    }

    /// Wraps a backend-translated [`NotificationProps`] into the reactive `view` snapshot. The
    /// adapters have already done all protocol-specific translation, so this is a pure wrap —
    /// the twin of [`from_stored`](Self::from_stored) for freshly-delivered ones.
    fn from_props(
        props: NotificationProps,
        connection: Connection,
        notif_tx: broadcast::Sender<NotificationEvent>,
    ) -> Notification {
        Self {
            zbus_connection: connection,
            notif_tx,
            id: props.id,
            view: Property::new(NotificationView {
                origin: props.origin,
                content: props.content,
                image: props.image,
                actions: props.actions,
                alert: props.alert,
                classification: props.classification,
                lifecycle: props.lifecycle,
                presentation: props.presentation,
                received: props.timestamp,
                close_reason: None,
            }),
            dispatch: Property::new(props.dispatch),
        }
    }
}
