use std::{collections::HashMap, fmt, path::PathBuf, time::Duration};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zbus::zvariant::{OwnedValue, Type, as_value::optional};

use crate::{
    core::backend::Backend,
    types::{ButtonPurpose, Category, Priority},
};

/// A globally-unique notification identity — the stable key a shell uses to reconcile and
/// address a card, independent of any D-Bus wire id. Randomly generated per notification
/// rather than sequential, so there is no counter to persist: two notifications never share
/// an identity by construction, and coalescing (`replaces_id` / `stack_tag`) deliberately
/// reuses one existing identity so a shell sees the supersede as an in-place update.
///
/// This is NOT a wire value. The freedesktop backend's `u32` D-Bus id lives separately in
/// `FreedesktopDispatch::wire_id`; GTK and portal address the app by their string keys.
///
/// Backed by `i64` (not `u64`) to match SQLite's signed `INTEGER` column, so persistence
/// stores it directly with no bit-cast — while still using the full 64 bits of entropy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NotificationId(i64);

impl NotificationId {
    /// Wraps a raw identity value (persistence / tests).
    pub fn new(id: i64) -> Self {
        Self(id)
    }

    /// Generates a fresh random identity.
    ///
    /// Uses the OS-seeded [`RandomState`](std::collections::hash_map::RandomState) hasher
    /// (fresh keys per call) for entropy — no extra dependency, and no persisted high-water
    /// mark, since identities are random rather than sequential. Full 64-bit range (the value
    /// may be negative); collision probability across a session's notifications is negligible.
    pub(crate) fn generate() -> Self {
        use std::{collections::hash_map::RandomState, hash::BuildHasher};
        Self(RandomState::new().hash_one(()) as i64)
    }

    /// The raw identity value (persistence / logging).
    pub fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for NotificationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A trusted, locale-independent `.desktop` entry id (e.g. `org.gnome.Clocks`) — the
/// icon-lookup / grouping / per-app key. Distinct from the spoofable display name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DesktopEntryId(String);

impl DesktopEntryId {
    /// Wraps a desktop-entry id.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An app-defined action id, guaranteed by construction never to be the reserved
/// body-click key (so a button can never collide with the default action). The default
/// action uses [`ActionId::default_action`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionId(String);

/// Returned by [`ActionId::new`] when given the reserved `"default"` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservedActionId;

impl fmt::Display for ReservedActionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "\"{}\" is a reserved action id", ActionId::DEFAULT_KEY)
    }
}

impl ActionId {
    /// The reserved key naming the body-click / default action.
    pub const DEFAULT_KEY: &str = "default";

    /// Wraps an app action id, rejecting the reserved [`DEFAULT_KEY`](Self::DEFAULT_KEY).
    ///
    /// # Errors
    /// Returns [`ReservedActionId`] if `id` is `"default"`.
    pub fn new(id: impl Into<String>) -> Result<Self, ReservedActionId> {
        let id = id.into();
        if id == Self::DEFAULT_KEY {
            Err(ReservedActionId)
        } else {
            Ok(Self(id))
        }
    }

    /// The reserved default (body-click) action id.
    pub fn default_action() -> Self {
        Self(String::from(Self::DEFAULT_KEY))
    }

    /// Whether this is the reserved default (body-click) action.
    pub fn is_default(&self) -> bool {
        self.0 == Self::DEFAULT_KEY
    }

