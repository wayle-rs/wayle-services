use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// The urgency level of a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Urgency {
    /// Low urgency. Server implementations may display the notification how they choose.
    Low = 0,
    /// Normal urgency. Server implementations may display the notification how they choose.
    Normal = 1,
    /// Critical urgency. Critical notifications do not automatically expire, as they are
    /// important for the user to see. They are closed only when the user dismisses them,
    /// for example, by clicking on the notification.
    Critical = 2,
}

impl From<u8> for Urgency {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Low,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }
}

/// The priority of a notification on the 4-level scale shared by `GNotification` and the
/// XDG Desktop Portal.
///
/// The freedesktop.org spec has only three urgencies and cannot express [`High`]; both
/// `org.gtk.Notifications` and `org.freedesktop.impl.portal.Notification` use these four
/// levels. This is the unified level intended for display (four distinct styles).
/// [`Urgency`] is retained as the freedesktop-compatible 3-level projection for backward
/// compatibility.
///
/// [`High`]: Priority::High
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum Priority {
    /// Low priority.
    Low = 0,
    /// Normal priority. The default when unspecified.
    #[default]
    Normal = 1,
    /// High priority: above normal, below urgent. Freedesktop notifications cannot express
    /// this level; it projects to [`Urgency::Normal`].
    High = 2,
    /// Urgent priority. Urgent notifications do not automatically expire.
    Urgent = 3,
}

impl From<u8> for Priority {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Low,
            2 => Self::High,
            3 => Self::Urgent,
            _ => Self::Normal,
        }
    }
}

impl From<Urgency> for Priority {
    /// Lifts a freedesktop urgency onto the 4-level scale. Freedesktop has no `High`, so
    /// `Critical` maps to [`Priority::Urgent`].
    fn from(urgency: Urgency) -> Self {
        match urgency {
            Urgency::Low => Self::Low,
            Urgency::Normal => Self::Normal,
            Urgency::Critical => Self::Urgent,
        }
    }
}

impl From<Priority> for Urgency {
    /// Projects the 4-level [`Priority`] back onto the freedesktop 3-level urgency scale: `High`
    /// has no freedesktop equivalent and collapses to `Normal`.
    fn from(priority: Priority) -> Self {
        match priority {
            Priority::Low => Self::Low,
            Priority::Normal | Priority::High => Self::Normal,
            Priority::Urgent => Self::Critical,
        }
    }
}

impl FromStr for Priority {
    type Err = ();

    /// Parses a `GNotification` / portal priority string (`low`, `normal`, `high`,
    /// `urgent`). Unknown values fall back to `Normal`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "low" => Self::Low,
            "high" => Self::High,
            "urgent" => Self::Urgent,
            _ => Self::Normal,
        })
    }
}

/// The reason a notification was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum ClosedReason {
    /// The notification expired.
    Expired = 1,
    /// The notification was dismissed by the user.
    DismissedByUser = 2,
    /// The notification was closed by a call to CloseNotification.
    Closed = 3,
    /// Undefined/reserved reasons.
    Unknown = 4,
}

impl From<u32> for ClosedReason {
    fn from(value: u32) -> Self {
        match value {
            1 => Self::Expired,
            2 => Self::DismissedByUser,
            3 => Self::Closed,
            _ => Self::Unknown,
        }
    }
}

