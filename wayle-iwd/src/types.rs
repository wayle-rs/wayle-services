//! Shared IWD type definitions.
//!
//! These mirror the equivalent `wayle-network` types so the UI layer can be
//! shared with minimal changes, while their constructors map from IWD's D-Bus
//! representation (string `Station.State`, string `Network.Type`,
//! `100 x dBm` signal strength) instead of NetworkManager's.

/// Connection-attempt-aware view of what the station is doing with respect to a
/// specific network — the single reactive state model for the "active
/// connection" UI.
///
/// IWD's raw `net.connman.iwd.Station.State` does not name the *target* of an
/// in-progress attempt (during a transition `Station.ConnectedNetwork` may still
/// point at the previous network), and reports no failure as a state. This type
/// augments the raw state with the target SSID. It is purely the positive state;
/// a failed attempt is surfaced via the `Result` returned by
/// [`Station::connect`](crate::Station::connect), not held here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    /// Not connected and not attempting a connection.
    #[default]
    Idle,
    /// Establishing (or roaming to) a connection to `ssid`.
    Connecting {
        /// SSID being connected to.
        ssid: String,
    },
    /// Connected to `ssid`.
    Connected {
        /// SSID currently connected to.
        ssid: String,
    },
    /// Connected to `ssid` but roaming between access points. Treated as an
    /// active connection (signal strength stays meaningful); the UI may label it
    /// distinctly from [`Connected`](Self::Connected).
    Roaming {
        /// SSID currently connected to.
        ssid: String,
    },
}

impl ConnectionState {
    /// Derives a connection state from IWD's raw `Station.State` string
    /// (`connected` / `connecting` / `disconnecting` / `disconnected` /
    /// `roaming`) and the resolved `ConnectedNetwork` SSID. Only the terminal
    /// `disconnected`/`disconnecting` clear to `Idle`.
    pub(crate) fn from_raw_state(state: &str, connected_ssid: Option<String>) -> Self {
        match state {
            "connected" => connected_ssid.map_or(Self::Idle, |ssid| Self::Connected { ssid }),
            "roaming" => connected_ssid.map_or(Self::Idle, |ssid| Self::Roaming { ssid }),
            "connecting" => connected_ssid.map_or(Self::Idle, |ssid| Self::Connecting { ssid }),
            _ => Self::Idle,
        }
    }

    /// The SSID of the active or in-progress connection, if any.
    pub fn ssid(&self) -> Option<&str> {
        match self {
            Self::Idle => None,
            Self::Connecting { ssid } | Self::Connected { ssid } | Self::Roaming { ssid } => {
                Some(ssid)
            }
        }
    }
}

/// Security type classification for a network.
///
/// Variants mirror `wayle-network`'s `SecurityType` for UI compatibility.
/// IWD only distinguishes `open`, `wep`, `psk`, and `8021x`. Its `psk` type
/// covers WPA2 and WPA3 personal networks alike, so both are reported as the
/// ambiguous [`SecurityType::Psk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityType {
    /// No security (open network).
    None,
    /// WEP - deprecated and insecure.
    Wep,
    /// Pre-shared key (WPA2 or WPA3 personal) - reported for every IWD `psk`
    /// network, which does not distinguish the two.
    Psk,
    /// Enterprise security (802.1X).
    Enterprise,
}

impl SecurityType {
    /// Derives the security type from IWD's `Network.Type` string
    /// (`open` / `wep` / `psk` / `8021x`).
    pub(crate) fn from_iwd_type(network_type: &str) -> Self {
        match network_type {
            "wep" => Self::Wep,
            "psk" => Self::Psk,
            "8021x" => Self::Enterprise,
            _ => Self::None,
        }
    }
}

/// dBm thresholds (descending) partitioning RSSI into the five
/// [`SignalStrength`] buckets, matching iwgtk's levels. These are registered with
/// IWD's `SignalLevelAgent` so it pushes a level change whenever the connected
/// link's RSSI crosses one — the same thresholds therefore both define the
/// buckets and drive event-based strength updates.
pub(crate) const SIGNAL_STRENGTH_THRESHOLDS: [i16; 4] = [-60, -67, -74, -81];

/// Signal strength as a discrete bucket (weakest to strongest), partitioned by
/// [`SIGNAL_STRENGTH_THRESHOLDS`]. Exposed instead of a raw percentage because
/// IWD's `SignalLevelAgent` reports a bucketed level, and the UI only renders
/// per-bucket icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum SignalStrength {
    /// No usable signal.
    #[default]
    None,
    /// Weak signal.
    Weak,
    /// Acceptable signal.
    Ok,
    /// Good signal.
    Good,
    /// Excellent signal.
    Excellent,
}

impl SignalStrength {
    /// Number of buckets (one more than the threshold count).
    pub const COUNT: usize = SIGNAL_STRENGTH_THRESHOLDS.len() + 1;

    /// Bucket index, `0` (weakest) to `COUNT - 1` (strongest).
    pub fn index(self) -> usize {
        self as usize
    }