    /// The id as a string slice (the dispatch key / wire action name).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A URI attached to a notification (a freedesktop `x-kde-urls` entry) — e.g. a screenshot
/// file, a downloaded file, or a link. Internal to the crate's dispatch: a URL notification's
/// body click opens its primary link (see [`Actions::urls`]); the shell never handles URIs
/// directly.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Uri(String);

impl Uri {
    /// Wraps a URI string.
    pub fn new(uri: impl Into<String>) -> Self {
        Self(uri.into())
    }
    /// The URI as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a notification interaction originated, passed to
/// [`invoke`](crate::core::notification::Notification::invoke) /
/// [`activate_default`](crate::core::notification::Notification::activate_default) /
/// [`reply`](crate::core::notification::Notification::reply) so the notification dismisses
/// itself appropriately. A `resident` notification (media controls) is never auto-dismissed,
/// whatever the source.
#[derive(Debug, Clone, Copy)]
pub enum InvokeSource {
    /// Invoked from the history list: the notification is removed from history on activation.
    History,
    /// Invoked from a popup banner: the banner is always dismissed on activation, and the
    /// notification is also removed from history when `remove_from_history` — the shell's
    /// per-popup close policy (keep-in-history vs remove-entirely). When `false`, it stays in
    /// history (only the banner goes).
    Popup {
        /// Whether to also remove it from history (vs. keeping it there, banner-only close).
        remove_from_history: bool,
    },
}

/// A fully-translated notification ready to become a [`Notification`].
///
/// Every backend adapter builds these typed facets from its own wire format (fdo hints, or
/// the GNotification/portal `a{sv}`), so `Notification::from_props` just wraps them reactively
/// — no protocol-specific translation remains in the core. This is the seam that keeps the
/// three backends' logic entirely inside their own modules.
#[derive(Debug, Clone)]
pub(crate) struct NotificationProps {
    pub id: NotificationId,
    pub timestamp: DateTime<Utc>,
    /// How this notification's actions are dispatched back to the app. For freedesktop this also
    /// carries the owning connection's unique name (the directed-signal target).
    pub dispatch: NotificationSource,
    pub origin: Origin,
    pub content: Content,
    pub image: Option<Image>,
    pub actions: Actions,
    pub alert: Alert,
    pub classification: Classification,
    pub lifecycle: Lifecycle,
    pub presentation: Presentation,
}

/// Where a notification came from, and how its actions are dispatched.
///
/// This is the single abstraction that lets `invoke`, the owner-watching strip logic, and the
/// close-signal emission treat all backends uniformly: each dispatch carries the name its
/// actions target (via [`Backend::dispatch_target`](crate::core::backend::Backend::dispatch_target))
/// and implements the per-protocol reachability, close, and dispatch behavior, so the core never
/// matches on the protocol outside [`backend`](Self::backend). Crate-internal: the shell consumes
/// the display facets on [`Notification`](crate::core::notification::Notification), never this.
#[derive(Debug, Clone)]
pub(crate) enum NotificationSource {
    /// `org.freedesktop.Notifications`. Actions are dispatched via a directed
    /// `ActionInvoked` signal to the owning connection (`Notification::owner`), keyed by the
    /// D-Bus [`wire_id`](FreedesktopDispatch::wire_id).
    Freedesktop(FreedesktopDispatch),
    /// `org.gtk.Notifications`. Actions are dispatched via
    /// `org.freedesktop.Application.ActivateAction`/`Activate`, which cold-launches the
    /// app via D-Bus activation when it is not running.
    Gtk(GtkDispatch),
    /// `org.freedesktop.impl.portal.Notification` (the XDG Desktop Portal backend, used by
    /// sandboxed apps). Actions are dispatched by emitting the backend `ActionInvoked`
    /// signal; xdg-desktop-portal relays it to the app (handling `app.`-prefix →
    /// `ActivateAction` cold-launch itself), so the backend just reports the raw action.
    Portal(PortalDispatch),
}

impl NotificationSource {
    /// The backend that dispatches this notification's actions — the single runtime match on
    /// the delivering protocol. Everything downstream forwards through the returned
    /// `&dyn Backend`, so no other core code enumerates backends.
    pub(crate) fn backend(&self) -> &dyn Backend {
        match self {
            NotificationSource::Freedesktop(dispatch) => dispatch,
            NotificationSource::Gtk(dispatch) => dispatch,
            NotificationSource::Portal(dispatch) => dispatch,
        }
    }
}

/// Everything needed to dispatch a freedesktop notification's actions to the owning app.
#[derive(Debug, Clone)]
pub(crate) struct FreedesktopDispatch {
    /// The `org.freedesktop.Notifications` D-Bus id — the `u32` returned from `Notify` and
    /// used in the `ActionInvoked`/`NotificationClosed`/`NotificationReplied` signals. Unique
    /// only within a session bus lifetime; unrelated to the [`NotificationId`] identity.
    pub wire_id: u32,
    /// The session bus GUID this notification was created under. On restart it tells whether
    /// the `wire_id` and owner still belong to the live session (so the app can replace/close
    /// it) or a prior one (history only). Empty when the bus GUID couldn't be read.
    pub session_id: String,
    /// Unique D-Bus name of the creating connection — the target every directed signal
    /// (`ActionInvoked`/`ActivationToken`/`NotificationClosed`/`NotificationReplied`) is sent to.
    /// `None` once the owner disconnects, or for a notification restored from a prior D-Bus
    /// session (its actions then survive as history but are undispatchable). Living inside the
    /// dispatch makes "owner is meaningful only for freedesktop" a structural fact.
    pub owner: Option<String>,
}

/// Everything needed to dispatch a GTK notification's actions to the owning app.
#[derive(Debug, Clone)]
pub(crate) struct GtkDispatch {
    /// The GApplication id (well-known bus name), e.g. `org.gnome.Calendar`.
    pub app_id: String,
    /// The app-chosen notification id (the replace/withdraw key).
    pub gtk_id: String,
    /// The `default-action` (body click). `None` ⇒ body click calls `Activate` (raise).
    pub default_action: Option<GtkAction>,
    /// Button actions, keyed by the action name exposed as the `Action.id` (the name as received,
    /// including any `"app."` prefix). Every button is kept (GNOME parity); dispatch decides at
    /// click time between `ActivateAction` (`"app."`-prefixed) and an `ActionInvoked` signal.
    pub button_actions: HashMap<String, GtkAction>,
}

/// A single GTK action target: the action name **as received** (including any `"app."` prefix)
/// plus its optional parameter. Dispatch strips the prefix for `ActivateAction`, or keeps the
/// full name when routing a non-`"app."` action back via the `org.gtk.Notifications.ActionInvoked`
/// signal (both matching GNOME Shell).
#[derive(Debug)]
pub(crate) struct GtkAction {
    /// Action name as received (the `"app."` prefix, if any, is stripped only at dispatch time).
    pub name: String,
    /// Optional GVariant parameter for the action.
    pub target: Option<OwnedValue>,
}

impl Clone for GtkAction {
    // `OwnedValue` has no `Clone` (only fallible `try_clone`, because a variant could
    // carry an fd). Notification action targets are always simple serializable variants,
    // so this never actually fails; degrade to no-target rather than panic if it ever did.
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            target: self.target.as_ref().and_then(|value| value.try_clone().ok()),
        }
    }
}

