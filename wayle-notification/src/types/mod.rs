//! Types from the freedesktop.org Desktop Notifications Specification.

mod notification;

pub use notification::*;

pub(crate) mod dbus {
    pub const SERVICE_NAME: &str = "org.freedesktop.Notifications";
    pub const SERVICE_PATH: &str = "/org/freedesktop/Notifications";
    pub const SERVICE_INTERFACE: &str = "org.freedesktop.Notifications";
    pub const WAYLE_SERVICE_NAME: &str = "com.wayle.Notifications1";
    pub const WAYLE_SERVICE_PATH: &str = "/com/wayle/Notifications";
    pub const GTK_SERVICE_NAME: &str = "org.gtk.Notifications";
    pub const GTK_SERVICE_PATH: &str = "/org/gtk/Notifications";
}

pub(crate) type Name = String;
pub(crate) type Vendor = String;
pub(crate) type Version = String;
pub(crate) type SpecVersion = String;

pub(crate) enum Signal {
    NotificationClosed,
    ActionInvoked,
    /// KDE inline-reply result: `NotificationReplied(u id, s text)`.
    NotificationReplied,
    /// freedesktop `ActivationToken(u id, s token)` — the focus token, emitted before
    /// `ActionInvoked` so the app may raise its window (see the freedesktop backend).
    ActivationToken,
}

impl Signal {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Signal::NotificationClosed => "NotificationClosed",
            Signal::ActionInvoked => "ActionInvoked",
            Signal::NotificationReplied => "NotificationReplied",
            Signal::ActivationToken => "ActivationToken",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_notification_closed_returns_correct_str() {
        let signal = Signal::NotificationClosed;

        let result = signal.as_str();

        assert_eq!(result, "NotificationClosed");
    }

    #[test]
    fn signal_action_invoked_returns_correct_str() {
        let signal = Signal::ActionInvoked;

        let result = signal.as_str();

        assert_eq!(result, "ActionInvoked");
    }

    #[test]
    fn signal_activation_token_returns_correct_str() {
        let signal = Signal::ActivationToken;

        let result = signal.as_str();

        assert_eq!(result, "ActivationToken");
    }
}
