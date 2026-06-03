//! Plain-value snapshot of a Mango client (window).

use crate::types::{ClientId, ClientSnapshot, TagId};

/// A Mango client and the tags it occupies at one point in time.
///
/// A client can belong to more than one tag (dwm tags are a bitmask), so
/// `tags` is a list of one-based indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    /// Mango's stable client id.
    pub id: ClientId,

    /// Window title, if the application set one.
    pub title: Option<String>,

    /// Wayland application id, if the application set one.
    pub app_id: Option<String>,

    /// Connector name of the monitor the client is on.
    pub monitor: String,

    /// One-based indices of the tags this client occupies.
    pub tags: Vec<TagId>,

    /// Whether the client requested attention.
    pub is_urgent: bool,

    /// Whether the client currently holds focus.
    pub is_focused: bool,
}

impl Client {
    pub(crate) fn from_snapshot(snapshot: ClientSnapshot) -> Self {
        Self {
            id: snapshot.id,
            title: snapshot.title.filter(|title| !title.is_empty()),
            app_id: snapshot.appid.filter(|app_id| !app_id.is_empty()),
            monitor: snapshot.monitor,
            tags: snapshot.tags,
            is_urgent: snapshot.is_urgent,
            is_focused: snapshot.is_focused,
        }
    }
}