/// Server capabilities as defined in the specification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Capabilities {
    /// Supports using icons instead of text for displaying actions.
    ///
    /// Icon usage requires per-notification activation via the "action-icons" hint.
    ActionIcons,
    /// The server will provide the specified actions to the user. Even if this cap is missing,
    /// actions may still be specified by the client, however the server is free to ignore them.
    Actions,
    /// Supports body text.
    ///
    /// Some implementations may only show the summary (for instance, onscreen displays,
    /// marquee/scrollers).
    Body,
    /// The server supports hyperlinks in the notifications.
    BodyHyperlinks,
    /// The server supports images in the notifications.
    BodyImages,
    /// Supports markup in the body text.
    ///
    /// If marked up text is sent to a server that does not provide this capability, the markup
    /// appears as regular text and requires client-side stripping.
    BodyMarkup,
    /// Indicates the server renders animations from all frames in a given image array.
    ///
    /// Clients may specify multiple frames even if this capability and/or "icon-static"
    /// is missing, though the server may ignore them and use only the primary frame.
    IconMulti,
    /// Supports display of exactly 1 frame of any given image array.
    ///
    /// This capability is mutually exclusive with "icon-multi"; specifying both is a
    /// protocol error.
    IconStatic,
    /// Indicates the server supports persistence of notifications.
    ///
    /// Notifications are retained until acknowledged or removed by the user, or recalled
    /// by the sender. This capability allows clients to rely on the server to ensure a
    /// notification is seen, eliminating the need for client-side reminding functions
    /// (such as status icons).
    Persistence,
    /// Indicates the server supports sounds on notifications.
    ///
    /// When present, the server also supports the "sound-file" and "suppress-sound" hints.
    Sound,
    /// Vendor-specific capability.
    Vendor(String),
}

impl FromStr for Capabilities {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "action-icons" => Self::ActionIcons,
            "actions" => Self::Actions,
            "body" => Self::Body,
            "body-hyperlinks" => Self::BodyHyperlinks,
            "body-images" => Self::BodyImages,
            "body-markup" => Self::BodyMarkup,
            "icon-multi" => Self::IconMulti,
            "icon-static" => Self::IconStatic,
            "persistence" => Self::Persistence,
            "sound" => Self::Sound,
            s if s.starts_with("x-") => Self::Vendor(s.to_string()),
            _ => Self::Vendor(format!("x-unknown-{s}")),
        })
    }
}

impl Capabilities {
    /// Convert to string representation for D-Bus.
    pub fn as_str(&self) -> &str {
        match self {
            Self::ActionIcons => "action-icons",
            Self::Actions => "actions",
            Self::Body => "body",
            Self::BodyHyperlinks => "body-hyperlinks",
            Self::BodyImages => "body-images",
            Self::BodyMarkup => "body-markup",
            Self::IconMulti => "icon-multi",
            Self::IconStatic => "icon-static",
            Self::Persistence => "persistence",
            Self::Sound => "sound",
            Self::Vendor(s) => s,
        }
    }
}

impl fmt::Display for Capabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Standard notification categories.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    /// A generic audio or video call notification that doesn't fit into any other category.
    Call,
    /// An audio or video call was ended.
    CallEnded,
    /// A audio or video call is incoming.
    CallIncoming,
    /// An incoming audio or video call was not answered.
    CallUnanswered,
    /// An audio or video call is ongoing (portal v2).
    CallOngoing,
    /// A generic device-related notification that doesn't fit into any other category.
    Device,
    /// A device, such as a USB device, was added to the system.
    DeviceAdded,
    /// A device had some kind of error.
    DeviceError,
    /// A device, such as a USB device, was removed from the system.
    DeviceRemoved,
    /// A generic e-mail-related notification that doesn't fit into any other category.
    Email,
    /// A new e-mail notification.
    EmailArrived,
    /// A notification stating that an e-mail has bounced.
    EmailBounced,
    /// A generic instant message-related notification that doesn't fit into any other category.
    Im,
    /// An instant message error notification.
    ImError,
    /// A received instant message notification.
    ImReceived,
    /// A generic network notification that doesn't fit into any other category.
    Network,
    /// A network connection notification, such as successful sign-on to a network service.
    ///
    /// Distinct from `device.added` for new network devices.
    NetworkConnected,
    /// A network disconnected notification.
    ///
    /// Distinct from `device.removed` for disconnected network devices.
    NetworkDisconnected,
    /// A network-related or connection-related error.
    NetworkError,
    /// A generic presence change notification that doesn't fit into any other category,
    /// such as going away or idle.
    Presence,
    /// An offline presence change notification.
    PresenceOffline,
    /// An online presence change notification.
    PresenceOnline,
    /// A generic file transfer or download notification that doesn't fit into any other category.
    Transfer,
    /// A file transfer or download complete notification.
    TransferComplete,
    /// A file transfer or download error.
    TransferError,
    /// A web notification from a browser (portal v2) — a top desktop notification source.
    BrowserWebNotification,
    /// A low-battery warning (portal v2).
    OsBatteryLow,
    /// An alarm is ringing (portal v2).
    AlarmRinging,
    /// An extreme weather warning (portal v2).
    WeatherWarningExtreme,
    /// A presidential-level cell-broadcast alert (portal v2).
    CellbroadcastDangerPresidential,
    /// An extreme-danger cell-broadcast alert (portal v2).
    CellbroadcastDangerExtreme,
    /// A severe-danger cell-broadcast alert (portal v2).
    CellbroadcastDangerSevere,
    /// A public-safety cell-broadcast alert (portal v2).
    CellbroadcastPublicSafety,
    /// An AMBER (child-abduction) cell-broadcast alert (portal v2).
    CellbroadcastAmberAlert,
    /// A cell-broadcast test message (portal v2).
    CellbroadcastTest,
    /// Vendor-specific category.
    Vendor(String),
}

