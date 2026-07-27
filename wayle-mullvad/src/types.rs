//! Public, backend-independent types for the Mullvad service.
//!
//! These form the stable surface of the crate: they are what
//! [`Property`](wayle_core::Property) fields carry and what the control methods
//! accept, and they are deliberately independent of any daemon wire schema.

use serde::{Deserialize, Serialize};

/// High-level VPN connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConnectionState {
    /// Not connected; no tunnel is up.
    #[default]
    Disconnected,
    /// A tunnel is being established.
    Connecting,
    /// A tunnel is up and traffic is protected.
    Connected,
    /// The tunnel is being torn down.
    Disconnecting,
    /// The daemon is in a blocked or error state.
    Error,
}

/// The relay the daemon is currently connected (or connecting) to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedNetwork {
    /// Relay hostname, e.g. `"se-got-wg-001"`, when known.
    pub hostname: Option<String>,
    /// Human-readable country, e.g. `"Sweden"`.
    pub country: String,
    /// Human-readable city, e.g. `"Gothenburg"`, when known.
    pub city: Option<String>,
}

/// Combined tunnel status: the connection state plus the active relay, if any.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TunnelStatus {
    /// Current connection state.
    pub state: ConnectionState,
    /// The relay in use, when connecting or connected.
    pub network: Option<ConnectedNetwork>,
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

/// Selects what to connect to: a whole country, a city, or a specific network.
///
/// Build one with [`NetworkTarget::country`], [`NetworkTarget::city`] or
/// [`NetworkTarget::network`], or from a [`MullvadNetwork`] via [`From`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkTarget {
    /// Country code, e.g. `"se"` (always required).
    pub country: String,
    /// City code, e.g. `"got"`, to narrow the selection to a city.
    pub city: Option<String>,
    /// Relay hostname, e.g. `"se-got-wg-001"`, to pin an exact network.
    pub hostname: Option<String>,
}

impl NetworkTarget {
    /// Targets an entire country by its code (e.g. `"se"`).
    #[must_use]
    pub fn country(country: impl Into<String>) -> Self {
        Self {
            country: country.into(),
            city: None,
            hostname: None,
        }
    }

    /// Targets a city by its country and city codes (e.g. `"se"`, `"got"`).
    #[must_use]
    pub fn city(country: impl Into<String>, city: impl Into<String>) -> Self {
        Self {
            country: country.into(),
            city: Some(city.into()),
            hostname: None,
        }
    }

    /// Targets a specific network by country code, city code and hostname.
    #[must_use]
    pub fn network(
        country: impl Into<String>,
        city: impl Into<String>,
        hostname: impl Into<String>,
    ) -> Self {
        Self {
            country: country.into(),
            city: Some(city.into()),
            hostname: Some(hostname.into()),
        }
    }
}

impl From<&MullvadNetwork> for NetworkTarget {
    fn from(network: &MullvadNetwork) -> Self {
        Self {
            country: network.country_code.clone(),
            city: Some(network.city_code.clone()),
            hostname: Some(network.hostname.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MullvadNetwork, NetworkTarget};

    #[test]
    fn country_target_has_no_city_or_hostname() {
        let target = NetworkTarget::country("se");
        assert_eq!(target.country, "se");
        assert_eq!(target.city, None);
        assert_eq!(target.hostname, None);
    }

    #[test]
    fn city_target_narrows_to_city() {
        let target = NetworkTarget::city("se", "got");
        assert_eq!(target.country, "se");
        assert_eq!(target.city.as_deref(), Some("got"));
        assert_eq!(target.hostname, None);
    }

    #[test]
    fn network_target_pins_hostname() {
        let target = NetworkTarget::network("se", "got", "se-got-wg-001");
        assert_eq!(target.city.as_deref(), Some("got"));
        assert_eq!(target.hostname.as_deref(), Some("se-got-wg-001"));
    }

    #[test]
    fn target_from_network_uses_all_codes() {
        let network = MullvadNetwork {
            hostname: "se-got-wg-001".to_owned(),
            country_code: "se".to_owned(),
            city_code: "got".to_owned(),
            active: true,
        };
        let target = NetworkTarget::from(&network);
        assert_eq!(target, NetworkTarget::network("se", "got", "se-got-wg-001"));
    }
}
