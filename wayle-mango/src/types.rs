//! Plain data types: the public [`Tag`] and [`FocusedClient`] values consumers
//! read, plus the wire structs the `watch all-monitors` stream deserializes
//! into.

use std::fmt::{self, Display, Formatter};

use serde::Deserialize;

/// One-based index of a tag on a monitor.
///
/// Wraps `u32` and is deliberately distinct from [`ClientId`] so a tag index
/// and a client id can never be passed in place of one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(transparent)]
pub struct TagId(u32);

impl TagId {
    /// Wraps a raw one-based tag index.
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// Returns the raw one-based tag index.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl Display for TagId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl From<u32> for TagId {
    fn from(index: u32) -> Self {
        Self(index)
    }
}

/// Stable id of a client (window).
///
/// Wraps `u32` and is deliberately distinct from [`TagId`] so a client id and
/// a tag index can never be passed in place of one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(transparent)]
pub struct ClientId(u32);

impl ClientId {
    /// Wraps a raw client id.
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns the raw client id.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl Display for ClientId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl From<u32> for ClientId {
    fn from(id: u32) -> Self {
        Self(id)
    }
}

/// A Mango tag, the dwm-style equivalent of a workspace.
///
/// Mango has a fixed set of tags per monitor. A tag is "occupied" when
/// `client_count > 0`, and more than one tag can be visible at once, so a
/// monitor reports `is_active` per tag rather than a single active workspace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Tag {
    /// One-based position of the tag on its monitor.
    pub index: TagId,

    /// Whether the tag is currently shown on its monitor.
    pub is_active: bool,

    /// Whether any client on the tag requested attention.
    pub is_urgent: bool,

    /// Layout symbol active for the tag (for example `T` for tile or `DW` for
    /// dwindle).
    pub layout: String,

    /// Number of clients placed on the tag.
    pub client_count: u32,
}

/// The focused client (window) on a monitor.
///
/// `title` and `app_id` are `None` when the client leaves the field unset,
/// distinct from there being no focused client at all (which the service
/// signals with `Option<FocusedClient>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusedClient {
    /// Mango's stable client id, usable with [`focus_window`].
    ///
    /// [`focus_window`]: crate::MangoService::focus_window
    pub id: ClientId,

    /// Window title, if the application set one.
    pub title: Option<String>,

    /// Wayland application id, if the application set one.
    pub app_id: Option<String>,
}

/// Top-level payload of one `watch all-monitors` frame.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MonitorList {
    pub monitors: Vec<MonitorSnapshot>,
}

/// One monitor as serialized by Mango's `build_monitor_json`.
///
/// Only the fields the three modules need are decoded; Mango sends more
/// (geometry, scale, layout index, last open surface) that this crate ignores.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MonitorSnapshot {
    pub name: String,

    pub active: bool,

    pub tags: Vec<Tag>,

    pub active_tags: Vec<TagId>,

    pub active_client: ActiveClientSnapshot,

    pub keymode: String,

    pub keyboardlayout: String,
}

/// Mango's `active_client` object, all fields null when no client is focused.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ActiveClientSnapshot {
    pub id: Option<ClientId>,

    pub title: Option<String>,

    pub appid: Option<String>,
}

impl ActiveClientSnapshot {
    /// Converts the wire object into a [`FocusedClient`], treating a null id or
    /// empty title/app id as absence.
    pub(crate) fn into_focused(self) -> Option<FocusedClient> {
        let id = self.id?;

        Some(FocusedClient {
            id,
            title: self.title.filter(|title| !title.is_empty()),
            app_id: self.appid.filter(|app_id| !app_id.is_empty()),
        })
    }
}

/// Top-level payload of one `watch all-clients` frame.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ClientList {
    pub clients: Vec<ClientSnapshot>,
}

/// One client (window) as serialized by Mango's `build_client_json`.
///
/// Only the fields the tag switcher needs are decoded.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ClientSnapshot {
    pub id: ClientId,

    pub title: Option<String>,

    pub appid: Option<String>,

    pub monitor: String,

    pub tags: Vec<TagId>,

    pub is_urgent: bool,

    pub is_focused: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_MONITORS_FRAME: &str = r#"{"monitors":[{"name":"eDP-1","active":true,"x":0,"y":0,"width":1920,"height":1080,"scale":1,"layout_index":0,"layout_symbol":"T","last_open_surface":"foot","tags":[{"index":1,"is_active":true,"is_urgent":false,"layout":"T","client_count":2},{"index":2,"is_active":false,"is_urgent":true,"layout":"DW","client_count":0}],"active_tags":[1],"active_client":{"id":42,"title":"nvim","appid":"foot"},"keymode":"default","keyboardlayout":"English (US)"}]}"#;

    #[test]
    fn parses_all_monitors_frame() -> serde_json::Result<()> {
        let list: MonitorList = serde_json::from_str(ALL_MONITORS_FRAME)?;
        let monitor = &list.monitors[0];

        assert_eq!(monitor.name, "eDP-1");
        assert!(monitor.active);
        assert_eq!(monitor.keyboardlayout, "English (US)");
        assert_eq!(monitor.active_tags, vec![TagId::new(1)]);

        assert_eq!(monitor.tags[1].index, TagId::new(2));
        assert!(monitor.tags[1].is_urgent);
        assert_eq!(monitor.tags[0].client_count, 2);

        Ok(())
    }

    #[test]
    fn focused_client_carries_id_and_strings() {
        let snapshot = ActiveClientSnapshot {
            id: Some(ClientId::new(42)),
            title: Some("nvim".to_owned()),
            appid: Some("foot".to_owned()),
        };

        let focused = snapshot.into_focused();

        assert_eq!(
            focused.as_ref().map(|client| client.id),
            Some(ClientId::new(42))
        );
        assert_eq!(
            focused.as_ref().and_then(|client| client.title.as_deref()),
            Some("nvim")
        );
        assert_eq!(
            focused.and_then(|client| client.app_id),
            Some("foot".to_owned())
        );
    }

    #[test]
    fn null_id_means_no_focused_client() {
        let snapshot = ActiveClientSnapshot {
            id: None,
            title: None,
            appid: None,
        };

        assert!(snapshot.into_focused().is_none());
    }

    #[test]
    fn empty_title_collapses_to_none() {
        let snapshot = ActiveClientSnapshot {
            id: Some(ClientId::new(7)),
            title: Some(String::new()),
            appid: Some("foot".to_owned()),
        };

        let focused = snapshot.into_focused();

        assert_eq!(
            focused.as_ref().and_then(|client| client.title.clone()),
            None
        );
        assert_eq!(
            focused.and_then(|client| client.app_id),
            Some("foot".to_owned())
        );
    }
}
