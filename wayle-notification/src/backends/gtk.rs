use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use derive_more::Debug;
use tokio::sync::broadcast;
use tracing::{debug, instrument, warn};
use wayle_core::Property;
use zbus::{Connection, fdo, zvariant::OwnedValue};

use async_trait::async_trait;

use super::{
    desktop_entry,
    gapplication::{self, APP_ACTION_PREFIX, APPLICATION_INTERFACE},
    glob,
    gvariant::{owned_string, parse_buttons, parse_icon, try_clone_owned},
};
use crate::{
    core::{
        backend::{Backend, DispatchCtx},
        notification::Notification,
        types::{
            Action, ActionId, Actions, Alert, Body, Classification, Content, DesktopEntryId,
            GtkAction, GtkDispatch, Lifecycle, NotificationId, NotificationProps,
            NotificationSource, Origin, Presentation, Timeout, gtk_object_path,
        },
    },
    error::Error,
    events::NotificationEvent,
    types::{
        ClosedReason, Priority,
        dbus::{GTK_SERVICE_NAME, GTK_SERVICE_PATH},
    },
};

/// Implements `org.gtk.Notifications`, the protocol GLib/GTK apps use via `GNotification`
/// (`g_application_send_notification`). Notifications are funneled into the same
/// `NotificationEvent` pipeline as freedesktop ones, so DND, popups, history, expiry and
/// persistence are shared; only action dispatch differs (see [`NotificationSource`]).
#[derive(Debug)]
pub(crate) struct GtkNotificationsDaemon {
    #[debug(skip)]
    pub zbus_connection: Connection,
    #[debug(skip)]
    pub notif_tx: broadcast::Sender<NotificationEvent>,
    #[debug(skip)]
    pub blocklist: Property<Vec<String>>,
    /// Maps `(app_id, notification_id)` → the synthesized id, so a re-send with the same key
    /// replaces in place and a `RemoveNotification` can find the id.
    #[debug(skip)]
    pub keys: Mutex<HashMap<(String, String), NotificationId>>,
}

#[zbus::interface(name = "org.gtk.Notifications")]
impl GtkNotificationsDaemon {
    /// Adds (or replaces, if the `(app_id, notification_id)` key already exists) a GTK
    /// notification.
    #[instrument(
        skip(self, notification),
        fields(app_id = %app_id, gtk_id = %notification_id)
    )]
    pub async fn add_notification(
        &self,
        app_id: String,
        notification_id: String,
        notification: HashMap<String, OwnedValue>,
    ) -> fdo::Result<()> {
        // Resolve a human-readable app name for display and for the blocklist, so GTK
        // notifications share the freedesktop human-name key space. Fall back to app id.
        let app_name =
            desktop_entry::resolve_name(&app_id).unwrap_or_else(|| app_id.clone());

        let blocked = self
            .blocklist
            .get()
            .iter()
            .any(|pattern| glob::matches(pattern, &app_name));
        if blocked {
            debug!(app = %app_name, "gtk notification blocked by blocklist");
            return Ok(());
        }

        let id = self.resolve_id(&app_id, &notification_id);
        debug!(id = %id, app = %app_name, "adding gtk notification");

        let props = build_props(
            id,
            Utc::now(),
            &app_id,
            &notification_id,
            app_name,
            &notification,
        );
        let notif = Notification::new(props, self.zbus_connection.clone(), self.notif_tx.clone());

        if self.notif_tx.send(NotificationEvent::Add(Box::new(notif))).is_err() {
            warn!("notification pipeline has no receiver; dropped an incoming notification");
        }
        Ok(())
    }

    /// Withdraws a notification previously added with the same key.
    #[instrument(skip(self), fields(app_id = %app_id, gtk_id = %notification_id))]
    pub async fn remove_notification(
        &self,
        app_id: String,
        notification_id: String,
    ) -> fdo::Result<()> {
        if let Some(id) = self.take_id(&app_id, &notification_id) {
            let _ = self
                .notif_tx
                .send(NotificationEvent::Remove(id, ClosedReason::Closed));
        }
        Ok(())
    }
}

impl GtkNotificationsDaemon {
    /// Builds the daemon, re-seeding the `(app_id, gtk_id)` → identity map from restored GTK
    /// notifications so an app can still replace or withdraw them across a restart.
    pub(crate) fn new(
        connection: Connection,
        notif_tx: broadcast::Sender<NotificationEvent>,
        blocklist: Property<Vec<String>>,
        restored: &[Arc<Notification>],
    ) -> Self {
        let mut keys = HashMap::new();
        for notif in restored {
            if let NotificationSource::Gtk(dispatch) = notif.dispatch.get() {
                keys.insert((dispatch.app_id, dispatch.gtk_id), notif.id);
            }
        }
        Self {
            zbus_connection: connection,
            notif_tx,
            blocklist,
            keys: Mutex::new(keys),
        }
    }