/// Everything needed to dispatch a portal notification's actions back to xdg-desktop-portal.
#[derive(Debug, Clone)]
pub(crate) struct PortalDispatch {
    /// The sandboxed app's id, as given to `AddNotification` (arg 1).
    pub app_id: String,
    /// The app-chosen notification id (the replace/withdraw key, and the `id` echoed back
    /// in the `ActionInvoked` signal).
    pub portal_id: String,
    /// The `default-action` (body click), if the notification declared one.
    pub default_action: Option<PortalAction>,
    /// Button actions, keyed by the raw action name exposed as the `Action.id`.
    pub button_actions: HashMap<String, PortalAction>,
    /// The `im.reply-with-text` button's action, if one was declared — the target of an
    /// inline reply (the typed text is appended to its `ActionInvoked` parameter array).
    pub reply_action: Option<PortalAction>,
}

/// A single portal action: the action name **as received** (unlike GTK, the portal backend
/// does not strip `"app."` — it echoes the name verbatim in `ActionInvoked` and lets the
/// frontend interpret the prefix) plus its optional parameter and button purpose.
#[derive(Debug)]
pub(crate) struct PortalAction {
    /// Action name exactly as the app sent it (e.g. `app.reply` or `custom`).
    pub name: String,
    /// Optional GVariant parameter for the action.
    pub target: Option<OwnedValue>,
    /// The button's purpose (portal v2), if declared. Surfaced onto the displayed
    /// [`Action`] in `Notification::from_props`.
    pub purpose: Option<ButtonPurpose>,
}

impl Clone for PortalAction {
    // Same rationale as `GtkAction::clone`: `OwnedValue` is only fallibly cloneable.
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            target: self.target.as_ref().and_then(|value| value.try_clone().ok()),
            purpose: self.purpose.clone(),
        }
    }
}

/// The object path every `org.freedesktop.impl.portal.*` interface is exported at.
pub(crate) const PORTAL_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
/// The portal notification backend interface name.
pub(crate) const PORTAL_NOTIFICATION_INTERFACE: &str =
    "org.freedesktop.impl.portal.Notification";

