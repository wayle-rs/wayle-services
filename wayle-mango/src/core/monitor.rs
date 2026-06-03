//! Plain-value snapshot of a Mango monitor.

use crate::types::{FocusedClient, MonitorSnapshot, Tag, TagId};

/// A Mango monitor and its tags at one point in time.
///
/// Mango pushes a full snapshot on every change, so each
/// [`MangoService::monitors`](crate::MangoService::monitors) update carries a
/// freshly built [`Monitor`]. Compare two values with `PartialEq` to tell
/// whether anything a consumer cares about changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    /// Connector name, for example `eDP-1`.
    pub name: String,

    /// Whether this is the focused monitor.
    ///
    /// Exactly one monitor has this flag while any monitor is connected.
    pub is_active: bool,

    /// Every tag on the monitor, in Mango's order.
    pub tags: Vec<Tag>,

    /// Indices of the tags currently shown, or `[0]` while the overview is open.
    pub active_tags: Vec<TagId>,

    /// The focused client on this monitor, if any.
    pub focused_client: Option<FocusedClient>,
}

impl Monitor {
    pub(crate) fn from_snapshot(snapshot: MonitorSnapshot) -> Self {
        Self {
            name: snapshot.name,
            is_active: snapshot.active,
            tags: snapshot.tags,
            active_tags: snapshot.active_tags,
            focused_client: snapshot.active_client.into_focused(),
        }
    }
}