impl FromStr for Category {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "call" => Self::Call,
            "call.ended" => Self::CallEnded,
            "call.incoming" => Self::CallIncoming,
            "call.unanswered" => Self::CallUnanswered,
            "call.ongoing" => Self::CallOngoing,
            "device" => Self::Device,
            "device.added" => Self::DeviceAdded,
            "device.error" => Self::DeviceError,
            "device.removed" => Self::DeviceRemoved,
            "email" => Self::Email,
            "email.arrived" => Self::EmailArrived,
            "email.bounced" => Self::EmailBounced,
            "im" => Self::Im,
            "im.error" => Self::ImError,
            "im.received" => Self::ImReceived,
            "network" => Self::Network,
            "network.connected" => Self::NetworkConnected,
            "network.disconnected" => Self::NetworkDisconnected,
            "network.error" => Self::NetworkError,
            "presence" => Self::Presence,
            "presence.offline" => Self::PresenceOffline,
            "presence.online" => Self::PresenceOnline,
            "transfer" => Self::Transfer,
            "transfer.complete" => Self::TransferComplete,
            "transfer.error" => Self::TransferError,
            "browser.web-notification" => Self::BrowserWebNotification,
            "os.battery.low" => Self::OsBatteryLow,
            "alarm.ringing" => Self::AlarmRinging,
            "weather.warning.extreme" => Self::WeatherWarningExtreme,
            "cellbroadcast.danger.presidential" => Self::CellbroadcastDangerPresidential,
            "cellbroadcast.danger.extreme" => Self::CellbroadcastDangerExtreme,
            "cellbroadcast.danger.severe" => Self::CellbroadcastDangerSevere,
            "cellbroadcast.public-safety" => Self::CellbroadcastPublicSafety,
            "cellbroadcast.amber-alert" => Self::CellbroadcastAmberAlert,
            "cellbroadcast.test" => Self::CellbroadcastTest,
            s if s.starts_with("x-") => Self::Vendor(s.to_string()),
            _ => Self::Vendor(format!("x-unknown-{s}")),
        })
    }
}