/// Derives an app's `org.freedesktop.Application` object path from its id, matching
/// `g_application_get_dbus_object_path`: prefix `/`, then `.`→`/` and `-`→`_`.
pub(crate) fn gtk_object_path(app_id: &str) -> String {
    let mut path = String::with_capacity(app_id.len() + 1);
    path.push('/');
    for ch in app_id.chars() {
        match ch {
            '.' => path.push('/'),
            '-' => path.push('_'),
            other => path.push(other),
        }
    }
    path
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BorrowedImageData<'a> {
    /// Image width in pixels.
    pub width: i32,
    /// Image height in pixels.
    pub height: i32,
    /// Distance in bytes between row starts (may include padding).
    pub rowstride: i32,
    /// Bits per sample (always 8 per spec).
    pub bits_per_sample: i32,
    /// Number of channels (3 for RGB, 4 for RGBA).
    pub channels: i32,
    /// Borrowed raw pixel data in RGB or RGBA byte order.
    pub data: &'a [u8],
}

/// Hints for notifications as specified by the Desktop Notifications Specification.
pub type NotificationHints = HashMap<String, OwnedValue>;

type RawImageData<'a> = (i32, i32, i32, bool, i32, i32, &'a [u8]);

#[derive(Debug, Default, Deserialize, Type)]
#[serde(default)]
#[zvariant(signature = "a{sv}")]
pub(crate) struct IncomingHints<'a> {
    #[serde(borrow, with = "optional", rename = "image-data")]
    image_data: Option<RawImageData<'a>>,
    #[serde(borrow, with = "optional", rename = "image_data")]
    image_data_legacy: Option<RawImageData<'a>>,
    #[serde(borrow, with = "optional", rename = "icon_data")]
    icon_data: Option<RawImageData<'a>>,
    #[serde(flatten)]
    hints: NotificationHints,
}

impl<'a> IncomingHints<'a> {
    pub(crate) fn image_data(&self) -> Option<BorrowedImageData<'a>> {
        let (width, height, rowstride, _has_alpha, bits_per_sample, channels, data) = self
            .image_data
            .or(self.image_data_legacy)
            .or(self.icon_data)?;

        Some(BorrowedImageData {
            width,
            height,
            rowstride,
            bits_per_sample,
            channels,
            data,
        })
    }

    pub(crate) fn into_owned(self) -> NotificationHints {
        self.hints
    }
}

/// Represents a notification action with an ID and label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// Action identifier (the dispatch key). For a button this is never the reserved default
    /// key; the body-click action carries [`ActionId::default_action`]. Internal: the shell
    /// invokes a button by passing the whole [`Action`] to [`Notification::invoke`](crate::core::notification::Notification::invoke), never the
    /// id, so this is not a display concern.
    pub(crate) id: ActionId,
    /// Human-readable label for display.
    pub label: String,
    /// The button's purpose (XDG portal v2), when the app declared one. `None` for
    /// freedesktop/GNotification buttons and the default action; set from the portal
    /// dispatch data in `Notification::from_props`.
    pub purpose: Option<ButtonPurpose>,
}

impl Action {
    /// Parses the freedesktop flat `[id, label, ...]` actions list. The reserved `"default"`
    /// id becomes [`ActionId::default_action`]; every other becomes a normal [`ActionId`].
    pub(crate) fn parse_dbus_actions(raw_actions: &[String]) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut iter = raw_actions.iter();

        while let Some(id) = iter.next() {
            let label = iter.next().unwrap_or(id);
            let action_id = if id == ActionId::DEFAULT_KEY {
                ActionId::default_action()
            } else {
                // Only `"default"` is reserved, and it is handled above, so this is Ok.
                match ActionId::new(id.clone()) {
                    Ok(action_id) => action_id,
                    Err(_) => continue,
                }
            };
            actions.push(Action {
                id: action_id,
                label: label.clone(),
                purpose: None,
            });
        }

        actions
    }

    #[cfg(test)]
    pub(crate) fn to_dbus_format(actions: &[Action]) -> Vec<String> {
        let mut raw = Vec::with_capacity(actions.len() * 2);

        for action in actions {
            raw.push(action.id.as_str().to_owned());
            raw.push(action.label.clone());
        }

        raw
    }
}

