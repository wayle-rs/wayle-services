//! Shared IWD type definitions.
//!
//! These mirror the equivalent `wayle-network` types so the UI layer can be
//! shared with minimal changes, while their constructors map from IWD's D-Bus
//! representation (string `Station.State`, string `Network.Type`,
//! `100 x dBm` signal strength) instead of NetworkManager's.

use std::sync::Arc;

use crate::{Error, Network};

/// What a station is doing, and with which network: IWD's `Station.State`,
/// with its `ConnectedNetwork`, which IWD sets as soon as a connection starts.
/// A failed connection isn't a state; it is recorded in
/// [`Station::last_error`](crate::Station::last_error).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    /// Not connected and not attempting a connection.
    #[default]
    Idle,
    /// Establishing a connection to `network`.
    Connecting {
        /// The network being connected to.
        network: Arc<Network>,
    },
    /// Connected to `network`.
    Connected {
        /// The network connected to.
        network: Arc<Network>,
    },
    /// Connected to `network` but roaming between access points. Treated as an
    /// active connection (signal strength stays meaningful); the UI may label it
    /// distinctly from [`Connected`](Self::Connected).
    Roaming {
        /// The network connected to.
        network: Arc<Network>,
    },
}

impl ConnectionState {
    /// Derives a connection state from IWD's raw `Station.State` string
    /// (`connected` / `connecting` / `disconnecting` / `disconnected` /
    /// `roaming`) and its `ConnectedNetwork`. Only the terminal
    /// `disconnected`/`disconnecting` clear to `Idle`.
    pub(crate) fn from_raw_state(state: &str, connected: Option<Arc<Network>>) -> Self {
        match state {
            "connected" => connected.map_or(Self::Idle, |network| Self::Connected { network }),
            "roaming" => connected.map_or(Self::Idle, |network| Self::Roaming { network }),
            "connecting" => connected.map_or(Self::Idle, |network| Self::Connecting { network }),
            _ => Self::Idle,
        }
    }

    /// The network of the active or in-progress connection, if any.
    pub fn network(&self) -> Option<&Arc<Network>> {
        match self {
            Self::Idle => None,
            Self::Connecting { network }
            | Self::Connected { network }
            | Self::Roaming { network } => Some(network),
        }
    }
}

/// A request a station was asked to make.
#[derive(Clone, PartialEq, Eq)]
pub enum StationAction {
    /// [`Network::connect`](crate::Network::connect) to `network`.
    Connect {
        /// The network connected to.
        network: Arc<Network>,
    },
    /// [`Station::disconnect`](crate::Station::disconnect).
    Disconnect,
    /// [`Station::scan`](crate::Station::scan).
    Scan,
    /// [`Station::set_powered`](crate::Station::set_powered).
    SetPowered,
    /// [`Network::forget`](crate::Network::forget) `network`.
    Forget {
        /// The network forgotten.
        network: Arc<Network>,
    },
}

/// Names the network by SSID and object path rather than dumping all of its
/// state, as this appears in logs.
impl std::fmt::Debug for StationAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let network = |name: &str, network: &Network, f: &mut std::fmt::Formatter<'_>| {
            f.debug_struct(name)
                .field("ssid", &network.ssid.get())
                .field("path", &network.object_path().as_str())
                .finish()
        };
        match self {
            Self::Connect { network: target } => network("Connect", target, f),
            Self::Forget { network: target } => network("Forget", target, f),
            Self::Disconnect => f.write_str("Disconnect"),
            Self::Scan => f.write_str("Scan"),
            Self::SetPowered => f.write_str("SetPowered"),
        }
    }
}

/// A request to IWD that failed.
///
/// IWD reports a failed request only in its error reply, so the service
/// records the most recent one on the station, where every consumer sees it.
#[derive(Debug, Clone)]
pub struct StationError {
    /// The request that failed.
    pub action: StationAction,
    /// The complete error, e.g. a [`zbus::Error::MethodError`] carrying IWD's
    /// error name (`net.connman.iwd.Failed`, `net.connman.iwd.Timeout`, ...).
    pub error: Arc<Error>,
}