impl Category {
    /// The category strings this server recognizes as typed variants. Advertised via the portal
    /// `SupportedOptions["category"]` so the frontend doesn't down-convert / strip categories it
    /// can't confirm are handled. Kept in sync with [`FromStr`](Self::from_str).
    pub const KNOWN: &'static [&'static str] = &[
        "call",
        "call.ended",
        "call.incoming",
        "call.unanswered",
        "call.ongoing",
        "device",
        "device.added",
        "device.error",
        "device.removed",
        "email",
        "email.arrived",
        "email.bounced",
        "im",
        "im.error",
        "im.received",
        "network",
        "network.connected",
        "network.disconnected",
        "network.error",
        "presence",
        "presence.offline",
        "presence.online",
        "transfer",
        "transfer.complete",
        "transfer.error",
        "browser.web-notification",
        "os.battery.low",
        "alarm.ringing",
        "weather.warning.extreme",
        "cellbroadcast.danger.presidential",
        "cellbroadcast.danger.extreme",
        "cellbroadcast.danger.severe",
        "cellbroadcast.public-safety",
        "cellbroadcast.amber-alert",
        "cellbroadcast.test",
    ];

    /// The exact category taxonomy defined by `org.freedesktop.portal.Notification` v2 — what the
    /// portal backend advertises via `SupportedOptions["category"]`. This is a specific subset
    /// (and superset) of the freedesktop set: it excludes fdo-only categories (email/device/
    /// network/presence/transfer/…) and includes the portal-only emergency ones. Keep in sync
    /// with the interface XML.
    pub const PORTAL: &'static [&'static str] = &[
        "im.received",
        "alarm.ringing",
        "call.incoming",
        "call.ongoing",
        "call.unanswered",
        "weather.warning.extreme",
        "cellbroadcast.danger.presidential",
        "cellbroadcast.danger.extreme",
        "cellbroadcast.danger.severe",
        "cellbroadcast.public-safety",
        "cellbroadcast.amber-alert",
        "cellbroadcast.test",
        "os.battery.low",
        "browser.web-notification",
    ];

    /// Convert to string representation for hints.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Call => "call",
            Self::CallEnded => "call.ended",
            Self::CallIncoming => "call.incoming",
            Self::CallUnanswered => "call.unanswered",
            Self::CallOngoing => "call.ongoing",
            Self::Device => "device",
            Self::DeviceAdded => "device.added",
            Self::DeviceError => "device.error",
            Self::DeviceRemoved => "device.removed",
            Self::Email => "email",
            Self::EmailArrived => "email.arrived",
            Self::EmailBounced => "email.bounced",
            Self::Im => "im",
            Self::ImError => "im.error",
            Self::ImReceived => "im.received",
            Self::Network => "network",
            Self::NetworkConnected => "network.connected",
            Self::NetworkDisconnected => "network.disconnected",
            Self::NetworkError => "network.error",
            Self::Presence => "presence",
            Self::PresenceOffline => "presence.offline",
            Self::PresenceOnline => "presence.online",
            Self::Transfer => "transfer",
            Self::TransferComplete => "transfer.complete",
            Self::TransferError => "transfer.error",
            Self::BrowserWebNotification => "browser.web-notification",
            Self::OsBatteryLow => "os.battery.low",
            Self::AlarmRinging => "alarm.ringing",
            Self::WeatherWarningExtreme => "weather.warning.extreme",
            Self::CellbroadcastDangerPresidential => "cellbroadcast.danger.presidential",
            Self::CellbroadcastDangerExtreme => "cellbroadcast.danger.extreme",
            Self::CellbroadcastDangerSevere => "cellbroadcast.danger.severe",
            Self::CellbroadcastPublicSafety => "cellbroadcast.public-safety",
            Self::CellbroadcastAmberAlert => "cellbroadcast.amber-alert",
            Self::CellbroadcastTest => "cellbroadcast.test",
            Self::Vendor(s) => s,
        }
    }
}

/// The purpose of a notification button (XDG portal v2), letting a shell treat certain
/// buttons specially (e.g. accent an accept/decline, or an alarm's custom alert). These are
/// the seven spec-defined purposes; anything else is preserved verbatim. Portal-only;
/// freedesktop/GNotification buttons have no purpose.
///
/// Note `im.reply-with-text` is *not* represented here as a button — the crate lifts it into
/// [`Actions::reply`](crate::core::types::Actions::reply) (an inline text-reply affordance)
/// rather than a plain button; the variant exists only so the adapter can recognize it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ButtonPurpose {
    /// A custom system alert (e.g. an alarm or timer), defined outside the categories.
    SystemCustomAlert,
    /// An inline text reply (IM). Lifted into [`Actions::reply`](crate::core::types::Actions),
    /// not surfaced as a button.
    ImReplyWithText,
    /// Accept an incoming call.
    CallAccept,
    /// Decline an incoming call.
    CallDecline,
    /// Hang up an ongoing call.
    CallHangUp,
    /// Enable the speakerphone for an ongoing call.
    CallEnableSpeakerphone,
    /// Disable the speakerphone for an ongoing call.
    CallDisableSpeakerphone,
    /// Any other (e.g. vendor `x-*`) purpose, preserved verbatim.
    Other(String),
}