    /// Maps this bucket onto a list of `num_icons` weakest-first icons, scaling
    /// when the list size differs from [`COUNT`](Self::COUNT). Returns `None` for
    /// an empty list. With the common 4-icon list (no "none" entry) both
    /// [`None`](Self::None) and [`Weak`](Self::Weak) map to the weakest icon.
    ///
    /// This is the single definition of the bucket→icon-slot mapping shared by
    /// every UI surface (bar icon, dropdown card, network list).
    pub fn icon_index(self, num_icons: usize) -> Option<usize> {
        if num_icons == 0 {
            return None;
        }
        Some((self.index() * num_icons / Self::COUNT).min(num_icons - 1))
    }

    /// Buckets a plain-dBm RSSI value (e.g. from `GetDiagnostics`, or a
    /// `GetOrderedNetworks` value already divided by 100).
    pub(crate) fn from_dbm(dbm: i16) -> Self {
        let index = SIGNAL_STRENGTH_THRESHOLDS
            .iter()
            .filter(|&&threshold| dbm >= threshold)
            .count();
        Self::from_index(index)
    }

    /// Maps a `SignalLevelAgent` level (`0` = strongest, `N` = weakest) to a
    /// bucket. IWD's ordering is the reverse of our weakest-first index.
    pub(crate) fn from_level(level: u8) -> Self {
        let index = SIGNAL_STRENGTH_THRESHOLDS
            .len()
            .saturating_sub(usize::from(level));
        Self::from_index(index)
    }

    fn from_index(index: usize) -> Self {
        match index {
            0 => Self::None,
            1 => Self::Weak,
            2 => Self::Ok,
            3 => Self::Good,
            _ => Self::Excellent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_state_from_raw_state() {
        let net = || Some(String::from("net"));
        assert_eq!(
            ConnectionState::from_raw_state("connected", net()),
            ConnectionState::Connected { ssid: "net".into() }
        );
        // Roaming is its own state, still carrying the SSID.
        assert_eq!(
            ConnectionState::from_raw_state("roaming", net()),
            ConnectionState::Roaming { ssid: "net".into() }
        );
        assert_eq!(
            ConnectionState::from_raw_state("connecting", net()),
            ConnectionState::Connecting { ssid: "net".into() }
        );
        // Disconnecting/disconnected/unknown collapse to Idle.
        assert_eq!(ConnectionState::from_raw_state("disconnecting", net()), ConnectionState::Idle);
        assert_eq!(ConnectionState::from_raw_state("disconnected", None), ConnectionState::Idle);
        assert_eq!(ConnectionState::from_raw_state("connected", None), ConnectionState::Idle);
    }

    #[test]
    fn security_from_iwd_type() {
        assert_eq!(SecurityType::from_iwd_type("open"), SecurityType::None);
        assert_eq!(SecurityType::from_iwd_type("wep"), SecurityType::Wep);
        assert_eq!(SecurityType::from_iwd_type("psk"), SecurityType::Psk);
        assert_eq!(SecurityType::from_iwd_type("8021x"), SecurityType::Enterprise);
        assert_eq!(SecurityType::from_iwd_type("other"), SecurityType::None);
    }

    #[test]
    fn signal_strength_from_dbm() {
        assert_eq!(SignalStrength::from_dbm(-55), SignalStrength::Excellent);
        assert_eq!(SignalStrength::from_dbm(-60), SignalStrength::Excellent); // boundary: >= -60
        assert_eq!(SignalStrength::from_dbm(-65), SignalStrength::Good);
        assert_eq!(SignalStrength::from_dbm(-70), SignalStrength::Ok);
        assert_eq!(SignalStrength::from_dbm(-78), SignalStrength::Weak);
        assert_eq!(SignalStrength::from_dbm(-85), SignalStrength::None);
    }

    #[test]
    fn signal_strength_from_agent_level() {
        // IWD level 0 = strongest .. 4 = weakest, the reverse of our index.
        assert_eq!(SignalStrength::from_level(0), SignalStrength::Excellent);
        assert_eq!(SignalStrength::from_level(1), SignalStrength::Good);
        assert_eq!(SignalStrength::from_level(2), SignalStrength::Ok);
        assert_eq!(SignalStrength::from_level(3), SignalStrength::Weak);
        assert_eq!(SignalStrength::from_level(4), SignalStrength::None);
        assert_eq!(SignalStrength::from_level(99), SignalStrength::None); // clamps
    }

    #[test]
    fn signal_strength_icon_index() {
        // 4-icon list (no "none"): None and Weak collapse to the weakest slot.
        assert_eq!(SignalStrength::None.icon_index(4), Some(0));
        assert_eq!(SignalStrength::Weak.icon_index(4), Some(0));
        assert_eq!(SignalStrength::Ok.icon_index(4), Some(1));
        assert_eq!(SignalStrength::Good.icon_index(4), Some(2));
        assert_eq!(SignalStrength::Excellent.icon_index(4), Some(3));
        // 5-icon list maps one bucket per icon.
        assert_eq!(SignalStrength::None.icon_index(5), Some(0));
        assert_eq!(SignalStrength::Excellent.icon_index(5), Some(4));
        // Empty list has no slot.
        assert_eq!(SignalStrength::Ok.icon_index(0), None);
    }

    #[test]
    fn signal_strength_index_round_trips_levels() {
        // from_dbm and from_level agree on the same RSSI bucket.
        assert_eq!(SignalStrength::Excellent.index(), SignalStrength::COUNT - 1);
        assert_eq!(SignalStrength::None.index(), 0);
        assert_eq!(SignalStrength::from_dbm(-65), SignalStrength::from_level(1));
    }
}
