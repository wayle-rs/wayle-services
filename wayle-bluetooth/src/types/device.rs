use std::{
    self,
    fmt::{Display, Formatter, Result},
};

/// Preferred bearer for dual-mode Bluetooth devices.
///
/// (BlueZ experimental)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreferredBearer {
    /// Connect to last used bearer first (default)
    #[default]
    LastUsed,
    /// Connect to BR/EDR first
    BrEdr,
    /// Connect to LE first
    Le,
    /// Connect to last seen bearer first
    LastSeen,
}

impl From<&str> for PreferredBearer {
    fn from(s: &str) -> Self {
        match s {
            "last-used" => Self::LastUsed,
            "bredr" => Self::BrEdr,
            "le" => Self::Le,
            "last-seen" => Self::LastSeen,
            _ => Self::LastUsed,
        }
    }
}

impl Display for PreferredBearer {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::LastUsed => write!(f, "last-used"),
            Self::BrEdr => write!(f, "bredr"),
            Self::Le => write!(f, "le"),
            Self::LastSeen => write!(f, "last-seen"),
        }
    }
}

/// An operation this service is currently performing on a device.
///
/// BlueZ publishes the outcome of these operations (`Connected`, `Paired`, the
/// device disappearing) but not that one is underway, so the service tracks the
/// operations it started itself: [`Device::connect`](crate::core::device::Device::connect)
/// and friends set it when they send the request. It returns to `Idle` when
/// BlueZ reports the outcome, or when the request fails. Operations started by
/// another BlueZ client (e.g. `bluetoothctl`) are not reflected.
///
/// BlueZ occasionally drops a connect without ever replying: when it falls
/// back to LE after a BR/EDR attempt fails, or when another client disconnects
/// the device while its services are being looked up. If the device's state
/// doesn't change either, `Connecting` stays until
/// [`Device::disconnect`](crate::core::device::Device::disconnect) cancels it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceActivity {
    /// No operation in progress.
    #[default]
    Idle,
    /// A [`connect`](crate::core::device::Device::connect) call is in flight.
    Connecting,
    /// A [`disconnect`](crate::core::device::Device::disconnect) call is in flight.
    Disconnecting,
    /// A [`pair`](crate::core::device::Device::pair) call is in flight.
    Pairing,
    /// A [`forget`](crate::core::device::Device::forget) call is in flight.
    Forgetting,
}

/// A request that can be made of a [`Device`](crate::core::device::Device).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceAction {
    /// [`connect`](crate::core::device::Device::connect)
    Connect,
    /// [`disconnect`](crate::core::device::Device::disconnect)
    Disconnect,
    /// [`connect_profile`](crate::core::device::Device::connect_profile)
    ConnectProfile,
    /// [`disconnect_profile`](crate::core::device::Device::disconnect_profile)
    DisconnectProfile,
    /// [`pair`](crate::core::device::Device::pair)
    Pair,
    /// [`cancel_pairing`](crate::core::device::Device::cancel_pairing)
    CancelPairing,
    /// [`forget`](crate::core::device::Device::forget)
    Forget,
    /// [`set_trusted`](crate::core::device::Device::set_trusted)
    SetTrusted,
    /// [`set_blocked`](crate::core::device::Device::set_blocked)
    SetBlocked,
    /// [`set_wake_allowed`](crate::core::device::Device::set_wake_allowed)
    SetWakeAllowed,
    /// [`set_alias`](crate::core::device::Device::set_alias)
    SetAlias,
    /// [`set_preferred_bearer`](crate::core::device::Device::set_preferred_bearer)
    SetPreferredBearer,
}

impl DeviceAction {
    /// Whether a request for this action can involve pairing: BlueZ pairs an
    /// unpaired device to connect it, and answers only once that's over.
    pub(crate) fn may_pair(self) -> bool {
        matches!(self, Self::Connect | Self::ConnectProfile | Self::Pair)
    }