// ===========================================================================================
// Unified, display-driven schema (the public shape a shell consumes).
//
// Every type below is a presentation/interaction concern; the delivering protocol is not
// represented. The backend adapters translate their wire formats (fdo hints / gtk & portal
// `a{sv}`) into these facets in this crate. `serde` derives let the facets persist as a JSON
// blob without per-field columns.
// ===========================================================================================

/// The renderable body text fused with the one bit that changes rendering: is it markup?
/// The shell escapes `Plain` and feeds `Markup` to its markup renderer (stripping tags it
/// doesn't support) — it never cross-references a separate flag or server capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Body {
    /// Plain text; escape before rendering.
    Plain(String),
    /// A markup subset (`<b> <i> <a href>` …); render rich, strip the unsupported.
    Markup(String),
}

impl Body {
    /// The underlying text regardless of markup mode.
    pub fn text(&self) -> &str {
        match self {
            Body::Plain(text) | Body::Markup(text) => text,
        }
    }

    /// Whether the text should be rendered as markup.
    pub fn is_markup(&self) -> bool {
        matches!(self, Body::Markup(_))
    }
}

/// A resolved image source — the union of every way the backends deliver an image, already
/// normalized so the shell has one `match` and never touches raw pixels, GIcons, bytes or fds
/// (all materialized to a cached file at ingest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Image {
    /// An icon-theme name to look up (the shell chooses the size).
    Named(String),
    /// A file on disk (a cache path or a plain path).
    Path(PathBuf),
}

/// A freedesktop sound-naming-spec event id (a namespace distinct from file paths).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundName(String);

impl SoundName {
    /// Wraps a sound-naming-spec event id.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
    /// The event id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Audio-feedback intent as one typed choice, which the shell's sound service resolves under
/// DND/mute.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Alert {
    /// No sound requested; the shell may still derive one from category/priority.
    #[default]
    Unspecified,
    /// Explicitly silent — play nothing (overrides any default).
    Silent,
    /// Play the system/shell default alert (portal-exclusive intent).
    Default,
    /// A themed sound-naming-spec event id.
    Named(SoundName),
    /// A specific sound file.
    File(PathBuf),
}

/// Requested auto-dismiss policy, from the wire's overloaded signed int into explicit intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Timeout {
    /// Use the shell's per-priority default (fdo `-1`).
    #[default]
    ServerDefault,
    /// The app explicitly requested never-auto-expire (fdo `0`).
    NeverByApp,
    /// The backend has no timeout concept; persists until withdrawn (gtk/portal). A shell MAY
    /// still apply its own auto-dismiss policy here, unlike [`NeverByApp`](Self::NeverByApp).
    PersistentByBackend,
    /// Auto-close after this duration (fdo `>0`).
    After(Duration),
}

/// Banner-vs-tray routing intent (kept independent of `transient`, which portal allows to
/// coexist with `tray`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Routing {
    /// Show as a banner.
    #[default]
    Banner,
    /// No banner; straight to a tray/center (portal `tray`).
    TrayOnly,
}

/// Lock-screen redaction (three mutually-exclusive states).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LockscreenVisibility {
    /// Show fully.
    #[default]
    Show,
    /// Show that a notification exists but redact its content.
    HideContent,
    /// Omit it entirely from the lock screen.
    Hide,
}

/// The sending application, resolved for the header and for grouping/filtering.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    /// Human display name (spoofable → a label, never a key).
    pub name: Option<String>,
    /// A secondary attribution line, distinct from `name`, for when one app proxies many
    /// sources: the device/account/contact a notification actually came from (e.g. "My Phone",
    /// "foo@example.com", a chat room). From the KDE `x-kde-origin-name` hint; `None` otherwise.
    pub origin_name: Option<String>,
    /// Trusted, locale-independent `.desktop` id — the icon-lookup / grouping / per-app-DND key.
    pub desktop_entry: Option<DesktopEntryId>,
    /// Small header/branding icon. Filled from a themed app icon; a file/bytes icon goes to
    /// [`NotificationView::image`](crate::core::notification::NotificationView::image) instead.
    pub icon: Option<Image>,
}

