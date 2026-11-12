//! Public, backend-independent types for the Mullvad service.
//!
//! These form the stable surface of the crate: they are what
//! [`Property`](wayle_core::Property) fields carry and what the control methods
//! accept, and they are deliberately independent of any daemon wire schema.

use serde::{Deserialize, Serialize};

/// A relay location for display — either the connected relay or the selected
/// one.
///
/// Carries human-readable names (already resolved from the daemon's internal
/// codes) plus the ISO country code for flag lookup, so consumers render it
/// directly without knowing anything about the daemon's wire representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayLocation {
    /// ISO country code, e.g. `"se"` — for flag lookup.
    pub country_code: String,
    /// Human-readable country, e.g. `"Sweden"`.
    pub country: String,
    /// Human-readable city, e.g. `"Gothenburg"`, when known.
    pub city: Option<String>,
    /// Relay hostname, e.g. `"se-got-wg-001"`, when a specific relay is pinned.
    pub hostname: Option<String>,
}

/// The overall VPN status to display: the tunnel state plus the active relay,
/// with the account/login state overlaid on top.
///
/// State and relay are folded into one value so illegal combinations (connected
/// with no relay, disconnected with a stale one) are unrepresentable. The
/// account login state is also folded in and takes **precedence**: while logged
/// out or revoked the tunnel cannot be up, so those variants override any (stale)
/// tunnel state. The *selected* relay is deliberately NOT part of this — it is
/// orthogonal, set by the client and persisting across states — and lives in its
/// own [`Mullvad::selected`](crate::Mullvad) property.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConnectionStatus {
    /// Not logged in to a Mullvad account (precedes any tunnel state).
    LoggedOut,
    /// The account's device was revoked; the user must re-authenticate
    /// (precedes any tunnel state).
    Revoked,
    /// Logged in; no tunnel is up.
    #[default]
    Disconnected,
    /// Establishing a tunnel; carries the target relay once the daemon reports it.
    Connecting(Option<RelayLocation>),
    /// A tunnel is up to this relay (the daemon always reports one here).
    Connected(RelayLocation),
    /// The tunnel is being torn down.
    Disconnecting,
    /// The daemon is in a blocked/error state, with the cause.
    Error(ErrorCause),
}

impl ConnectionStatus {
    /// The active relay, when the daemon has reported one for the current state
    /// (connecting or connected).
    #[must_use]
    pub fn relay(&self) -> Option<&RelayLocation> {
        match self {
            Self::Connecting(relay) => relay.as_ref(),
            Self::Connected(relay) => Some(relay),
            Self::LoggedOut
            | Self::Revoked
            | Self::Disconnected
            | Self::Disconnecting
            | Self::Error(_) => None,
        }
    }
}

/// Why the tunnel is in a blocked/error state — the display-relevant subset of
/// the daemon's error causes. Unmapped/future causes fold into [`Other`](Self::Other).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ErrorCause {
    /// Account authentication failed (invalid/expired account, or device limit).
    AuthFailed,
    /// The device is offline (no network connectivity).
    Offline,
    /// Any other blocking/error cause (firewall, DNS, tunnel setup, …).
    #[default]
    Other,
}

/// Account login state, folded into [`ConnectionStatus`] (`LoggedOut`/`Revoked`).
/// Used at the backend boundary; not a public property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginState {
    /// An account is logged in.
    LoggedIn,
    /// No account is logged in.
    LoggedOut,
    /// The device was revoked server-side.
    Revoked,
}

/// A country in the available-network tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkCountry {
    /// Human-readable country name, e.g. `"Sweden"`.
    pub name: String,
    /// Country code, e.g. `"se"`.
    pub code: String,
    /// Cities within this country.
    pub cities: Vec<NetworkCity>,
}

/// A city within a [`NetworkCountry`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkCity {
    /// Human-readable city name, e.g. `"Gothenburg"`.
    pub name: String,
    /// City code, e.g. `"got"`.
    pub code: String,
    /// Individual networks (relays) in this city.
    pub networks: Vec<MullvadNetwork>,
}

