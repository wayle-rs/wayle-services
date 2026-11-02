use std::fmt::{self, Display};

use serde::{Deserialize, Deserializer};

use crate::{Address, MonitorId, deserialize_optional_address};

/// Numeric workspace ID.
///
/// Only numbered workspaces have one. Named and special workspaces are
/// identified by their [`WorkspaceAddress`] instead.
pub type WorkspaceId = i64;

const SPECIAL_NAME_PREFIX: &str = "special:";

/// The name Hyprland uses to refer to a workspace.
///
/// Numbered workspaces use their number (`"3"`), named workspaces use their
/// name (`"chat"`), and special workspaces use `"special:<name>"`. On
/// Hyprland 0.57+ the address survives a rename but changes when a numbered
/// workspace is renumbered (see [`HyprlandEvent::ChangeWorkspaceId`]).
///
/// [`HyprlandEvent::ChangeWorkspaceId`]: crate::HyprlandEvent::ChangeWorkspaceId
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceAddress(String);

impl WorkspaceAddress {
    /// Creates an address from its string form.
    pub fn new(address: impl Into<String>) -> Self {
        Self(address.into())
    }

    /// Returns the address as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the address and returns the inner string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Display for WorkspaceAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for WorkspaceAddress {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for WorkspaceAddress {
    fn from(address: String) -> Self {
        Self(address)
    }
}

impl From<&str> for WorkspaceAddress {
    fn from(address: &str) -> Self {
        Self(address.to_string())
    }
}

/// Whether a workspace is a regular workspace or a special (scratchpad) one.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkspaceKind {
    /// A regular workspace, numbered or named.
    Normal,
    /// A special workspace that overlays the regular one.
    Special,
}

impl WorkspaceKind {
    fn from_name(name: &str) -> Self {
        if name.starts_with(SPECIAL_NAME_PREFIX) {
            Self::Special
        } else {
            Self::Normal
        }
    }

    fn from_payload(kind: Option<&str>, name: &str) -> Self {
        match kind {
            Some("special") => Self::Special,
            Some("normal") => Self::Normal,
            _ => Self::from_name(name),
        }
    }
}

/// Workspace identity resolved from either IPC format.
///
/// Hyprland 0.57 sends `address`, `type`, and an `id` only for numbered
/// workspaces. Earlier versions send only `id`, where named and special
/// workspaces have negative IDs that carry no meaning; those map to an
/// address equal to the workspace name and no ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceIdentity {
    pub id: Option<WorkspaceId>,
    pub address: WorkspaceAddress,
    pub kind: WorkspaceKind,
}

impl WorkspaceIdentity {
    pub(crate) fn from_payload(
        id: Option<WorkspaceId>,
        address: Option<&str>,
        kind: Option<&str>,
        name: &str,
    ) -> Option<Self> {
        let Some(address) = address else {
            return Self::from_legacy_id(id?, name);
        };

        if address.is_empty() {
            return None;
        }

        Some(Self {
            id,
            address: WorkspaceAddress::from(address),
            kind: WorkspaceKind::from_payload(kind, name),
        })
    }

    /// Resolves the leading workspace field of a socket2 event, which is an
    /// address on Hyprland 0.57+ and a numeric ID before that.
    pub(crate) fn from_event_field(field: &str, name: &str) -> Option<Self> {
        if field.is_empty() {
            return None;
        }

        match field.parse::<WorkspaceId>() {
            Ok(id) => Self::from_legacy_id(id, name),
            Err(_) => Some(Self {
                id: None,
                address: WorkspaceAddress::from(field),
                kind: WorkspaceKind::from_name(field),
            }),
        }
    }

    fn from_legacy_id(id: WorkspaceId, name: &str) -> Option<Self> {
        if id > 0 {
            return Some(Self {
                id: Some(id),
                address: WorkspaceAddress::new(id.to_string()),
                kind: WorkspaceKind::Normal,
            });
        }

        if name.is_empty() {
            return None;
        }

        Some(Self {
            id: None,
            address: WorkspaceAddress::from(name),
            kind: WorkspaceKind::from_name(name),
        })
    }
}

/// A workspace reference embedded in monitor and window data.
#[non_exhaustive]
#[derive(Debug, Deserialize, Clone, PartialEq, Eq, PartialOrd)]
#[serde(try_from = "RawWorkspaceInfo")]
pub struct WorkspaceInfo {
    /// Workspace number, present only for numbered workspaces.
    pub id: Option<WorkspaceId>,
    /// Workspace address.
    pub address: WorkspaceAddress,
    /// Regular or special workspace.
    pub kind: WorkspaceKind,
    /// Display name.
    pub name: String,
}

#[derive(Deserialize)]
struct RawWorkspaceInfo {
    #[serde(default)]
    id: Option<WorkspaceId>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    name: String,
}

impl RawWorkspaceInfo {
    fn into_workspace_info(self) -> Option<WorkspaceInfo> {
        let identity = WorkspaceIdentity::from_payload(
            self.id,
            self.address.as_deref(),
            self.kind.as_deref(),
            &self.name,
        )?;

        Some(WorkspaceInfo {
            id: identity.id,
            address: identity.address,
            kind: identity.kind,
            name: self.name,
        })
    }
}

impl TryFrom<RawWorkspaceInfo> for WorkspaceInfo {
    type Error = String;

    fn try_from(raw: RawWorkspaceInfo) -> Result<Self, Self::Error> {
        let name = raw.name.clone();

        raw.into_workspace_info()
            .ok_or_else(|| format!("workspace `{name}` has no address or id"))
    }
}

