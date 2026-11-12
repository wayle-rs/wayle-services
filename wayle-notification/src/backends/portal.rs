use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use derive_more::Debug;
use tokio::sync::broadcast;
use tracing::{debug, instrument, warn};
use wayle_core::Property;
use zbus::{
    Connection, fdo,
    zvariant::{OwnedValue, Value},
};

use async_trait::async_trait;

use super::{
    desktop_entry, gapplication, glob,
    gvariant::{owned_string, parse_buttons, parse_icon, read_fd, try_clone_owned},
};
use crate::{
    core::{
        backend::{Backend, DispatchCtx},
        notification::Notification,
        types::{
            Action, ActionId, Actions, Alert, Body, Classification, Content, DesktopEntryId,
            Lifecycle, LockscreenVisibility, NotificationId, NotificationProps,
            NotificationSource, Origin, PORTAL_NOTIFICATION_INTERFACE, PORTAL_OBJECT_PATH,
            PortalAction, PortalDispatch, Presentation, Reply, Routing, Timeout,
        },
    },
    error::Error,
    events::NotificationEvent,
    image_cache,
    types::{ButtonPurpose, Category, ClosedReason, Priority, Signal},
};

/// The interface version this backend implements (portal Notification v2).
const INTERFACE_VERSION: u32 = 2;

/// Implements `org.freedesktop.impl.portal.Notification`, the backend interface
/// xdg-desktop-portal forwards sandboxed apps' notifications to. Notifications funnel into
/// the same [`NotificationEvent`] pipeline as the freedesktop and GTK backends; only action
/// dispatch differs — see [`NotificationSource::Portal`] and `activate_portal`.
///
/// The `zbus_connection` here is the *shared portal connection* (the one owning
/// `org.freedesktop.impl.portal.desktop.<shell>`), so the `ActionInvoked` signal is emitted
/// on the connection xdg-desktop-portal subscribes to.
#[derive(Debug)]
pub(crate) struct PortalNotificationsDaemon {
    #[debug(skip)]
    pub zbus_connection: Connection,
    #[debug(skip)]
    pub notif_tx: broadcast::Sender<NotificationEvent>,
    #[debug(skip)]
    pub blocklist: Property<Vec<String>>,
    /// Maps `(app_id, portal_id)` → the synthesized id, so a re-send with the same key
    /// replaces in place and a `RemoveNotification` can find the id.
    #[debug(skip)]
    pub keys: Mutex<HashMap<(String, String), NotificationId>>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Notification")]
impl PortalNotificationsDaemon {
    /// Adds (or replaces, if the `(app_id, id)` key already exists) a portal notification.
    #[instrument(skip(self, notification), fields(app_id = %app_id, portal_id = %id))]
    pub async fn add_notification(
        &self,
        app_id: String,
        id: String,
        notification: HashMap<String, OwnedValue>,
    ) -> fdo::Result<()> {
        let app_name = desktop_entry::resolve_name(&app_id).unwrap_or_else(|| app_id.clone());

        let blocked = self
            .blocklist
            .get()
            .iter()
            .any(|pattern| glob::matches(pattern, &app_name));
        if blocked {
            debug!(app = %app_name, "portal notification blocked by blocklist");
            return Ok(());
        }

        let notif_id = self.resolve_id(&app_id, &id);
        debug!(id = %notif_id, app = %app_name, "adding portal notification");

        let props = build_props(notif_id, Utc::now(), &app_id, &id, app_name, &notification);
        let notif = Notification::new(props, self.zbus_connection.clone(), self.notif_tx.clone());

        if self.notif_tx.send(NotificationEvent::Add(Box::new(notif))).is_err() {
            warn!("notification pipeline has no receiver; dropped an incoming notification");
        }
        Ok(())
    }

    /// Withdraws a notification previously added with the same key.
    #[instrument(skip(self), fields(app_id = %app_id, portal_id = %id))]
    pub async fn remove_notification(&self, app_id: String, id: String) -> fdo::Result<()> {
        if let Some(notif_id) = self.take_id(&app_id, &id) {
            let _ = self
                .notif_tx
                .send(NotificationEvent::Remove(notif_id, ClosedReason::Closed));
        }
        Ok(())
    }

    /// The interface version (portal Notification v2). Lowercase `version` per portal
    /// convention.
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        INTERFACE_VERSION
    }