impl FromStr for ButtonPurpose {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "system.custom-alert" => Self::SystemCustomAlert,
            "im.reply-with-text" => Self::ImReplyWithText,
            "call.accept" => Self::CallAccept,
            "call.decline" => Self::CallDecline,
            "call.hang-up" => Self::CallHangUp,
            "call.enable-speakerphone" => Self::CallEnableSpeakerphone,
            "call.disable-speakerphone" => Self::CallDisableSpeakerphone,
            other => Self::Other(other.to_owned()),
        })
    }
}

impl ButtonPurpose {
    /// The purpose's string form.
    pub fn as_str(&self) -> &str {
        match self {
            Self::SystemCustomAlert => "system.custom-alert",
            Self::ImReplyWithText => "im.reply-with-text",
            Self::CallAccept => "call.accept",
            Self::CallDecline => "call.decline",
            Self::CallHangUp => "call.hang-up",
            Self::CallEnableSpeakerphone => "call.enable-speakerphone",
            Self::CallDisableSpeakerphone => "call.disable-speakerphone",
            Self::Other(purpose) => purpose,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urgency_from_u8_with_zero_returns_low() {
        let result = Urgency::from(0);

        assert_eq!(result, Urgency::Low);
    }

    #[test]
    fn urgency_from_u8_with_two_returns_critical() {
        let result = Urgency::from(2);

        assert_eq!(result, Urgency::Critical);
    }

    #[test]
    fn urgency_from_u8_with_one_returns_normal() {
        let result = Urgency::from(1);

        assert_eq!(result, Urgency::Normal);
    }

    #[test]
    fn urgency_from_u8_with_five_returns_normal() {
        let result = Urgency::from(5);

        assert_eq!(result, Urgency::Normal);
    }

    #[test]
    fn priority_from_str_maps_all_four_levels() {
        assert_eq!("low".parse::<Priority>().unwrap(), Priority::Low);
        assert_eq!("normal".parse::<Priority>().unwrap(), Priority::Normal);
        assert_eq!("high".parse::<Priority>().unwrap(), Priority::High);
        assert_eq!("urgent".parse::<Priority>().unwrap(), Priority::Urgent);
        assert_eq!("bogus".parse::<Priority>().unwrap(), Priority::Normal);
    }

    #[test]
    fn priority_u8_round_trips() {
        for priority in [
            Priority::Low,
            Priority::Normal,
            Priority::High,
            Priority::Urgent,
        ] {
            assert_eq!(Priority::from(priority as u8), priority);
        }
    }

    #[test]
    fn priority_projects_to_urgency_collapsing_high() {
        // High has no freedesktop equivalent and must collapse to Normal (prior behavior).
        assert_eq!(Urgency::from(Priority::Low), Urgency::Low);
        assert_eq!(Urgency::from(Priority::Normal), Urgency::Normal);
        assert_eq!(Urgency::from(Priority::High), Urgency::Normal);
        assert_eq!(Urgency::from(Priority::Urgent), Urgency::Critical);
    }

    #[test]
    fn urgency_lifts_to_priority_without_high() {
        // freedesktop urgencies round-trip through Priority (Critical <-> Urgent).
        assert_eq!(Priority::from(Urgency::Low), Priority::Low);
        assert_eq!(Priority::from(Urgency::Normal), Priority::Normal);
        assert_eq!(Priority::from(Urgency::Critical), Priority::Urgent);
    }

    #[test]
    fn closed_reason_from_u32_with_one_returns_expired() {
        let result = ClosedReason::from(1);

        assert_eq!(result, ClosedReason::Expired);
    }

    #[test]
    fn closed_reason_from_u32_with_two_returns_dismissed_by_user() {
        let result = ClosedReason::from(2);

        assert_eq!(result, ClosedReason::DismissedByUser);
    }

    #[test]
    fn closed_reason_from_u32_with_three_returns_closed() {
        let result = ClosedReason::from(3);

        assert_eq!(result, ClosedReason::Closed);
    }

    #[test]
    fn closed_reason_from_u32_with_zero_returns_unknown() {
        let result = ClosedReason::from(0);

        assert_eq!(result, ClosedReason::Unknown);
    }

    #[test]
    fn closed_reason_from_u32_with_five_returns_unknown() {
        let result = ClosedReason::from(5);

        assert_eq!(result, ClosedReason::Unknown);
    }

    #[test]
    fn capabilities_from_str_with_action_icons_returns_correct_variant() {
        let result = "action-icons".parse::<Capabilities>().unwrap();

        assert_eq!(result, Capabilities::ActionIcons);
    }

    #[test]
    fn capabilities_from_str_with_actions_returns_correct_variant() {
        let result = "actions".parse::<Capabilities>().unwrap();

        assert_eq!(result, Capabilities::Actions);
    }

    #[test]
    fn capabilities_from_str_with_persistence_returns_correct_variant() {
        let result = "persistence".parse::<Capabilities>().unwrap();

        assert_eq!(result, Capabilities::Persistence);
    }

    #[test]
    fn capabilities_from_str_with_vendor_prefix_returns_vendor() {
        let result = "x-custom-cap".parse::<Capabilities>().unwrap();

        assert_eq!(result, Capabilities::Vendor("x-custom-cap".to_string()));
    }

    #[test]
    fn capabilities_from_str_with_unknown_wraps_in_vendor_format() {
        let result = "unknown-capability".parse::<Capabilities>().unwrap();

        assert_eq!(
            result,
            Capabilities::Vendor("x-unknown-unknown-capability".to_string())
        );
    }

    #[test]
    fn capabilities_as_str_for_action_icons_returns_correct_string() {
        let cap = Capabilities::ActionIcons;

        let result = cap.as_str();

        assert_eq!(result, "action-icons");
    }

    #[test]
    fn capabilities_as_str_for_persistence_returns_correct_string() {
        let cap = Capabilities::Persistence;

        let result = cap.as_str();

        assert_eq!(result, "persistence");
    }

    #[test]
    fn capabilities_as_str_with_vendor_returns_inner_string() {
        let cap = Capabilities::Vendor("x-custom".to_string());

        let result = cap.as_str();

        assert_eq!(result, "x-custom");
    }

    #[test]
    fn category_from_str_with_call_returns_correct_variant() {
        let result = "call".parse::<Category>().unwrap();

        assert_eq!(result, Category::Call);
    }

    /// Guards the advertised `Category::KNOWN` list against drifting from `FromStr`/`as_str`:
    /// every advertised string must parse to a typed (non-`Vendor`) variant and round-trip.
    #[test]
    fn known_categories_parse_to_typed_variants_and_round_trip() {
        for string in Category::KNOWN {
            let category: Category = string.parse().expect("known category parses");
            assert!(
                !matches!(category, Category::Vendor(_)),
                "advertised category {string} fell through to Vendor"
            );
            assert_eq!(category.as_str(), *string, "{string} did not round-trip");
        }
    }

    #[test]
    fn category_from_str_with_email_arrived_returns_correct_variant() {
        let result = "email.arrived".parse::<Category>().unwrap();

        assert_eq!(result, Category::EmailArrived);
    }

    #[test]
    fn category_from_str_with_network_error_returns_correct_variant() {
        let result = "network.error".parse::<Category>().unwrap();

        assert_eq!(result, Category::NetworkError);
    }

    #[test]
    fn category_from_str_with_vendor_prefix_returns_vendor() {
        let result = "x-custom-category".parse::<Category>().unwrap();

        assert_eq!(result, Category::Vendor("x-custom-category".to_string()));
    }

    #[test]
    fn category_from_str_with_unknown_wraps_in_vendor_format() {
        let result = "unknown-category".parse::<Category>().unwrap();

        assert_eq!(
            result,
            Category::Vendor("x-unknown-unknown-category".to_string())
        );
    }

    #[test]
    fn category_as_str_for_call_returns_correct_string() {
        let cat = Category::Call;

        let result = cat.as_str();

        assert_eq!(result, "call");
    }

    #[test]
    fn category_as_str_for_email_arrived_returns_correct_string() {
        let cat = Category::EmailArrived;

        let result = cat.as_str();

        assert_eq!(result, "email.arrived");
    }

    #[test]
    fn category_as_str_with_vendor_returns_inner_string() {
        let cat = Category::Vendor("x-custom".to_string());

        let result = cat.as_str();

        assert_eq!(result, "x-custom");
    }
}
