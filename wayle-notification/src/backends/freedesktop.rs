use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use derive_more::Debug;
use tokio::sync::broadcast;
use tracing::{debug, instrument, warn};
use wayle_core::Property;
use zbus::{
    Connection, fdo,
    zvariant::{OwnedValue, Str, Value},
};

use crate::{
    core::{
        backend::{Backend, DispatchCtx},
        notification::{Notification, open_uri_via_portal},
        types::{
            Action, ActionId, Actions, Alert, BorrowedImageData, Classification, Content,
            DesktopEntryId, FreedesktopDispatch, Image, IncomingHints, Lifecycle,
            NotificationHints, NotificationId, NotificationProps, NotificationSource, Origin,
            Presentation, Reply, SoundName, Timeout, Uri,
        },
    },
    error::Error,
    events::NotificationEvent,
    image_cache,
    types::{
        Capabilities, ClosedReason, Name, Priority, Signal, SpecVersion, Urgency, Vendor, Version,
        dbus::{SERVICE_INTERFACE, SERVICE_PATH},
    },
};
use super::glob;

/// The action key that requests KDE's inline text reply (the label is app-chosen). It is not
/// shown as a normal button and its result returns via `NotificationReplied`, not
/// `ActionInvoked`.
const INLINE_REPLY_ACTION: &str = "inline-reply";

#[derive(Debug)]
pub(crate) struct FdoNotificationDaemon {
    /// Session-scoped freedesktop wire-id allocator (see [`Self::next_wire_id`]).
    #[debug(skip)]
    pub wire_counter: AtomicU32,
    /// The current session bus GUID, stamped onto each notification's dispatch so its wire id
    /// and owner can be scoped to this session across a restart.
    #[debug(skip)]
    pub session_id: String,
    #[debug(skip)]
    pub zbus_connection: Connection,
    #[debug(skip)]
    pub notif_tx: broadcast::Sender<NotificationEvent>,
    #[debug(skip)]
    pub blocklist: Property<Vec<String>>,
    /// Live wire id → its notification identity and owning app name. Lets a `replaces_id`
    /// resolve to the identity it supersedes (and gates the ownership check), and
    /// `CloseNotification(wire_id)` map back to the identity to remove.
    #[debug(skip)]
    pub wire_slots: Mutex<HashMap<u32, WireSlot>>,
    /// `(app, stack_tag)` → the identity holding that coalescing slot, so a same-tagged
    /// notification reuses it (replaces the previous one in place).
    #[debug(skip)]
    pub stack_slots: Mutex<HashMap<(String, String), NotificationId>>,
}