    /// Advertises which optional values this server understands, so clients can adapt. We
    /// understand the full v2 button-purpose taxonomy (custom alert, inline reply, and the
    /// call controls).
    #[zbus(property)]
    fn supported_options(&self) -> HashMap<String, OwnedValue> {
        let mut options = HashMap::new();
        let purposes = vec![
            String::from("system.custom-alert"),
            String::from("im.reply-with-text"),
            String::from("call.accept"),
            String::from("call.decline"),
            String::from("call.hang-up"),
            String::from("call.enable-speakerphone"),
            String::from("call.disable-speakerphone"),
        ];
        if let Ok(purposes) = OwnedValue::try_from(Value::new(purposes)) {
            options.insert(String::from("button-purpose"), purposes);
        }

        // Advertise exactly the portal v2 category taxonomy (NOT the wider freedesktop set), so
        // the portal frontend sees the categories this interface version actually defines.
        let categories: Vec<String> = Category::PORTAL.iter().map(|c| (*c).to_owned()).collect();
        if let Ok(categories) = OwnedValue::try_from(Value::new(categories)) {
            options.insert(String::from("category"), categories);
        }
        options
    }
}

impl PortalNotificationsDaemon {
    /// Builds the daemon, re-seeding the `(app_id, portal_id)` → identity map from restored
    /// portal notifications so a re-send replaces in place (and `RemoveNotification` still
    /// finds them) across a restart.
    pub(crate) fn new(
        connection: Connection,
        notif_tx: broadcast::Sender<NotificationEvent>,
        blocklist: Property<Vec<String>>,
        restored: &[Arc<Notification>],
    ) -> Self {
        let mut keys = HashMap::new();
        for notif in restored {
            if let NotificationSource::Portal(dispatch) = notif.dispatch.get() {
                keys.insert((dispatch.app_id, dispatch.portal_id), notif.id);
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
    fn resolve_id(&self, app_id: &str, portal_id: &str) -> NotificationId {
        let key = (app_id.to_owned(), portal_id.to_owned());
        let mut keys = self.keys.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(&id) = keys.get(&key) {
            return id;
        }
        let id = NotificationId::generate();
        keys.insert(key, id);
        id
    }

    fn take_id(&self, app_id: &str, portal_id: &str) -> Option<NotificationId> {
        let key = (app_id.to_owned(), portal_id.to_owned());
        self.keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&key)
    }
}

#[async_trait]
impl Backend for PortalDispatch {
    /// A declared action (default or button) emits the backend `ActionInvoked` on the shared
    /// portal connection; xdg-desktop-portal (the sole subscriber) relays it to the app, handling
    /// the `app.`-prefix → `ActivateAction` cold-launch itself, so we report the action name
    /// verbatim. A body click with NO declared default-action instead raises/launches the app
    /// directly (mirroring GNOME Shell and the GTK backend); an unknown button name ⇒ no-op.
    async fn dispatch_action(&self, ctx: &DispatchCtx<'_>, action: &ActionId) -> Result<(), Error> {
        let portal_action = if action.is_default() {
            self.default_action.as_ref()
        } else {
            self.button_actions.get(action.as_str())
        };
        let Some(portal_action) = portal_action else {
            // A body click with no declared default-action → best-effort raise/launch the app.
            // Emitting a synthetic `ActionInvoked` here would no-op (the app registered no such
            // action and xdg-desktop-portal never calls `Activate` itself), so we raise directly.
            // Unlike GTK, a portal notification is never stripped for reachability, so its app_id
            // may not be D-Bus-activatable (e.g. a scope-derived id with no service file); a failed
            // raise is then a silent no-op, not an error. An unknown *button* name is a no-op.
            if action.is_default()
                && let Err(error) =
                    gapplication::activate(ctx.connection, &self.app_id, ctx.activation_token).await
            {
                debug!(app_id = %self.app_id, %error, "portal default-click raise failed (app not activatable)");
            }
            return Ok(());
        };
        // `av` = [ target? , platform-data{activation-token} ] — the same ordering
        // xdg-desktop-portal-gtk emits, which the portal frontend relays verbatim to the app so
        // it can raise its window. xdg-desktop-portal never fills the token in itself, so the
        // backend (us) must supply it; it's omitted only when the shell couldn't mint one.
        let mut parameter: Vec<OwnedValue> = portal_action
            .target
            .as_ref()
            .and_then(|target| target.try_clone().ok())
            .into_iter()
            .collect();
        if let Some(platform_data) = activation_platform_data(ctx.activation_token) {
            parameter.push(platform_data);
        }
        ctx.connection
            .emit_signal(
                None::<&str>,
                PORTAL_OBJECT_PATH,
                PORTAL_NOTIFICATION_INTERFACE,
                Signal::ActionInvoked.as_str(),
                &(
                    self.app_id.as_str(),
                    self.portal_id.as_str(),
                    portal_action.name.as_str(),
                    parameter,
                ),
            )
            .await?;
        Ok(())
    }

    /// Emits the `im.reply-with-text` button's `ActionInvoked` with the typed text appended to
    /// the parameter array; xdg-desktop-portal positions it and relays to the app.
    async fn reply(&self, ctx: &DispatchCtx<'_>, text: &str) -> Result<bool, Error> {
        let Some(action) = self.reply_action.as_ref() else {
            return Ok(false);
        };
        let mut parameter: Vec<OwnedValue> = action
            .target
            .as_ref()
            .and_then(|target| target.try_clone().ok())
            .into_iter()
            .collect();
        if let Ok(text_value) = OwnedValue::try_from(Value::new(text)) {
            parameter.push(text_value);
        }
        ctx.connection
            .emit_signal(
                None::<&str>,
                PORTAL_OBJECT_PATH,
                PORTAL_NOTIFICATION_INTERFACE,
                Signal::ActionInvoked.as_str(),
                &(
                    self.app_id.as_str(),
                    self.portal_id.as_str(),
                    action.name.as_str(),
                    parameter,
                ),
            )
            .await?;
        Ok(true)
    }
    // close: the portal has no per-notification close-back signal — the no-op default applies.

    /// Portal notifications dispatch through the persistent xdg-desktop-portal frontend (which
    /// relays to, and cold-launches, the sandboxed app itself), so they are never unreachable.
    fn is_unreachable(&self, _live: &HashSet<String>, _activatable: &HashSet<String>) -> bool {
        false
    }

    /// No bus name of the app's own gates reachability, so a vanished name never invalidates it.
    fn dispatch_target(&self) -> Option<&str> {
        None
    }
}

/// Builds the portal `av` platform-data element — an `a{sv}` `{"activation-token": <token>}`
/// wrapped as a variant — appended after the action target in `ActionInvoked`. `None` when the
/// shell couldn't mint a token.
fn activation_platform_data(token: Option<&str>) -> Option<OwnedValue> {
    let token = token?;
    let mut platform_data: HashMap<String, OwnedValue> = HashMap::new();
    platform_data.insert(
        String::from("activation-token"),
        OwnedValue::try_from(Value::new(token)).ok()?,
    );
    OwnedValue::try_from(Value::new(platform_data)).ok()
}

/// Translates a portal `a{sv}` (v2) into the unified facet model.
///
/// Portal is the richest wire format: a `markup-body` vs `body` distinction, a `category`
/// taxonomy, a `sound` intent, per-button `purpose`, and a `display-hint` array driving
/// tray/persistent/lock-screen/re-alert behavior. Every action name is kept verbatim (unlike
/// GTK, the frontend interprets the `app.` prefix on `ActionInvoked`).
fn build_props(
    id: NotificationId,
    timestamp: DateTime<Utc>,
    app_id: &str,
    portal_id: &str,
    app_name: String,
    map: &HashMap<String, OwnedValue>,
) -> NotificationProps {
    let title = owned_string(map.get("title")).unwrap_or_default();
    // Prefer the markup body (v2); the enum variant tells the shell how to render it.
    let body = match owned_string(map.get("markup-body")) {
        Some(markup) => Some(Body::Markup(markup)),
        None => owned_string(map.get("body")).map(Body::Plain),
    };

    let priority = owned_string(map.get("priority"))
        .and_then(|value| value.parse::<Priority>().ok())
        .unwrap_or_default();

    // The portal `icon` is the app's own icon (any GIcon form) → origin.icon.
    // (A v2 backend receives `file-descriptor` for inline icons; see `parse_icon`.)
    let icon = map.get("icon").and_then(parse_icon);

    // Buttons keep their raw action name (no `"app."` stripping) and may carry a purpose.
    let mut button_actions = HashMap::new();
    let mut buttons = Vec::new();
    let mut reply = None;
    let mut reply_action = None;
    for button in parse_buttons(map.get("buttons")) {
        // A portal action name is kept verbatim; skip the (spec-illegal) reserved key.
        let Ok(action_id) = ActionId::new(button.action.clone()) else {
            continue;
        };
        let purpose = button.purpose.and_then(|p| p.parse::<ButtonPurpose>().ok());

        // `im.reply-with-text` is the inline-reply affordance, not a normal button: lift it
        // to `Actions::reply` + a dispatch target (the typed text is appended to this action's
        // `ActionInvoked` parameter array).
        if purpose == Some(ButtonPurpose::ImReplyWithText) {
            reply = Some(Reply {
                placeholder: None,
                submit_label: (!button.label.is_empty()).then_some(button.label),
                submit_icon: None,
            });
            reply_action = Some(PortalAction {
                name: button.action,
                target: button.target,
                purpose,
            });
            continue;
        }

        button_actions.insert(
            button.action.clone(),
            PortalAction {
                name: button.action,
                target: button.target,
                purpose: purpose.clone(),
            },
        );
        buttons.push(Action {
            id: action_id,
            label: button.label,
            purpose,
        });
    }

    // The body is always clickable (GNOME Shell parity): a declared default-action is invoked,
    // otherwise the click raises/launches the app — see this backend's `dispatch_action`.
    let portal_default = owned_string(map.get("default-action")).map(|name| PortalAction {
        name,
        target: map.get("default-action-target").and_then(try_clone_owned),
        purpose: None,
    });
    let default = Some(Action {
        id: ActionId::default_action(),
        label: String::new(),
        purpose: None,
    });

    let alert = parse_sound(map);

    let lockscreen = if has_display_hint(map, "hide-content-on-lockscreen") {
        LockscreenVisibility::HideContent
    } else if has_display_hint(map, "hide-on-lockscreen") {
        LockscreenVisibility::Hide
    } else {
        LockscreenVisibility::Show
    };

    NotificationProps {
        id,
        timestamp,
        dispatch: NotificationSource::Portal(PortalDispatch {
            app_id: app_id.to_owned(),
            portal_id: portal_id.to_owned(),
            default_action: portal_default,
            button_actions,
            reply_action,
        }),
        origin: Origin {
            name: Some(app_name),
            // The portal has no secondary-origin concept.
            origin_name: None,
            desktop_entry: Some(DesktopEntryId::new(app_id)),
            icon,
        },
        content: Content {
            summary: title,
            body,
            // The portal Notification interface has no progress/value concept.
            progress: None,
        },
        // The portal has no separate large-content-image concept; its icon is the app icon.
        image: None,
        actions: Actions {
            default,
            buttons,
            reply,
            // The portal has no attached-URLs concept.
            urls: Vec::new(),
            action_icons: false,
        },
        alert,
        classification: Classification {
            priority,
            category: owned_string(map.get("category")).and_then(|category| category.parse().ok()),
            transient: has_display_hint(map, "transient"),
            resident: false,
            stack_tag: None,
        },
        lifecycle: Lifecycle {
            timeout: Timeout::PersistentByBackend,
            locked_open: has_display_hint(map, "persistent"),
        },
        presentation: Presentation {
            routing: if has_display_hint(map, "tray") {
                Routing::TrayOnly
            } else {
                Routing::Banner
            },
            lockscreen,
            realert_on_replace: has_display_hint(map, "show-as-new"),
        },
    }
}

/// Translates the portal `sound` value into a typed [`Alert`]: the bare strings `"silent"`
/// / `"default"`, or a `('file-descriptor', <h>)` whose fd is read and cached to a file the
/// shell can play (ogg/opus, ogg/vorbis or wav/pcm per the portal spec).
fn parse_sound(map: &HashMap<String, OwnedValue>) -> Alert {
    let Some(value) = map.get("sound") else {
        return Alert::Unspecified;
    };
    match &**value {
        Value::Str(name) => match name.as_str() {
            "silent" => Alert::Silent,
            "default" => Alert::Default,
            _ => Alert::Unspecified,
        },
        Value::Structure(structure) => {
            if let [Value::Str(tag), Value::Value(payload)] = structure.fields()
                && tag.as_str() == "file-descriptor"
                && let Value::Fd(fd) = &**payload
                && let Some(bytes) = read_fd(fd)
                && let Some(path) = image_cache::cache_encoded_sound(&bytes)
            {
                Alert::File(PathBuf::from(path))
            } else {
                Alert::Unspecified
            }
        }
        _ => Alert::Unspecified,
    }
}

/// Whether the notification's `display-hint` (`as`) array contains `hint`.
fn has_display_hint(map: &HashMap<String, OwnedValue>, hint: &str) -> bool {
    let Some(value) = map.get("display-hint") else {
        return false;
    };
    let Value::Array(array) = &**value else {
        return false;
    };
    array
        .iter()
        .any(|element| matches!(element, Value::Str(name) if name.as_str() == hint))
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::Value;

    use super::*;
    use crate::types::Category;

    fn owned(value: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(value).expect("value converts to OwnedValue")
    }

    fn props(map: &HashMap<String, OwnedValue>) -> NotificationProps {
        build_props(
            NotificationId::new(1),
            Utc::now(),
            "org.example.App",
            "n1",
            String::from("Example"),
            map,
        )
    }

    #[test]
    fn realistic_portal_notification_maps_all_facets() {
        let mut button: HashMap<String, OwnedValue> = HashMap::new();
        button.insert(String::from("label"), owned(Value::new("Snooze")));
        button.insert(String::from("action"), owned(Value::new("app.snooze")));
        button.insert(String::from("purpose"), owned(Value::new("system.custom-alert")));

        let mut map: HashMap<String, OwnedValue> = HashMap::new();
        map.insert(String::from("title"), owned(Value::new("Reminder")));
        map.insert(String::from("markup-body"), owned(Value::new("<i>Standup</i>")));
        map.insert(String::from("body"), owned(Value::new("Standup")));
        map.insert(String::from("priority"), owned(Value::new("urgent")));
        map.insert(String::from("category"), owned(Value::new("im.received")));
        map.insert(String::from("default-action"), owned(Value::new("app.open")));
        map.insert(String::from("buttons"), owned(Value::new(vec![button])));
        map.insert(
            String::from("display-hint"),
            owned(Value::new(vec![
                String::from("transient"),
                String::from("tray"),
            ])),
        );

        let props = props(&map);

        assert_eq!(
            props.origin.desktop_entry.as_ref().map(DesktopEntryId::as_str),
            Some("org.example.App")
        );

        // Content: markup-body wins over the plain body and is flagged as markup.
        assert_eq!(props.content.body, Some(Body::Markup(String::from("<i>Standup</i>"))));

        // Actions: default present (declared); button carries its purpose.
        assert!(props.actions.default.as_ref().is_some_and(|a| a.id.is_default()));
        assert_eq!(props.actions.buttons[0].id.as_str(), "app.snooze");
        assert_eq!(
            props.actions.buttons[0].purpose,
            Some(ButtonPurpose::SystemCustomAlert)
        );

        // Classification: priority + category + transient (from display-hint).
        assert_eq!(props.classification.priority, Priority::Urgent);
        assert_eq!(props.classification.category, Some(Category::ImReceived));
        assert!(props.classification.transient);

        // Presentation: tray routing from the display-hint.
        assert_eq!(props.presentation.routing, Routing::TrayOnly);

        let NotificationSource::Portal(dispatch) = &props.dispatch else {
            panic!("expected a portal dispatch");
        };
        assert_eq!(dispatch.default_action.as_ref().unwrap().name, "app.open");
        assert!(dispatch.button_actions.contains_key("app.snooze"));
    }

    #[test]
    fn im_reply_button_becomes_reply_affordance() {
        let mut reply_btn: HashMap<String, OwnedValue> = HashMap::new();
        reply_btn.insert(String::from("label"), owned(Value::new("Reply")));
        reply_btn.insert(String::from("action"), owned(Value::new("app.reply")));
        reply_btn.insert(String::from("purpose"), owned(Value::new("im.reply-with-text")));
        let mut mute_btn: HashMap<String, OwnedValue> = HashMap::new();
        mute_btn.insert(String::from("label"), owned(Value::new("Mute")));
        mute_btn.insert(String::from("action"), owned(Value::new("app.mute")));
        let mut map: HashMap<String, OwnedValue> = HashMap::new();
        map.insert(String::from("buttons"), owned(Value::new(vec![reply_btn, mute_btn])));

        let props = props(&map);

        // im.reply-with-text is lifted to a reply affordance (submit label = the button label).
        let reply = props.actions.reply.expect("im.reply-with-text ⇒ reply affordance");
        assert_eq!(reply.submit_label.as_deref(), Some("Reply"));
        // …and is NOT a normal button.
        let button_ids: Vec<_> = props.actions.buttons.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(button_ids, vec!["app.mute"]);
        // The dispatch carries the reply action (not among the normal button actions).
        let NotificationSource::Portal(dispatch) = &props.dispatch else {
            panic!("expected a portal dispatch");
        };
        assert_eq!(dispatch.reply_action.as_ref().unwrap().name, "app.reply");
        assert!(!dispatch.button_actions.contains_key("app.reply"));
    }

    #[test]
    fn call_button_purposes_parse() {
        let purpose = |raw: &str| {
            let mut btn: HashMap<String, OwnedValue> = HashMap::new();
            btn.insert(String::from("label"), owned(Value::new("x")));
            btn.insert(String::from("action"), owned(Value::new("app.x")));
            btn.insert(String::from("purpose"), owned(Value::new(raw)));
            let mut map: HashMap<String, OwnedValue> = HashMap::new();
            map.insert(String::from("buttons"), owned(Value::new(vec![btn])));
            props(&map)
                .actions
                .buttons
                .first()
                .and_then(|button| button.purpose.clone())
        };
        assert_eq!(purpose("call.accept"), Some(ButtonPurpose::CallAccept));
        assert_eq!(purpose("call.decline"), Some(ButtonPurpose::CallDecline));
        assert_eq!(purpose("call.hang-up"), Some(ButtonPurpose::CallHangUp));
    }

    #[test]
    fn plain_body_without_markup_is_plain() {
        let mut map: HashMap<String, OwnedValue> = HashMap::new();
        map.insert(String::from("body"), owned(Value::new("just text")));
        assert_eq!(
            props(&map).content.body,
            Some(Body::Plain(String::from("just text")))
        );
    }

    #[test]
    fn no_default_action_still_clickable_and_raises_app() {
        // GNOME Shell parity: with no declared default-action the body is still clickable, but the
        // dispatch carries no default_action, so `dispatch_action` falls back to raising the app
        // via `org.freedesktop.Application.Activate` rather than emitting `ActionInvoked`.
        let map: HashMap<String, OwnedValue> = HashMap::new();
        let props = props(&map);
        assert!(
            props.actions.default.as_ref().is_some_and(|action| action.id.is_default()),
            "body is always clickable",
        );
        let NotificationSource::Portal(dispatch) = &props.dispatch else {
            panic!("expected a portal dispatch");
        };
        assert!(dispatch.default_action.is_none(), "no declared default-action");
    }

    #[test]
    fn sound_intent_maps_to_typed_alert() {
        let alert = |sound: Option<&str>| {
            let mut map: HashMap<String, OwnedValue> = HashMap::new();
            if let Some(sound) = sound {
                map.insert(String::from("sound"), owned(Value::new(sound)));
            }
            props(&map).alert
        };
        assert_eq!(alert(Some("silent")), Alert::Silent);
        assert_eq!(alert(Some("default")), Alert::Default);
        assert_eq!(alert(None), Alert::Unspecified);
    }

    #[test]
    fn display_hints_drive_lifecycle_and_presentation() {
        let mut map: HashMap<String, OwnedValue> = HashMap::new();
        map.insert(
            String::from("display-hint"),
            owned(Value::new(vec![
                String::from("persistent"),
                String::from("show-as-new"),
                String::from("hide-content-on-lockscreen"),
            ])),
        );

        let props = props(&map);

        assert!(props.lifecycle.locked_open, "persistent ⇒ locked open");
        assert!(props.presentation.realert_on_replace, "show-as-new ⇒ re-alert");
        assert_eq!(
            props.presentation.lockscreen,
            LockscreenVisibility::HideContent
        );
        // Not requested ⇒ defaults.
        assert_eq!(props.presentation.routing, Routing::Banner);
        assert!(!props.classification.transient);
    }
}