impl StationError {
    pub(crate) fn new(action: StationAction, error: Error) -> Self {
        Self {
            action,
            error: Arc::new(error),
        }
    }

    /// Whether this is IWD refusing to turn WiFi on because a hardware switch
    /// (or the firmware) blocks the radio, which software can't lift. IWD
    /// reports a hardware block only this way, and not its end: the error
    /// stays until the next request, after the switch may have been flipped
    /// back.
    pub fn hardware_blocked(&self) -> bool {
        self.action == StationAction::SetPowered
            && self.error.is_iwd_error(crate::error::IWD_NOT_AVAILABLE)
    }
}

/// Two errors are equal only if they are the same recorded failure, so a new
/// failure is always seen as a change.
impl PartialEq for StationError {
    fn eq(&self, other: &Self) -> bool {
        self.action == other.action && Arc::ptr_eq(&self.error, &other.error)
    }
}

impl Eq for StationError {}

/// A passphrase IWD asks this service's agent for, to connect to `network`.
/// Answer it with [`IwdService::provide_passphrase`](crate::IwdService::provide_passphrase)
/// or turn it down with [`IwdService::cancel_passphrase_request`](crate::IwdService::cancel_passphrase_request).
#[derive(Debug, Clone)]
pub struct PassphraseRequest {
    /// The network being connected to.
    pub network: Arc<Network>,
    /// Whether IWD asks again because connecting with the passphrase this
    /// service last gave it for this network failed. IWD doesn't say why: the
    /// passphrase may be wrong, or the network may have dropped out meanwhile.
    pub rejected: bool,
}

impl PartialEq for PassphraseRequest {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.network, &other.network) && self.rejected == other.rejected
    }
}

impl Eq for PassphraseRequest {}

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
/// the dBm thresholds `-60`, `-67`, `-74` and `-81` (iwgtk's levels). Exposed instead of a raw percentage because
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
    /// `GetOrderedNetworks` value already divided by 100, as
    /// [`Network::signal`](crate::Network::signal) is).
    pub fn from_dbm(dbm: i16) -> Self {
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
        let (commands, _) = crate::dispatcher::command::Commands::channel();
        let network = Arc::new(Network::new(
            &commands,
            zbus::zvariant::OwnedObjectPath::try_from("/net/connman/iwd/0/4/6e6574_psk").unwrap(),
        ));
        let net = || Some(Arc::clone(&network));
        assert_eq!(
            ConnectionState::from_raw_state("connected", net()),
            ConnectionState::Connected {
                network: net().unwrap()
            }
        );
        // Roaming is its own state, still carrying the network.
        assert_eq!(
            ConnectionState::from_raw_state("roaming", net()),
            ConnectionState::Roaming {
                network: net().unwrap()
            }
        );
        assert_eq!(
            ConnectionState::from_raw_state("connecting", net()),
            ConnectionState::Connecting {
                network: net().unwrap()
            }
        );
        // Disconnecting/disconnected/unknown collapse to Idle.
        assert_eq!(
            ConnectionState::from_raw_state("disconnecting", net()),
            ConnectionState::Idle
        );
        assert_eq!(
            ConnectionState::from_raw_state("disconnected", None),
            ConnectionState::Idle
        );
        assert_eq!(
            ConnectionState::from_raw_state("connected", None),
            ConnectionState::Idle
        );
    }

    #[test]
    fn security_from_iwd_type() {
        assert_eq!(SecurityType::from_iwd_type("open"), SecurityType::None);
        assert_eq!(SecurityType::from_iwd_type("wep"), SecurityType::Wep);
        assert_eq!(SecurityType::from_iwd_type("psk"), SecurityType::Psk);
        assert_eq!(
            SecurityType::from_iwd_type("8021x"),
            SecurityType::Enterprise
        );
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