/// The identity and owner behind a live freedesktop wire id.
#[derive(Debug)]
pub(crate) struct WireSlot {
    pub guid: NotificationId,
    pub app: String,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl FdoNotificationDaemon {
    #[allow(clippy::too_many_arguments)]
    #[instrument(
        skip(self, actions, hints, header),
        fields(
            app = %app_name,
            replaces = %replaces_id,
            timeout = %expire_timeout
        )
    )]
    pub fn notify(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: IncomingHints<'_>,
        expire_timeout: i32,
    ) -> fdo::Result<u32> {
        let hints = normalize_hints(hints);
        let stack_tag = stack_tag_from_hints(&hints);
        let (guid, wire_id) = self.resolve(replaces_id, &app_name, stack_tag.as_deref());

        let blocked = self
            .blocklist
            .get()
            .iter()
            .any(|pattern| glob::matches(pattern, &app_name));

        if blocked {
            debug!(app = %app_name, "notification blocked by blocklist");
            return Ok(wire_id);
        }

        let owner = header.sender().map(|sender| sender.to_string());

        let props = build_props(FdoNotify {
            guid,
            wire_id,
            session_id: self.session_id.clone(),
            timestamp: Utc::now(),
            owner,
            app_name,
            app_icon,
            summary,
            body,
            actions,
            hints,
            expire_timeout,
        });

        let notif = Notification::new(props, self.zbus_connection.clone(), self.notif_tx.clone());
        if self.notif_tx.send(NotificationEvent::Add(Box::new(notif))).is_err() {
            warn!("notification pipeline has no receiver; dropped an incoming notification");
        }

        Ok(wire_id)
    }

    #[instrument(skip(self), fields(wire_id = %id))]
    pub async fn close_notification(&self, id: u32) -> fdo::Result<()> {
        // The app closes by wire id; map it back to the notification identity to remove.
        let guid = self
            .wire_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id)
            .map(|slot| slot.guid);
        if let Some(guid) = guid {
            let _ = self
                .notif_tx
                .send(NotificationEvent::Remove(guid, ClosedReason::Closed));
        }
        Ok(())
    }

    pub async fn get_capabilities(&self) -> Vec<String> {
        // Advertise every capability the daemon parses into the notification model, independent
        // of what wayle-shell renders today: an app negotiates against what the *server* accepts,
        // so anything the daemon captures (and can later hand the shell) is an honest capability.
        // `icon-multi` is the sole omission — it is mutually exclusive with `icon-static` and the
        // daemon keeps only a single frame.
        vec![
            Capabilities::Body.to_string(),
            // The shell renders the body as Pango markup, and GTK's `GtkLabel` opens `<a href>`
            // links via its built-in `activate-link` handler (→ portal `OpenURI` on Wayland); the
            // daemon also lifts the `x-kde-urls` hint into the model.
            Capabilities::BodyMarkup.to_string(),
            Capabilities::BodyHyperlinks.to_string(),
            Capabilities::Actions.to_string(),
            // The daemon parses the `action-icons` hint into `Actions::action_icons`; advertised
            // so apps may mark their actions icon-only even though the shell doesn't render
            // icon-only actions yet (until then it falls back to the action labels).
            Capabilities::ActionIcons.to_string(),
            Capabilities::IconStatic.to_string(),
            // `image-data`/`image-path` are materialized to a cached file the model carries.
            Capabilities::BodyImages.to_string(),
            Capabilities::Persistence.to_string(),
            // The daemon parses `sound-file`/`sound-name`/`suppress-sound` into `Alert`;
            // advertised so apps defer playback to the server even though the shell doesn't play
            // audio yet.
            Capabilities::Sound.to_string(),
            // KDE inline-reply extension: apps gate on this capability string before adding
            // an `inline-reply` action.
            String::from(INLINE_REPLY_ACTION),
        ]
    }

    pub async fn get_server_information(&self) -> (Name, Vendor, Version, SpecVersion) {
        let name = String::from("wayle");
        let vendor = String::from("wayle");
        let version = String::from(env!("CARGO_PKG_VERSION"));
        let spec_version = String::from("1.3");

        (name, vendor, version, spec_version)
    }
}

impl FdoNotificationDaemon {
    /// Builds the daemon, re-establishing the freedesktop wire-id space from the notifications
    /// restored for the current session: seeds the coalescing slots (so an app can still
    /// replace/close them), resumes the wire counter above their max (so a fresh id never
    /// collides with a live one), and clears the owner of any notification from a PRIOR session
    /// (its connection is gone — it survives as history, but its actions can't be dispatched).
    pub(crate) fn new(
        connection: Connection,
        notif_tx: broadcast::Sender<NotificationEvent>,
        blocklist: Property<Vec<String>>,
        session_id: String,
        restored: &[Arc<Notification>],
    ) -> Self {
        let mut wire_slots: HashMap<u32, WireSlot> = HashMap::new();
        let mut stack_slots: HashMap<(String, String), NotificationId> = HashMap::new();
        let mut max_wire_id = 0u32;
        for notif in restored {
            let NotificationSource::Freedesktop(dispatch) = notif.dispatch.get() else {
                continue;
            };
            if dispatch.session_id != session_id {
                // Prior-session owner is gone: clear it so the actions survive as history but are
                // undispatchable (the reachability seed then strips them). Owner lives inside the
                // dispatch, so rebuild it with `owner: None`.
                notif.dispatch.replace(NotificationSource::Freedesktop(FreedesktopDispatch {
                    owner: None,
                    ..dispatch
                }));
                continue;
            }
            max_wire_id = max_wire_id.max(dispatch.wire_id);
            if let Some(app) = notif.view.get().origin.name {
                wire_slots.insert(
                    dispatch.wire_id,
                    WireSlot {
                        guid: notif.id,
                        app: app.clone(),
                    },
                );
                if let Some(tag) = notif.view.get().classification.stack_tag {
                    stack_slots.insert((app, tag), notif.id);
                }
            }
        }

        Self {
            wire_counter: AtomicU32::new(max_wire_id.saturating_add(1)),
            session_id,
            zbus_connection: connection,
            notif_tx,
            blocklist,
            wire_slots: Mutex::new(wire_slots),
            stack_slots: Mutex::new(stack_slots),
        }
    }