pub(crate) fn deserialize_optional_workspace_info<'de, D>(
    deserializer: D,
) -> Result<Option<WorkspaceInfo>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: RawWorkspaceInfo = Deserialize::deserialize(deserializer)?;
    Ok(raw.into_workspace_info())
}

/// A workspace rule from Hyprland configuration.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRule {
    /// The workspace identifier string (could be name or ID).
    pub workspace_string: String,
    /// The monitor this workspace is bound to, if specified.
    #[serde(default)]
    pub monitor: Option<String>,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceData {
    #[serde(default)]
    pub id: Option<WorkspaceId>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    pub name: String,
    pub monitor: String,
    #[serde(rename = "monitorID")]
    pub monitor_id: Option<MonitorId>,
    pub windows: u16,
    #[serde(rename = "hasfullscreen")]
    pub fullscreen: bool,
    #[serde(
        rename = "lastwindow",
        deserialize_with = "deserialize_optional_address"
    )]
    pub last_window: Option<Address>,
    #[serde(rename = "lastwindowtitle")]
    pub last_window_title: String,
    #[serde(rename = "ispersistent")]
    pub persistent: bool,
    pub tiled_layout: String,
}

impl WorkspaceData {
    pub(crate) fn identity(&self) -> Option<WorkspaceIdentity> {
        WorkspaceIdentity::from_payload(
            self.id,
            self.address.as_deref(),
            self.kind.as_deref(),
            &self.name,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(
        id: Option<WorkspaceId>,
        address: &str,
        kind: WorkspaceKind,
    ) -> Option<WorkspaceIdentity> {
        Some(WorkspaceIdentity {
            id,
            address: WorkspaceAddress::from(address),
            kind,
        })
    }

    #[test]
    fn payload_with_address_keeps_numbered_id() {
        let resolved = WorkspaceIdentity::from_payload(Some(3), Some("3"), Some("normal"), "3");

        assert_eq!(resolved, identity(Some(3), "3", WorkspaceKind::Normal));
    }

    #[test]
    fn payload_with_address_and_no_id_is_named() {
        let resolved = WorkspaceIdentity::from_payload(None, Some("chat"), Some("normal"), "chat");

        assert_eq!(resolved, identity(None, "chat", WorkspaceKind::Normal));
    }

    #[test]
    fn payload_type_marks_special_workspace() {
        let resolved = WorkspaceIdentity::from_payload(
            None,
            Some("special:scratch"),
            Some("special"),
            "special:scratch",
        );

        assert_eq!(
            resolved,
            identity(None, "special:scratch", WorkspaceKind::Special)
        );
    }

    #[test]
    fn payload_with_empty_address_is_absent() {
        let resolved = WorkspaceIdentity::from_payload(None, Some(""), Some(""), "");

        assert_eq!(resolved, None);
    }

    #[test]
    fn legacy_positive_id_becomes_numbered_address() {
        let resolved = WorkspaceIdentity::from_payload(Some(5), None, None, "5");

        assert_eq!(resolved, identity(Some(5), "5", WorkspaceKind::Normal));
    }

    #[test]
    fn legacy_negative_id_resolves_named_workspace_by_name() {
        let resolved = WorkspaceIdentity::from_payload(Some(-1338), None, None, "chat");

        assert_eq!(resolved, identity(None, "chat", WorkspaceKind::Normal));
    }

    #[test]
    fn legacy_negative_id_resolves_special_workspace_by_name() {
        let resolved = WorkspaceIdentity::from_payload(Some(-98), None, None, "special:scratch");

        assert_eq!(
            resolved,
            identity(None, "special:scratch", WorkspaceKind::Special)
        );
    }

    #[test]
    fn legacy_empty_special_slot_is_absent() {
        let resolved = WorkspaceIdentity::from_payload(Some(0), None, None, "");

        assert_eq!(resolved, None);
    }

    #[test]
    fn payload_without_address_or_id_is_absent() {
        let resolved = WorkspaceIdentity::from_payload(None, None, None, "orphan");

        assert_eq!(resolved, None);
    }

    #[test]
    fn event_field_numeric_resolves_numbered_workspace() {
        let resolved = WorkspaceIdentity::from_event_field("7", "7");

        assert_eq!(resolved, identity(Some(7), "7", WorkspaceKind::Normal));
    }

    #[test]
    fn event_field_address_resolves_named_workspace() {
        let resolved = WorkspaceIdentity::from_event_field("chat", "chat");

        assert_eq!(resolved, identity(None, "chat", WorkspaceKind::Normal));
    }

    #[test]
    fn event_field_special_address_resolves_special_workspace() {
        let resolved = WorkspaceIdentity::from_event_field("special:scratch", "special:scratch");

        assert_eq!(
            resolved,
            identity(None, "special:scratch", WorkspaceKind::Special)
        );
    }

    #[test]
    fn event_field_legacy_negative_id_resolves_by_name() {
        let resolved = WorkspaceIdentity::from_event_field("-1338", "chat");

        assert_eq!(resolved, identity(None, "chat", WorkspaceKind::Normal));
    }

    #[test]
    fn event_field_legacy_negative_id_without_name_is_absent() {
        let resolved = WorkspaceIdentity::from_event_field("-1338", "");

        assert_eq!(resolved, None);
    }

    #[test]
    fn event_field_empty_is_absent() {
        let resolved = WorkspaceIdentity::from_event_field("", "");

        assert_eq!(resolved, None);
    }

    #[test]
    fn workspace_info_fails_without_identity() {
        let parsed = serde_json::from_str::<WorkspaceInfo>(r#"{"address":"","name":""}"#);

        assert!(parsed.is_err());
    }
}