    /// Reuses the identity for an existing key (so a re-send replaces in place); otherwise
    /// generates a fresh identity and records the mapping.
    fn resolve_id(&self, app_id: &str, gtk_id: &str) -> NotificationId {
        let key = (app_id.to_owned(), gtk_id.to_owned());
        let mut keys = self.keys.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(&id) = keys.get(&key) {
            return id;
        }
        let id = NotificationId::generate();
        keys.insert(key, id);
        id
    }

    fn take_id(&self, app_id: &str, gtk_id: &str) -> Option<NotificationId> {
        let key = (app_id.to_owned(), gtk_id.to_owned());
        self.keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&key)
    }
}

#[async_trait]
impl Backend for GtkDispatch {
    /// Dispatches matching GNOME Shell: an `"app."`-prefixed action → the app's action group via
    /// `org.freedesktop.Application.ActivateAction(name, [target?], platform_data)` (cold-launching
    /// a `DBusActivatable` app that isn't running); any other action name → the
    /// `org.gtk.Notifications.ActionInvoked(app_id, id, action, [target?], platform_data)` signal;
    /// and a body click with no default action → `Activate(platform_data)` (raise/launch).
    async fn dispatch_action(&self, ctx: &DispatchCtx<'_>, action: &ActionId) -> Result<(), Error> {
        let gtk_action = if action.is_default() {
            self.default_action.as_ref()
        } else {
            self.button_actions.get(action.as_str())
        };
        let Some(gtk_action) = gtk_action else {
            // Body click with no default action → raise/launch the app (GNOME Shell parity).
            return gapplication::activate(ctx.connection, &self.app_id, ctx.activation_token).await;
        };
        // Carry the shell's focus token so the (possibly cold-launched) app can raise its window.
        let platform_data = gapplication::platform_data(ctx.activation_token);
        // `av` parameter: zero or one target variant, matching GNOME's shell.
        let parameter: Vec<OwnedValue> = gtk_action
            .target
            .as_ref()
            .and_then(|target| target.try_clone().ok())
            .into_iter()
            .collect();
        match gtk_action.name.strip_prefix(APP_ACTION_PREFIX) {
            Some(app_action) => {
                ctx.connection
                    .call_method(
                        Some(self.app_id.as_str()),
                        gtk_object_path(&self.app_id).as_str(),
                        Some(APPLICATION_INTERFACE),
                        "ActivateAction",
                        &(app_action, parameter, platform_data),
                    )
                    .await?;
            }
            None => {
                // A non-`app.` action isn't in the app's action group, so GNOME relays it back
                // as an `org.gtk.Notifications.ActionInvoked` signal the app can pick up.
                ctx.connection
                    .emit_signal(
                        None::<&str>,
                        GTK_SERVICE_PATH,
                        GTK_SERVICE_NAME,
                        "ActionInvoked",
                        &(
                            self.app_id.as_str(),
                            self.gtk_id.as_str(),
                            gtk_action.name.as_str(),
                            parameter,
                            platform_data,
                        ),
                    )
                    .await?;
            }
        }
        Ok(())
    }
    // reply: GNotification has no inline-reply mechanism — the trait's no-op default applies.
    // close: GNotification has no per-notification close-back signal — the no-op default applies.

    /// A GTK notification is reachable while its app owns its bus name OR the app is
    /// D-Bus-activatable (cold-launchable); strip only when neither holds.
    fn is_unreachable(&self, live: &HashSet<String>, activatable: &HashSet<String>) -> bool {
        !live.contains(&self.app_id) && !activatable.contains(&self.app_id)
    }

    fn dispatch_target(&self) -> Option<&str> {
        Some(&self.app_id)
    }

    /// Unlike freedesktop/portal, a GTK notification can be delivered for an app that is neither
    /// running nor activatable, so its reachability must be checked at ingest.
    fn may_be_unreachable_at_ingest(&self) -> bool {
        true
    }
}