    /// Resolves the identity and wire id for an incoming `Notify`, honoring in priority order:
    ///
    /// 1. an app-owned `replaces_id` — reuse both the identity and the wire id (a normal
    ///    self-replace, where the app deliberately reuses the id it was given);
    /// 2. a `stack_tag` slot the app has used before — reuse the identity but issue a *fresh*
    ///    wire id, so the app still gets its own D-Bus id per call while a shell sees the
    ///    supersede as an in-place update of one card;
    /// 3. otherwise a fresh identity and wire id.
    ///
    /// Records the resulting slot so later `replaces_id`/`stack_tag` calls coalesce onto it and
    /// `CloseNotification` can map the wire id back to the identity.
    fn resolve(
        &self,
        replaces_id: u32,
        app_name: &str,
        stack_tag: Option<&str>,
    ) -> (NotificationId, u32) {
        let (guid, wire_id) = if replaces_id != 0 && self.owns_wire(replaces_id, app_name) {
            let guid = self
                .wire_slots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&replaces_id)
                .map_or_else(NotificationId::generate, |slot| slot.guid);
            debug!(replaces_id, guid = %guid, "reusing replaces_id owned by same app");
            (guid, replaces_id)
        } else if let Some(guid) = stack_tag.and_then(|tag| self.stack_slot(app_name, tag)) {
            let wire_id = self.next_wire_id();
            debug!(?stack_tag, guid = %guid, wire_id, "coalescing onto existing stack-tag slot");
            (guid, wire_id)
        } else {
            let guid = NotificationId::generate();
            let wire_id = self.next_wire_id();
            debug!(guid = %guid, wire_id, "assigned fresh identity");
            (guid, wire_id)
        };

        self.record_slot(wire_id, guid, app_name, stack_tag);
        (guid, wire_id)
    }

    /// Whether `wire_id` currently belongs to a notification created by `app_name`.
    fn owns_wire(&self, wire_id: u32, app_name: &str) -> bool {
        self.wire_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&wire_id)
            .is_some_and(|slot| slot.app == app_name)
    }

    /// The identity holding `app_name`'s `stack_tag` slot, if any.
    fn stack_slot(&self, app_name: &str, tag: &str) -> Option<NotificationId> {
        self.stack_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&(app_name.to_owned(), tag.to_owned()))
            .copied()
    }

    /// Records the resolved wire slot (and stack-tag slot, if any) for future coalescing.
    fn record_slot(
        &self,
        wire_id: u32,
        guid: NotificationId,
        app_name: &str,
        stack_tag: Option<&str>,
    ) {
        self.wire_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                wire_id,
                WireSlot {
                    guid,
                    app: app_name.to_owned(),
                },
            );
        if let Some(tag) = stack_tag {
            self.stack_slots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert((app_name.to_owned(), tag.to_owned()), guid);
        }
    }

    /// Issues the next session-scoped freedesktop wire id.
    fn next_wire_id(&self) -> u32 {
        self.wire_counter.fetch_add(1, Ordering::Relaxed)
    }
}

fn normalize_hints(hints: IncomingHints<'_>) -> NotificationHints {
    normalize_hints_with(hints, image_cache::cache_borrowed_image)
}