    /// The activity a request for this action shows while it's under way, if
    /// it's one BlueZ reports the outcome of.
    pub(crate) fn activity(self) -> Option<DeviceActivity> {
        match self {
            Self::Connect => Some(DeviceActivity::Connecting),
            Self::Disconnect => Some(DeviceActivity::Disconnecting),
            Self::Pair => Some(DeviceActivity::Pairing),
            Self::Forget => Some(DeviceActivity::Forgetting),
            Self::ConnectProfile
            | Self::DisconnectProfile
            | Self::CancelPairing
            | Self::SetTrusted
            | Self::SetBlocked
            | Self::SetWakeAllowed
            | Self::SetAlias
            | Self::SetPreferredBearer => None,
        }
    }
}

/// A failed [`DeviceAction`].
pub type DeviceError = super::ActionError<DeviceAction>;

/// Bluetooth device disconnection reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    /// Unknown reason
    Unknown,
    /// Connection timeout
    ConnectionTimeout,
    /// Connection terminated by local host
    ConnectionTerminatedLocal,
    /// Connection terminated by remote host
    ConnectionTerminatedRemote,
    /// Authentication failure
    AuthenticationFailure,
    /// Connection terminated due to suspend
    Suspend,
}

impl From<&str> for DisconnectReason {
    fn from(s: &str) -> Self {
        match s {
            "org.bluez.Reason.Timeout" => Self::ConnectionTimeout,
            "org.bluez.Reason.Local" => Self::ConnectionTerminatedLocal,
            "org.bluez.Reason.Remote" => Self::ConnectionTerminatedRemote,
            "org.bluez.Reason.Authentication" => Self::AuthenticationFailure,
            "org.bluez.Reason.Suspend" => Self::Suspend,
            _ => Self::Unknown,
        }
    }
}

impl Display for DisconnectReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Unknown => write!(f, "Unknown"),
            Self::ConnectionTimeout => write!(f, "Connection timeout"),
            Self::ConnectionTerminatedLocal => write!(f, "Connection terminated by local host"),
            Self::ConnectionTerminatedRemote => write!(f, "Connection terminated by remote host"),
            Self::AuthenticationFailure => write!(f, "Authentication failure"),
            Self::Suspend => write!(f, "Suspend"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_bearer_from_str_handles_all_variants() {
        assert_eq!(
            PreferredBearer::from("last-used"),
            PreferredBearer::LastUsed
        );
        assert_eq!(PreferredBearer::from("bredr"), PreferredBearer::BrEdr);
        assert_eq!(PreferredBearer::from("le"), PreferredBearer::Le);
        assert_eq!(
            PreferredBearer::from("last-seen"),
            PreferredBearer::LastSeen
        );
    }

    #[test]
    fn preferred_bearer_from_str_defaults_to_last_used() {
        assert_eq!(PreferredBearer::from("unknown"), PreferredBearer::LastUsed);
        assert_eq!(PreferredBearer::from(""), PreferredBearer::LastUsed);
    }

    #[test]
    fn disconnect_reason_from_str_handles_all_variants() {
        assert_eq!(
            DisconnectReason::from("org.bluez.Reason.Timeout"),
            DisconnectReason::ConnectionTimeout
        );
        assert_eq!(
            DisconnectReason::from("org.bluez.Reason.Local"),
            DisconnectReason::ConnectionTerminatedLocal
        );
        assert_eq!(
            DisconnectReason::from("org.bluez.Reason.Remote"),
            DisconnectReason::ConnectionTerminatedRemote
        );
        assert_eq!(
            DisconnectReason::from("org.bluez.Reason.Authentication"),
            DisconnectReason::AuthenticationFailure
        );
        assert_eq!(
            DisconnectReason::from("org.bluez.Reason.Suspend"),
            DisconnectReason::Suspend
        );
    }

    #[test]
    fn disconnect_reason_from_str_defaults_to_unknown() {
        assert_eq!(DisconnectReason::from("unknown"), DisconnectReason::Unknown);
        assert_eq!(DisconnectReason::from(""), DisconnectReason::Unknown);
    }
}