/// Translates a GNotification `a{sv}` into the unified facet model.
///
/// GNotification has no timeout, sound, category or urgency-hint vocabulary: the mapping is
/// direct. The single serialized-GIcon `icon` is the app's icon → [`Origin::icon`] (themed name
/// or cached/on-disk path), never the large content-image slot; every button becomes a displayed
/// [`Action`] plus dispatch metadata (GNOME parity), and the body is always clickable — a click
/// invokes the `default-action` or raises the app.
fn build_props(
    id: NotificationId,
    timestamp: DateTime<Utc>,
    app_id: &str,
    gtk_id: &str,
    app_name: String,
    map: &HashMap<String, OwnedValue>,
) -> NotificationProps {
    let title = owned_string(map.get("title")).unwrap_or_default();
    let body = owned_string(map.get("body"));

    // GNotification's 4 priorities map 1:1 onto our 4-level Priority. Unknown/absent → Normal.
    // Legacy fallback: a bare `urgent` bool (which gnome-shell's server still honors) maps
    // true → Urgent when no `priority` string is present. Current GLib folds urgency into
    // `priority`, so this only matters for older/third-party senders.
    let priority = owned_string(map.get("priority"))
        .and_then(|value| value.parse::<Priority>().ok())
        .or_else(|| {
            map.get("urgent")
                .and_then(|value| value.downcast_ref::<bool>().ok())
                .and_then(|urgent| urgent.then_some(Priority::Urgent))
        })
        .unwrap_or_default();

    // The GNotification icon is the app's own icon (any GIcon form) → origin.icon.
    let icon = map.get("icon").and_then(parse_icon);

    // Buttons → dispatch metadata + displayed actions. GNOME Shell displays EVERY button
    // regardless of action prefix; only the dispatch route differs (`"app."` → ActivateAction,
    // otherwise → an ActionInvoked signal), decided at click time. So keep them all here and
    // store the action name verbatim.
    let mut button_actions = HashMap::new();
    let mut buttons = Vec::new();
    for button in parse_buttons(map.get("buttons")) {
        // The displayed action id is the action name as received. It can only fail to construct
        // if the app literally named a button `"default"` (the reserved body-click key) — skip
        // that pathological case rather than shadow the default action.
        let Ok(action_id) = ActionId::new(button.action.clone()) else {
            continue;
        };
        buttons.push(Action {
            id: action_id,
            label: button.label,
            purpose: None,
        });
        button_actions.insert(
            button.action.clone(),
            GtkAction {
                name: button.action,
                target: button.target,
            },
        );
    }

    // The default action is likewise kept verbatim (GNOME dispatches it via the same
    // app.-prefix rule on body click).
    let default_action = owned_string(map.get("default-action")).map(|action| GtkAction {
        name: action,
        target: map.get("default-action-target").and_then(try_clone_owned),
    });

    NotificationProps {
        id,
        timestamp,
        dispatch: NotificationSource::Gtk(GtkDispatch {
            app_id: app_id.to_owned(),
            gtk_id: gtk_id.to_owned(),
            default_action,
            button_actions,
        }),
        origin: Origin {
            name: Some(app_name),
            // GNotification has no secondary-origin concept.
            origin_name: None,
            // The app id is the trusted grouping/icon-resolution key.
            desktop_entry: Some(DesktopEntryId::new(app_id)),
            icon,
        },
        content: Content {
            summary: title,
            body: body.map(Body::Plain),
            // GNotification has no progress/value concept.
            progress: None,
        },
        // GNotification has no separate large-content-image concept; its icon is the app icon.
        image: None,
        actions: Actions {
            // Always body-clickable: invokes `default-action`, or raises the app if none.
            default: Some(Action {
                id: ActionId::default_action(),
                label: String::new(),
                purpose: None,
            }),
            buttons,
            // GNotification cannot express inline reply or attached URLs.
            reply: None,
            urls: Vec::new(),
            action_icons: false,
        },
        // GNotification has no sound, category, urgency-hint, timeout or display taxonomy.
        alert: Alert::Unspecified,
        classification: Classification {
            priority,
            category: None,
            transient: false,
            resident: false,
            stack_tag: None,
        },
        lifecycle: Lifecycle {
            timeout: Timeout::PersistentByBackend,
            locked_open: false,
        },
        presentation: Presentation::default(),
    }
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::Value;

    use super::*;
    use crate::core::types::Image;

    fn owned(value: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(value).expect("value converts to OwnedValue")
    }

    fn props(app_id: &str, map: &HashMap<String, OwnedValue>) -> NotificationProps {
        build_props(NotificationId::new(1), Utc::now(), app_id, "n1", String::from("Fractal"), map)
    }

    #[test]
    fn realistic_chat_notification_maps_all_facets() {
        let mut button: HashMap<String, OwnedValue> = HashMap::new();
        button.insert(String::from("label"), owned(Value::new("Reply")));
        button.insert(String::from("action"), owned(Value::new("app.reply")));
        button.insert(String::from("target"), owned(Value::new("room-7")));

        let mut map: HashMap<String, OwnedValue> = HashMap::new();
        map.insert(String::from("title"), owned(Value::new("Alice")));
        map.insert(String::from("body"), owned(Value::new("hey <there>")));
        map.insert(String::from("priority"), owned(Value::new("high")));
        map.insert(
            String::from("default-action"),
            owned(Value::new("app.open-room")),
        );
        map.insert(String::from("buttons"), owned(Value::new(vec![button])));

        let props = props("org.gnome.Fractal", &map);

        // Origin: display name + trusted app id + no themed icon here.
        assert_eq!(props.origin.name.as_deref(), Some("Fractal"));
        assert_eq!(
            props.origin.desktop_entry.as_ref().map(DesktopEntryId::as_str),
            Some("org.gnome.Fractal")
        );
        assert!(props.origin.icon.is_none());

        // Content: GNotification body is PLAIN (the shell will escape the `<there>`).
        assert_eq!(props.content.summary, "Alice");
        assert_eq!(props.content.body, Some(Body::Plain(String::from("hey <there>"))));

        // Actions: always body-clickable; one "app."-prefixed button surfaced with its label.
        assert!(props.actions.default.as_ref().is_some_and(|action| action.id.is_default()));
        assert_eq!(props.actions.buttons.len(), 1);
        assert_eq!(props.actions.buttons[0].id.as_str(), "app.reply");
        assert_eq!(props.actions.buttons[0].label, "Reply");

        // Classification: priority parsed; no category/transient/etc.
        assert_eq!(props.classification.priority, Priority::High);
        assert_eq!(props.classification.category, None);

        // No timeout concept; alert unspecified.
        assert_eq!(props.lifecycle.timeout, Timeout::PersistentByBackend);
        assert_eq!(props.alert, Alert::Unspecified);

        // Dispatch carries the app id, gtk id, default + button targets.
        let NotificationSource::Gtk(dispatch) = &props.dispatch else {
            panic!("expected a gtk dispatch");
        };
        assert_eq!(dispatch.app_id, "org.gnome.Fractal");
        // The action name is kept verbatim (the `app.` prefix is stripped only at dispatch).
        assert_eq!(dispatch.default_action.as_ref().unwrap().name, "app.open-room");
        let reply = dispatch.button_actions.get("app.reply").expect("button dispatch");
        assert_eq!(reply.name, "app.reply");
        assert!(reply.target.is_some());
    }

    #[test]
    fn iconless_notification_still_carries_app_id_for_icon_resolution() {
        // A GNOME-app notification (e.g. org.gnome.Clocks) with no icon: the shell resolves
        // the icon from the desktop entry, so `origin.desktop_entry` must be the app id.
        let map: HashMap<String, OwnedValue> = HashMap::new();
        let props = build_props(
            NotificationId::new(1),
            Utc::now(),
            "org.gnome.Clocks",
            "alarm-1",
            String::from("Clocks"),
            &map,
        );

        assert!(props.origin.icon.is_none(), "no icon key ⇒ no themed origin icon");
        assert_eq!(
            props.origin.desktop_entry.as_ref().map(DesktopEntryId::as_str),
            Some("org.gnome.Clocks")
        );
        // Body-clickable even with no explicit default-action (raises the app).
        assert!(props.actions.default.is_some());
    }

    #[test]
    fn themed_icon_becomes_origin_icon() {
        let icon = Value::new((
            String::from("themed"),
            Value::new(vec![String::from("org.gnome.Clocks")]),
        ));
        let mut map: HashMap<String, OwnedValue> = HashMap::new();
        map.insert(String::from("icon"), owned(icon));

        let props = props("org.gnome.Clocks", &map);

        assert_eq!(
            props.origin.icon,
            Some(Image::Named(String::from("org.gnome.Clocks")))
        );
        assert!(props.image.is_none(), "themed icon is a header icon, not a content image");
    }

    #[test]
    fn non_app_prefixed_buttons_are_displayed_and_dispatchable() {
        let mut button: HashMap<String, OwnedValue> = HashMap::new();
        button.insert(String::from("label"), owned(Value::new("Snooze")));
        button.insert(String::from("action"), owned(Value::new("win.close")));
        let mut map: HashMap<String, OwnedValue> = HashMap::new();
        map.insert(String::from("buttons"), owned(Value::new(vec![button])));

        let props = props("org.gnome.Fractal", &map);

        // GNOME Shell displays every button regardless of prefix; a non-`app.` one is kept and
        // routed via `ActionInvoked` at dispatch (rather than dropped).
        assert_eq!(props.actions.buttons.len(), 1, "every button is displayed");
        assert_eq!(props.actions.buttons[0].label, "Snooze");
        let NotificationSource::Gtk(dispatch) = props.dispatch else {
            panic!("expected a GTK dispatch");
        };
        let action = dispatch.button_actions.get("win.close").expect("non-app. button kept for dispatch");
        assert_eq!(action.name, "win.close", "the full action name is retained");
    }

    #[test]
    fn absent_priority_defaults_to_normal() {
        let map: HashMap<String, OwnedValue> = HashMap::new();
        assert_eq!(props("app.X", &map).classification.priority, Priority::Normal);
    }
}