/// The renderable text: an always-shown summary and an optional body with its markup mode.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Content {
    /// Single-line headline, always rendered.
    pub summary: String,
    /// Optional multi-line message; `None` = summary-only card.
    pub body: Option<Body>,
    /// A completion percentage (0–100) to render as a progress bar, for OSD-style
    /// notifications (volume, brightness, transfers). From the de-facto freedesktop `value`
    /// hint; `None` when absent. GNotification and the portal have no equivalent.
    pub progress: Option<u8>,
}

/// The interaction surface of the card.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Actions {
    /// The body-click action (no button drawn); `None` = inert body. When the notification
    /// carries attached `urls` this is `Some` even without an app action, so the body is
    /// clickable to open the primary link.
    pub default: Option<Action>,
    /// Ordered buttons, never including the default/body action or the inline-reply action.
    pub buttons: Vec<Action>,
    /// Links attached to the notification (freedesktop `x-kde-urls`). Internal to dispatch, not a
    /// display concern: when present, [`default`](Self::default) is synthesized so the body is
    /// clickable, and [`Notification::invoke`](crate::core::notification::Notification::invoke) on the default opens the first link. The shell
    /// only sees that the body is clickable (via `default`), never the URIs themselves.
    /// Empty for GNotification/portal, which have no equivalent.
    pub(crate) urls: Vec<Uri>,
    /// An inline text-reply affordance (IM apps), when the notification requested one. Lifted
    /// from the fdo `inline-reply` action or a portal `im.reply-with-text` button; the shell
    /// renders a text field and calls [`Notification::reply`](crate::core::notification::Notification::reply).
    pub reply: Option<Reply>,
    /// Render button ids as themed icon names instead of text labels (fdo `action-icons`).
    pub action_icons: bool,
}

/// An inline text-reply affordance. The presence of this (on [`Actions::reply`]) tells the
/// shell to draw a reply text field; submitting calls
/// [`Notification::reply`](crate::core::notification::Notification::reply), which routes the
/// typed text back to the app (freedesktop `NotificationReplied` signal, or a portal
/// `im.reply-with-text` `ActionInvoked`). GNotification cannot express inline reply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// Placeholder text for the empty field (fdo `x-kde-reply-placeholder-text`); `None` ⇒
    /// the shell's default. The portal supplies no placeholder.
    pub placeholder: Option<String>,
    /// Submit-button label (fdo `x-kde-reply-submit-button-text`, or the portal
    /// `im.reply-with-text` button's own label); `None` ⇒ the shell's default.
    pub submit_label: Option<String>,
    /// Submit-button themed icon name (fdo `x-kde-reply-submit-button-icon-name`); `None` ⇒
    /// the shell's default.
    pub submit_icon: Option<String>,
}

/// Importance, type, and the persistence/coalescing behavior axes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    /// 4-level importance — the biggest behavior switch (accent, sticky, DND bypass, sort).
    pub priority: Priority,
    /// Semantic type; fallback icon/sound selector, special layouts, grouping key.
    pub category: Option<Category>,
    /// Banner-only: do not archive to history after close.
    pub transient: bool,
    /// fdo-only: keep the card alive after an action fires (media controls). Distinct from
    /// [`Lifecycle::timeout`] being [`Timeout::PersistentByBackend`].
    pub resident: bool,
    /// Coalescing key beyond `replaces` (volume/brightness/MPRIS single-slot). `None` = none.
    /// Internal: the freedesktop ingest resolves this to a shared [`NotificationId`] at
    /// notify-time (a same-tag notification reuses the previous one's identity, so a shell
    /// sees an in-place update), so it is a dispatch concern the shell never reads.
    pub(crate) stack_tag: Option<String>,
}

/// How long the notification lives and how it may leave.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Lifecycle {
    /// The app's requested auto-dismiss policy — one input to the shell's precedence chain.
    pub timeout: Timeout,
    /// The app forbids manual dismissal (portal `persistent`); the shell hides its close X.
    pub locked_open: bool,
}