/// A single Mullvad network (relay) that can be connected to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MullvadNetwork {
    /// Relay hostname, e.g. `"se-got-wg-001"`.
    pub hostname: String,
    /// Code of the country this network lives in, e.g. `"se"`.
    pub country_code: String,
    /// Code of the city this network lives in, e.g. `"got"`.
    pub city_code: String,
    /// Whether the relay is currently active/available.
    pub active: bool,
}

/// Selects what to connect to: a whole country, a city, or a specific relay.
///
/// A hierarchy of *codes* (not display names — those live in [`RelayLocation`]),
/// modelled as an enum so invalid combinations (e.g. a relay with no city) are
/// unrepresentable. Build with [`NetworkTarget::country`] /
/// [`NetworkTarget::city`] / [`NetworkTarget::relay`], or from a
/// [`MullvadNetwork`] via [`From`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkTarget {
    /// A whole country.
    Country {
        /// Country code, e.g. `"se"`.
        code: String,
    },
    /// A city within a country.
    City {
        /// Country code, e.g. `"se"`.
        country: String,
        /// City code, e.g. `"got"`.
        code: String,
    },
    /// A specific relay within a city and country.
    Relay {
        /// Country code, e.g. `"se"`.
        country: String,
        /// City code, e.g. `"got"`.
        city: String,
        /// Relay hostname, e.g. `"se-got-wg-001"`.
        hostname: String,
    },
}

impl NetworkTarget {
    /// Targets an entire country by its code (e.g. `"se"`).
    #[must_use]
    pub fn country(code: impl Into<String>) -> Self {
        Self::Country { code: code.into() }
    }

    /// Targets a city by its country and city codes (e.g. `"se"`, `"got"`).
    #[must_use]
    pub fn city(country: impl Into<String>, code: impl Into<String>) -> Self {
        Self::City {
            country: country.into(),
            code: code.into(),
        }
    }

    /// Targets a specific relay by country code, city code and hostname.
    #[must_use]
    pub fn relay(
        country: impl Into<String>,
        city: impl Into<String>,
        hostname: impl Into<String>,
    ) -> Self {
        Self::Relay {
            country: country.into(),
            city: city.into(),
            hostname: hostname.into(),
        }
    }
}

impl From<&MullvadNetwork> for NetworkTarget {
    fn from(network: &MullvadNetwork) -> Self {
        Self::Relay {
            country: network.country_code.clone(),
            city: network.city_code.clone(),
            hostname: network.hostname.clone(),
        }
    }
}

/// Derives the ISO country code from a Mullvad relay hostname
/// (e.g. `"se-got-wg-001"` → `"se"`), if it has the conventional 2-letter
/// country prefix.
pub(crate) fn country_code_from_hostname(hostname: &str) -> Option<String> {
    hostname
        .split('-')
        .next()
        .filter(|code| code.len() == 2 && code.chars().all(|c| c.is_ascii_alphabetic()))
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::{MullvadNetwork, NetworkTarget};

    #[test]
    fn country_target_is_country_variant() {
        assert_eq!(
            NetworkTarget::country("se"),
            NetworkTarget::Country {
                code: "se".to_owned()
            }
        );
    }

    #[test]
    fn city_target_is_city_variant() {
        assert_eq!(
            NetworkTarget::city("se", "got"),
            NetworkTarget::City {
                country: "se".to_owned(),
                code: "got".to_owned(),
            }
        );
    }

    #[test]
    fn relay_target_is_relay_variant() {
        assert_eq!(
            NetworkTarget::relay("se", "got", "se-got-wg-001"),
            NetworkTarget::Relay {
                country: "se".to_owned(),
                city: "got".to_owned(),
                hostname: "se-got-wg-001".to_owned(),
            }
        );
    }

    #[test]
    fn target_from_network_is_relay() {
        let network = MullvadNetwork {
            hostname: "se-got-wg-001".to_owned(),
            country_code: "se".to_owned(),
            city_code: "got".to_owned(),
            active: true,
        };
        assert_eq!(
            NetworkTarget::from(&network),
            NetworkTarget::relay("se", "got", "se-got-wg-001")
        );
    }
}