fn normalize_hints_with<F>(hints: IncomingHints<'_>, cache_image: F) -> NotificationHints
where
    F: FnOnce(BorrowedImageData<'_>) -> Option<String>,
{
    let cached_path = hints.image_data().and_then(cache_image);
    let mut normalized = hints.into_owned();

    if let Some(cached_path) = cached_path {
        normalized.insert(
            String::from("image-path"),
            OwnedValue::from(Str::from(cached_path)),
        );
    }

    normalized
}

/// The decoded arguments of a `Notify` call, ready to translate into facets.
struct FdoNotify {
    /// The notification identity (already resolved for coalescing).
    guid: NotificationId,
    /// The freedesktop D-Bus wire id returned to the app.
    wire_id: u32,
    /// The session bus GUID this notification was created under.
    session_id: String,
    timestamp: DateTime<Utc>,
    owner: Option<String>,
    app_name: String,
    app_icon: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    hints: NotificationHints,
    expire_timeout: i32,
}

#[async_trait]
impl Backend for FreedesktopDispatch {
    async fn dispatch_action(&self, ctx: &DispatchCtx<'_>, action: &ActionId) -> Result<(), Error> {
        // A URL notification's body click opens its primary link (matching KDE's
        // KIO::OpenUrlJob → xdg-open → portal OpenURI), taking precedence over any app default
        // action — a URL notification is "about" opening the link.
        if action.is_default() && let Some(uri) = ctx.urls.first() {
            return open_uri_via_portal(ctx.connection, uri.as_str()).await;
        }
        // Direct the ActionInvoked signal to the owning connection. If the owner is unknown,
        // skip emission rather than broadcasting: a broadcast lets clients that don't filter by
        // id react to notifications they didn't create.
        let Some(owner) = self.owner.as_deref() else {
            return Ok(());
        };
        // Per the spec, the focus token is emitted immediately BEFORE ActionInvoked (same id),
        // directed to the owner like ActionInvoked, so the app can raise its window on the click.
        if let Some(token) = ctx.activation_token {
            ctx.connection
                .emit_signal(
                    Some(owner),
                    SERVICE_PATH,
                    SERVICE_INTERFACE,
                    Signal::ActivationToken.as_str(),
                    &(self.wire_id, token),
                )
                .await?;
        }
        ctx.connection
            .emit_signal(
                Some(owner),
                SERVICE_PATH,
                SERVICE_INTERFACE,
                Signal::ActionInvoked.as_str(),
                &(self.wire_id, action.as_str()),
            )
            .await?;
        Ok(())
    }

    /// KDE-style inline reply: `NotificationReplied(id, text)` directed to the owning
    /// connection. Per the KDE protocol the reply is delivered *solely* via this signal.
    async fn reply(&self, ctx: &DispatchCtx<'_>, text: &str) -> Result<bool, Error> {
        // Without a known owner the directed reply can't be delivered (and must not broadcast).
        let Some(owner) = self.owner.as_deref() else {
            return Ok(false);
        };
        ctx.connection
            .emit_signal(
                Some(owner),
                SERVICE_PATH,
                SERVICE_INTERFACE,
                Signal::NotificationReplied.as_str(),
                &(self.wire_id, text),
            )
            .await?;
        Ok(true)
    }

    /// Directs `NotificationClosed(wire_id, reason)` to the owning connection (skipped, `Ok`,
    /// when the owner is unknown — the same no-broadcast rule as `dispatch_action`). GTK/portal
    /// have no close-back signal, so only this backend overrides the default no-op.
    async fn close(&self, connection: &Connection, reason: ClosedReason) -> Result<(), Error> {
        let Some(owner) = self.owner.as_deref() else {
            return Ok(());
        };
        connection
            .emit_signal(
                Some(owner),
                SERVICE_PATH,
                SERVICE_INTERFACE,
                Signal::NotificationClosed.as_str(),
                &(self.wire_id, reason as u32),
            )
            .await?;
        Ok(())
    }

    /// The owner's unique name is the only dispatch target; if it's gone from the bus (or was
    /// never known), the actions can never fire again.
    fn is_unreachable(&self, live: &HashSet<String>, _activatable: &HashSet<String>) -> bool {
        match self.owner.as_deref() {
            Some(owner) => !live.contains(owner),
            None => true,
        }
    }

    fn dispatch_target(&self) -> Option<&str> {
        self.owner.as_deref()
    }
}

/// Translates a freedesktop `Notify` into the unified facet model. This is the *only* place
/// the fdo hint vocabulary (urgency, category, image-path, sound-*, transient, …) is read;
/// unrecognized hints are intentionally dropped (there is no untyped escape hatch).
fn build_props(notify: FdoNotify) -> NotificationProps {
    let FdoNotify {
        guid,
        wire_id,
        session_id,
        timestamp,
        owner,
        app_name,
        app_icon,
        summary,
        body,
        actions,
        hints,
        expire_timeout,
    } = notify;

    // `x-kde-display-appname` overrides the shown app name; `x-kde-origin-name` is a secondary
    // attribution line (the account/device/contact a proxied notification came from).
    let name = hint_string(&hints, "x-kde-display-appname")
        .filter(|name| !name.is_empty())
        .or_else(|| (!app_name.is_empty()).then_some(app_name));
    let origin = Origin {
        name,
        origin_name: hint_string(&hints, "x-kde-origin-name"),
        desktop_entry: hint_string(&hints, "desktop-entry").map(DesktopEntryId::new),
        icon: classify_icon(app_icon),
    };

    // The server advertises the `body-markup` capability, so fdo bodies may contain markup.
    let content = Content {
        summary,
        body: (!body.is_empty()).then(|| crate::core::types::Body::Markup(body)),
        progress: hint_progress(&hints),
    };

    // A large content image (or a cache path materialized from `image-data`); always a file.
    // Accept the deprecated spec-1.0 underscore spelling `image_path` as a fallback.
    let image = hint_string(&hints, "image-path")
        .or_else(|| hint_string(&hints, "image_path"))
        .filter(|path| !path.is_empty())
        .map(|path| {
            let trimmed = path.strip_prefix("file://").unwrap_or(&path);
            Image::Path(PathBuf::from(trimmed))
        });

    // Attached links (KDE `x-kde-urls`): openable, and the body click opens the first.
    let urls = hint_urls(&hints);

    // Flat `[id, label, ...]`; the `"default"` entry (if any) is the body-click action. When
    // the notification carries URLs but no app default, synthesize one so the body is
    // clickable (its click opens the primary link — see `Notification::invoke`).
    let mut parsed = Action::parse_dbus_actions(&actions);
    let default = parsed
        .iter()
        .find(|action| action.id.is_default())
        .cloned()
        .or_else(|| {
            (!urls.is_empty()).then(|| Action {
                id: ActionId::default_action(),
                label: String::new(),
                purpose: None,
            })
        });
    // KDE inline reply: an `inline-reply` action requests a text field (customized by the
    // x-kde-reply-* hints). It is neither the default nor a normal button, and its result
    // comes back via `NotificationReplied`.
    let reply = parsed
        .iter()
        .any(|action| action.id.as_str() == INLINE_REPLY_ACTION)
        .then(|| Reply {
            placeholder: hint_string(&hints, "x-kde-reply-placeholder-text"),
            submit_label: hint_string(&hints, "x-kde-reply-submit-button-text"),
            submit_icon: hint_string(&hints, "x-kde-reply-submit-button-icon-name"),
        });
    parsed
        .retain(|action| !action.id.is_default() && action.id.as_str() != INLINE_REPLY_ACTION);
    let actions = Actions {
        default,
        buttons: parsed,
        reply,
        urls,
        action_icons: hint_bool(&hints, "action-icons"),
    };

    let alert = if hint_bool(&hints, "suppress-sound") {
        Alert::Silent
    } else if let Some(file) = hint_string(&hints, "sound-file") {
        Alert::File(PathBuf::from(file))
    } else if let Some(name) = hint_string(&hints, "sound-name") {
        Alert::Named(SoundName::new(name))
    } else {
        Alert::Unspecified
    };

    // The 3-level urgency hint is lifted onto the 4-level scale (absent ⇒ Normal, never High).
    let priority: Priority = hints
        .get("urgency")
        .and_then(|hint| hint.downcast_ref::<u8>().ok())
        .map_or(Urgency::Normal, Urgency::from)
        .into();
    let stack_tag = stack_tag_from_hints(&hints);
    let classification = Classification {
        priority,
        category: hint_string(&hints, "category").and_then(|category| category.parse().ok()),
        transient: hint_bool(&hints, "transient"),
        resident: hint_bool(&hints, "resident"),
        stack_tag,
    };

    let lifecycle = Lifecycle {
        timeout: resolve_timeout(expire_timeout, priority),
        // freedesktop has no "persistent" concept — the close affordance is always available.
        locked_open: false,
    };

    // Standard freedesktop notifications carry no display-hint taxonomy: banner, shown on the
    // lock screen, no re-alert semantics.
    let presentation = Presentation::default();

    // Unmodeled hints are intentionally dropped, not carried in an escape hatch — anything a
    // shell should act on gets a typed facet. Notably the spec's `x`/`y` "point-to-screen-
    // location" hints are ignored: they are an X11-era artifact with no valid Wayland use
    // (a Wayland client cannot know a global screen coordinate to point at), and Wayle only
    // targets Wayland. If a hint ever proves worth supporting, model it as a facet.

    NotificationProps {
        id: guid,
        timestamp,
        dispatch: NotificationSource::Freedesktop(FreedesktopDispatch {
            wire_id,
            session_id,
            owner,
        }),
        origin,
        content,
        image,
        actions,
        alert,
        classification,
        lifecycle,
        presentation,
    }
}

/// Extracts the coalescing stack tag from the de-facto hints apps use for it (dunst's tag, and
/// Canonical's synchronous-OSD keys). Shared by ingest (for the facet) and id resolution.
fn stack_tag_from_hints(hints: &NotificationHints) -> Option<String> {
    hint_string(hints, "x-dunst-stack-tag")
        .or_else(|| hint_string(hints, "x-canonical-private-synchronous"))
        .or_else(|| hint_string(hints, "synchronous"))
}

fn hint_bool(hints: &NotificationHints, key: &str) -> bool {
    hints
        .get(key)
        .and_then(|hint| hint.downcast_ref::<bool>().ok())
        .unwrap_or(false)
}

fn hint_string(hints: &NotificationHints, key: &str) -> Option<String> {
    hints
        .get(key)
        .and_then(|hint| hint.downcast_ref::<String>().ok())
}

/// Reads the KDE `x-kde-urls` hint (a string array `as`) into typed URIs.
fn hint_urls(hints: &NotificationHints) -> Vec<Uri> {
    let Some(Value::Array(array)) = hints.get("x-kde-urls").map(|value| &**value) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|element| match element {
            Value::Str(url) => Some(Uri::new(url.as_str())),
            _ => None,
        })
        .collect()
}

/// Reads the de-facto `value` progress hint (sent as `int` or `uint`), clamped to a 0..=100
/// percentage. This is the OSD progress bar (volume/brightness/transfers).
fn hint_progress(hints: &NotificationHints) -> Option<u8> {
    let value = hints.get("value")?;
    let raw = value
        .downcast_ref::<i32>()
        .ok()
        .or_else(|| value.downcast_ref::<u32>().ok().and_then(|v| i32::try_from(v).ok()))?;
    Some(raw.clamp(0, 100) as u8)
}

/// Maps the fdo `expire_timeout` (an overloaded signed int) to a [`Timeout`]: `>0` ⇒ auto-close
/// after that many ms; `0` ⇒ never expire; `<0` ⇒ the server default — except a Critical
/// (⇒ [`Priority::Urgent`]) notification under the server default becomes [`Timeout::NeverByApp`],
/// since the urgency-levels spec says critical notifications "should only be closed when the user
/// dismisses them" and a shell-chosen default must not silently dismiss one. An explicit positive
/// timeout from the app is always honored.
fn resolve_timeout(expire_timeout: i32, priority: Priority) -> Timeout {
    match expire_timeout {
        timeout if timeout > 0 => Timeout::After(Duration::from_millis(timeout as u64)),
        0 => Timeout::NeverByApp,
        _ if priority == Priority::Urgent => Timeout::NeverByApp,
        _ => Timeout::ServerDefault,
    }
}

/// Classifies the fdo `app_icon` string as a themed name or a filesystem path.
fn classify_icon(value: String) -> Option<Image> {
    if value.is_empty() {
        None
    } else if let Some(path) = value.strip_prefix("file://") {
        Some(Image::Path(PathBuf::from(path)))
    } else if value.starts_with('/') {
        Some(Image::Path(PathBuf::from(value)))
    } else {
        Some(Image::Named(value))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use zbus::zvariant::{LE, Value, serialized::Context, to_bytes};

    use super::*;
    use crate::core::types::Body;

    #[test]
    fn normalize_hints_replaces_image_data_with_cached_path() {
        let pixels = [0u8, 1, 2, 3];
        let mut raw = HashMap::new();
        raw.insert("category", Value::new("im.received"));
        raw.insert(
            "image-data",
            Value::new((1i32, 1i32, 4i32, true, 8i32, 4i32, &pixels[..])),
        );
        let encoded = to_bytes(Context::new_dbus(LE, 0), &raw).expect("hints should encode");
        let (hints, _) = encoded
            .deserialize::<IncomingHints<'_>>()
            .expect("hints should decode");

        let normalized = normalize_hints_with(hints, |_| Some(String::from("/tmp/fake.png")));

        assert!(normalized.contains_key("category"));
        assert!(!normalized.contains_key("image-data"));
        assert_eq!(
            normalized
                .get("image-path")
                .and_then(|value| value.downcast_ref::<String>().ok())
                .as_deref(),
            Some("/tmp/fake.png")
        );
    }

    #[test]
    fn normalize_hints_discards_malformed_image_data() {
        let mut raw = HashMap::new();
        raw.insert("urgency", Value::new(1u8));
        raw.insert("image-data", Value::new("not-an-image"));
        let encoded = to_bytes(Context::new_dbus(LE, 0), &raw).expect("hints should encode");

        assert!(
            encoded.deserialize::<IncomingHints<'_>>().is_err(),
            "invalid image-data should be rejected at the D-Bus boundary"
        );
    }

    // --- facet translation ---------------------------------------------------------------

    fn owned(value: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(value).expect("value converts to OwnedValue")
    }

    /// Builds facets from a `Notify`-like call, defaulting the metadata a shell ignores.
    fn props(
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: NotificationHints,
        expire_timeout: i32,
    ) -> NotificationProps {
        build_props(FdoNotify {
            guid: NotificationId::new(1),
            wire_id: 1,
            session_id: String::from("test-session"),
            timestamp: Utc::now(),
            owner: Some(String::from(":1.42")),
            app_name: String::from("Mail"),
            app_icon: String::from(app_icon),
            summary: String::from(summary),
            body: String::from(body),
            actions: actions.iter().map(|action| (*action).to_owned()).collect(),
            hints,
            expire_timeout,
        })
    }

    #[test]
    fn realistic_email_notification_maps_all_facets() {
        let mut hints = NotificationHints::new();
        hints.insert(String::from("urgency"), owned(Value::U8(2)));
        hints.insert(String::from("category"), owned(Value::new("email.arrived")));
        hints.insert(
            String::from("desktop-entry"),
            owned(Value::new("org.gnome.Evolution")),
        );
        hints.insert(String::from("resident"), owned(Value::Bool(true)));

        let props = props(
            "mail-unread",
            "New mail from Alice",
            "Subject: <b>Lunch?</b>",
            &["default", "", "reply", "Reply", "archive", "Archive"],
            hints,
            0,
        );

        // Origin: name / trusted desktop id / themed header icon.
        assert_eq!(props.origin.name.as_deref(), Some("Mail"));
        assert_eq!(
            props.origin.desktop_entry.as_ref().map(DesktopEntryId::as_str),
            Some("org.gnome.Evolution")
        );
        assert_eq!(props.origin.icon, Some(Image::Named(String::from("mail-unread"))));

        // Content: fdo body is markup.
        assert_eq!(props.content.summary, "New mail from Alice");
        assert_eq!(
            props.content.body,
            Some(Body::Markup(String::from("Subject: <b>Lunch?</b>")))
        );

        // Actions: the "default" entry is the body click; the rest are buttons.
        assert!(props.actions.default.as_ref().is_some_and(|action| action.id.is_default()));
        let button_ids: Vec<_> = props.actions.buttons.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(button_ids, vec!["reply", "archive"]);
        assert_eq!(props.actions.buttons[0].label, "Reply");

        // Classification: urgency 2 ⇒ Urgent; category parsed; resident carried.
        assert_eq!(props.classification.priority, Priority::Urgent);
        assert_eq!(props.classification.category, Some(crate::types::Category::EmailArrived));
        assert!(props.classification.resident);
        assert!(!props.classification.transient);

        // Lifecycle: expire_timeout 0 ⇒ never by app.
        assert_eq!(props.lifecycle.timeout, Timeout::NeverByApp);

        assert!(matches!(props.dispatch, NotificationSource::Freedesktop(_)));
    }

    #[test]
    fn urgency_lifts_onto_four_level_scale_never_high() {
        let priority = |urgency: Option<u8>| {
            let mut hints = NotificationHints::new();
            if let Some(level) = urgency {
                hints.insert(String::from("urgency"), owned(Value::U8(level)));
            }
            props("", "s", "", &[], hints, -1).classification.priority
        };

        assert_eq!(priority(Some(0)), Priority::Low);
        assert_eq!(priority(Some(1)), Priority::Normal);
        assert_eq!(priority(Some(2)), Priority::Urgent);
        assert_eq!(priority(None), Priority::Normal, "absent urgency ⇒ Normal");
    }

    #[test]
    fn timeout_maps_from_expire_timeout() {
        let timeout = |expire: i32| props("", "s", "", &[], NotificationHints::new(), expire).lifecycle.timeout;
        assert_eq!(timeout(-1), Timeout::ServerDefault);
        assert_eq!(timeout(0), Timeout::NeverByApp);
        assert_eq!(timeout(5000), Timeout::After(Duration::from_millis(5000)));
    }

    #[test]
    fn transient_and_sound_and_image_facets() {
        let mut hints = NotificationHints::new();
        hints.insert(String::from("transient"), owned(Value::Bool(true)));
        hints.insert(String::from("suppress-sound"), owned(Value::Bool(true)));
        hints.insert(
            String::from("image-path"),
            owned(Value::new("file:///tmp/pic.png")),
        );

        let props = props("", "s", "", &[], hints, -1);

        assert!(props.classification.transient);
        assert_eq!(props.alert, Alert::Silent);
        assert_eq!(props.image, Some(Image::Path(PathBuf::from("/tmp/pic.png"))));
    }

    #[test]
    fn sound_file_and_name_become_typed_alert() {
        let mut file_hints = NotificationHints::new();
        file_hints.insert(String::from("sound-file"), owned(Value::new("/snd/a.oga")));
        assert_eq!(
            props("", "s", "", &[], file_hints, -1).alert,
            Alert::File(PathBuf::from("/snd/a.oga"))
        );

        let mut name_hints = NotificationHints::new();
        name_hints.insert(String::from("sound-name"), owned(Value::new("message-new")));
        assert_eq!(
            props("", "s", "", &[], name_hints, -1).alert,
            Alert::Named(SoundName::new("message-new"))
        );
    }

    #[test]
    fn absolute_app_icon_is_a_path_themed_name_is_named() {
        assert_eq!(
            props("/usr/share/icons/x.png", "s", "", &[], NotificationHints::new(), -1)
                .origin
                .icon,
            Some(Image::Path(PathBuf::from("/usr/share/icons/x.png")))
        );
        assert_eq!(
            props("firefox", "s", "", &[], NotificationHints::new(), -1)
                .origin
                .icon,
            Some(Image::Named(String::from("firefox")))
        );
    }

    #[test]
    fn kde_inline_reply_and_origin_hints() {
        let mut hints = NotificationHints::new();
        hints.insert(
            String::from("x-kde-reply-placeholder-text"),
            owned(Value::new("Type a reply...")),
        );
        hints.insert(
            String::from("x-kde-reply-submit-button-text"),
            owned(Value::new("Send")),
        );
        hints.insert(
            String::from("x-kde-origin-name"),
            owned(Value::new("#general:matrix.org")),
        );
        hints.insert(String::from("x-kde-display-appname"), owned(Value::new("Element")));

        let props = props(
            "",
            "New message",
            "hi",
            &["inline-reply", "Reply", "mute", "Mute"],
            hints,
            -1,
        );

        // The inline-reply action becomes a typed reply affordance, not a button.
        let reply = props.actions.reply.expect("inline-reply ⇒ reply affordance");
        assert_eq!(reply.placeholder.as_deref(), Some("Type a reply..."));
        assert_eq!(reply.submit_label.as_deref(), Some("Send"));
        let button_ids: Vec<_> = props.actions.buttons.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(button_ids, vec!["mute"], "inline-reply excluded from buttons");

        // Origin: x-kde-display-appname overrides the name; x-kde-origin-name is the 2nd line.
        assert_eq!(props.origin.name.as_deref(), Some("Element"));
        assert_eq!(props.origin.origin_name.as_deref(), Some("#general:matrix.org"));
    }

    #[test]
    fn x_kde_urls_are_exposed_and_make_body_clickable() {
        let mut hints = NotificationHints::new();
        hints.insert(
            String::from("x-kde-urls"),
            owned(Value::new(vec![
                String::from("file:///home/me/shot.png"),
                String::from("https://example.com/x"),
            ])),
        );

        // No app action at all, but URLs are present.
        let props = props("", "Screenshot saved", "", &[], hints, -1);

        let urls: Vec<_> = props.actions.urls.iter().map(Uri::as_str).collect();
        assert_eq!(urls, vec!["file:///home/me/shot.png", "https://example.com/x"]);
        // The body is clickable (a synthetic default) so a click can open the first URL,
        // even though the app declared no action.
        assert!(
            props.actions.default.as_ref().is_some_and(|action| action.id.is_default()),
            "URLs make the body clickable even with no app action"
        );
    }

    #[test]
    fn value_hint_becomes_clamped_progress() {
        let progress = |value: Value<'_>| {
            let mut hints = NotificationHints::new();
            hints.insert(String::from("value"), owned(value));
            props("", "s", "", &[], hints, -1).content.progress
        };
        assert_eq!(progress(Value::I32(42)), Some(42));
        assert_eq!(progress(Value::U32(80)), Some(80), "uint is accepted too");
        assert_eq!(progress(Value::I32(150)), Some(100), "clamped to 100");
        assert_eq!(progress(Value::I32(-10)), Some(0), "clamped to 0");
        assert_eq!(
            props("", "s", "", &[], NotificationHints::new(), -1)
                .content
                .progress,
            None,
            "absent value ⇒ no progress"
        );
    }
}