/// Where the card surfaces and how it is redacted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Presentation {
    /// Banner-vs-tray routing.
    pub routing: Routing,
    /// Lock-screen privacy (the shell gates on session-lock state).
    pub lockscreen: LockscreenVisibility,
    /// On replace, re-alert instead of updating silently (portal `show-as-new`).
    pub realert_on_replace: bool,
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::{LE, Value, serialized::Context, to_bytes};

    use super::*;

    #[test]
    fn parse_dbus_actions_with_empty_input_returns_empty_vec() {
        let raw_actions: Vec<String> = vec![];

        let result = Action::parse_dbus_actions(&raw_actions);

        assert_eq!(result, vec![]);
    }

    #[test]
    fn parse_dbus_actions_with_even_count_creates_actions() {
        let raw_actions = vec![
            "reply".to_string(),
            "Reply".to_string(),
            "delete".to_string(),
            "Delete".to_string(),
        ];

        let result = Action::parse_dbus_actions(&raw_actions);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id.as_str(), "reply");
        assert_eq!(result[0].label, "Reply");
        assert_eq!(result[1].id.as_str(), "delete");
        assert_eq!(result[1].label, "Delete");
    }

    #[test]
    fn parse_dbus_actions_with_odd_count_uses_id_as_label_for_last() {
        let raw_actions = vec![
            "reply".to_string(),
            "Reply".to_string(),
            "default".to_string(),
        ];

        let result = Action::parse_dbus_actions(&raw_actions);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id.as_str(), "reply");
        assert_eq!(result[0].label, "Reply");
        assert_eq!(result[1].id.as_str(), "default");
        assert_eq!(result[1].label, "default");
    }

    #[test]
    fn to_dbus_format_with_empty_input_returns_empty_vec() {
        let actions: Vec<Action> = vec![];

        let result = Action::to_dbus_format(&actions);

        assert_eq!(result, Vec::<String>::new());
    }

    #[test]
    fn to_dbus_format_creates_alternating_id_label_pairs() {
        let actions = vec![
            Action {
                id: ActionId::new("reply").unwrap(),
                label: "Reply".to_string(),
                purpose: None,
            },
            Action {
                id: ActionId::new("delete").unwrap(),
                label: "Delete".to_string(),
                purpose: None,
            },
        ];

        let result = Action::to_dbus_format(&actions);

        assert_eq!(result.len(), 4);
        assert_eq!(result[0], "reply");
        assert_eq!(result[1], "Reply");
        assert_eq!(result[2], "delete");
        assert_eq!(result[3], "Delete");
    }

    #[test]
    fn parse_and_to_dbus_format_are_inverse_operations() {
        let original = vec![
            "reply".to_string(),
            "Reply".to_string(),
            "mark-read".to_string(),
            "Mark as Read".to_string(),
        ];

        let parsed = Action::parse_dbus_actions(&original);
        let result = Action::to_dbus_format(&parsed);

        assert_eq!(result, original);
    }

    #[test]
    fn incoming_hints_extracts_image_data_without_storing_raw_hint() {
        let pixels = [0u8, 1, 2, 3];
        let mut raw = HashMap::new();
        raw.insert("category", Value::new("im.received"));
        raw.insert(
            "image-data",
            Value::new((1i32, 1i32, 4i32, true, 8i32, 4i32, &pixels[..])),
        );
        let encoded = to_bytes(Context::new_dbus(LE, 0), &raw).expect("hints should encode");

        let (hints, _): (IncomingHints<'_>, _) = encoded.deserialize().expect("hints should parse");

        let image = hints.image_data().expect("image-data should parse");

        assert_eq!(image.width, 1);
        assert_eq!(image.height, 1);
        assert_eq!(image.data, pixels);
        assert!(hints.into_owned().contains_key("category"));
    }

    #[test]
    fn incoming_hints_prefers_spec_image_data_key() {
        let low_priority = [9u8, 9, 9, 9];
        let high_priority = [1u8, 2, 3, 4];
        let mut raw = HashMap::new();
        raw.insert(
            "icon_data",
            Value::new((1i32, 1i32, 4i32, true, 8i32, 4i32, &low_priority[..])),
        );
        raw.insert(
            "image-data",
            Value::new((1i32, 1i32, 4i32, true, 8i32, 4i32, &high_priority[..])),
        );
        let encoded = to_bytes(Context::new_dbus(LE, 0), &raw).expect("hints should encode");

        let (hints, _): (IncomingHints<'_>, _) = encoded.deserialize().expect("hints should parse");

        assert_eq!(
            hints.image_data().expect("image-data should parse").data,
            high_priority
        );
    }
}
