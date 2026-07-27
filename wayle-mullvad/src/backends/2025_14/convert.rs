//! Conversions from the 2025.14-line wire types into the crate's public types.
//!
//! Keeping these in one place makes the wire→public boundary explicit and easy
//! to audit when the schema changes.

use super::proto;
use crate::{
    backend::BackendEvent,
    types::{
        ConnectedNetwork, ConnectionState, MullvadNetwork, NetworkCity, NetworkCountry,
        NetworkTarget, TunnelStatus,
    },
};

/// Converts a wire [`proto::TunnelState`] into a [`TunnelStatus`].
pub(super) fn tunnel_status(state: proto::TunnelState) -> TunnelStatus {
    use proto::tunnel_state::State;

    match state.state {
        None | Some(State::Disconnected(_)) => TunnelStatus {
            state: ConnectionState::Disconnected,
            network: None,
        },
        Some(State::Connecting(inner)) => TunnelStatus {
            state: ConnectionState::Connecting,
            network: inner.relay_info.and_then(connected_network),
        },
        Some(State::Connected(inner)) => TunnelStatus {
            state: ConnectionState::Connected,
            network: inner.relay_info.and_then(connected_network),
        },
        Some(State::Disconnecting(_)) => TunnelStatus {
            state: ConnectionState::Disconnecting,
            network: None,
        },
        Some(State::Error(_)) => TunnelStatus {
            state: ConnectionState::Error,
            network: None,
        },
    }
}

/// Extracts the connected relay from tunnel relay info, if a location is present.
fn connected_network(info: proto::TunnelStateRelayInfo) -> Option<ConnectedNetwork> {
    info.location.map(|location| ConnectedNetwork {
        hostname: location.hostname,
        country: location.country,
        city: location.city,
    })
}

/// Returns whether the device state indicates a logged-in account.
///
/// Matches on the raw wire value so any unrecognized/future enum value is
/// treated as NOT logged in (the prost `state()` accessor would instead map
/// unknown values to the `LoggedIn` default).
pub(super) fn is_logged_in(device: &proto::DeviceState) -> bool {
    matches!(
        proto::device_state::State::try_from(device.state),
        Ok(proto::device_state::State::LoggedIn)
    )
}

/// Converts a wire [`proto::RelayList`] into the public country→city→network tree.
pub(super) fn networks(list: proto::RelayList) -> Vec<NetworkCountry> {
    list.countries
        .into_iter()
        .map(|country| {
            let code = country.code;
            let cities = country
                .cities
                .into_iter()
                .map(|city| network_city(&code, city))
                .collect();
            NetworkCountry {
                name: country.name,
                code,
                cities,
            }
        })
        .collect()
}

/// Converts one wire city, tagging each relay with its country/city codes.
fn network_city(country_code: &str, city: proto::RelayListCity) -> NetworkCity {
    let code = city.code;
    let networks = city
        .relays
        .into_iter()
        .map(|relay| MullvadNetwork {
            hostname: relay.hostname,
            country_code: country_code.to_owned(),
            city_code: code.clone(),
            active: relay.active,
        })
        .collect();
    NetworkCity {
        name: city.name,
        code,
        networks,
    }
}

/// Builds relay settings selecting `target`, preserving the daemon's `current`
/// settings and changing only the exit location.
///
/// `SetRelaySettings` is a full replace on the daemon, so we start from the
/// existing `NormalRelaySettings` (fetched via `GetSettings`) and mutate only
/// its `location` — keeping the user's provider/ownership/multihop/entry/IP
/// constraints intact. A custom or absent current setting falls back to a fresh
/// normal setting with default constraints.
pub(super) fn relay_settings_with_location(
    current: Option<proto::RelaySettings>,
    target: &NetworkTarget,
) -> proto::RelaySettings {
    let geographic = proto::GeographicLocationConstraint {
        country: target.country.clone(),
        city: target.city.clone(),
        hostname: target.hostname.clone(),
    };
    let location = proto::LocationConstraint {
        r#type: Some(proto::location_constraint::Type::Location(geographic)),
    };

    let mut normal = match current.and_then(|settings| settings.endpoint) {
        Some(proto::relay_settings::Endpoint::Normal(normal)) => normal,
        None => proto::NormalRelaySettings::default(),
    };
    normal.location = Some(location);
    // The daemon rejects the request if wireguard_constraints is absent.
    if normal.wireguard_constraints.is_none() {
        normal.wireguard_constraints = Some(proto::WireguardConstraints::default());
    }

    proto::RelaySettings {
        endpoint: Some(proto::relay_settings::Endpoint::Normal(normal)),
    }
}

/// Translates a daemon event into a public [`BackendEvent`], or `None` for
/// event kinds the service does not track.
pub(super) fn backend_event(event: proto::DaemonEvent) -> Option<BackendEvent> {
    use proto::daemon_event::Event;

    match event.event? {
        Event::TunnelState(state) => Some(BackendEvent::Tunnel(tunnel_status(state))),
        Event::RelayList(list) => Some(BackendEvent::Networks(networks(list))),
        Event::Device(device) => Some(BackendEvent::LoggedIn(is_logged_in(&device.new_state?))),
    }
}

#[cfg(test)]
mod tests {
    use super::proto;
    use super::{
        backend_event, is_logged_in, networks, relay_settings_with_location, tunnel_status,
    };
    use crate::{
        backend::BackendEvent,
        types::{ConnectionState, NetworkTarget, TunnelStatus},
    };

    #[test]
    fn tunnel_status_none_is_disconnected() {
        let status = tunnel_status(proto::TunnelState { state: None });
        assert_eq!(status.state, ConnectionState::Disconnected);
        assert!(status.network.is_none());
    }

    #[test]
    fn tunnel_status_connected_extracts_relay() {
        let state = proto::TunnelState {
            state: Some(proto::tunnel_state::State::Connected(
                proto::tunnel_state::Connected {
                    relay_info: Some(proto::TunnelStateRelayInfo {
                        location: Some(proto::GeoIpLocation {
                            country: "Sweden".to_owned(),
                            city: Some("Gothenburg".to_owned()),
                            hostname: Some("se-got-wg-001".to_owned()),
                        }),
                    }),
                },
            )),
        };
        let status = tunnel_status(state);
        assert_eq!(status.state, ConnectionState::Connected);
        let network = status.network.expect("expected a connected relay");
        assert_eq!(network.hostname.as_deref(), Some("se-got-wg-001"));
        assert_eq!(network.country, "Sweden");
        assert_eq!(network.city.as_deref(), Some("Gothenburg"));
    }

    #[test]
    fn tunnel_status_error_variant_maps_to_error() {
        let state = proto::TunnelState {
            state: Some(proto::tunnel_state::State::Error(
                proto::tunnel_state::Error {},
            )),
        };
        assert_eq!(tunnel_status(state).state, ConnectionState::Error);
    }

    #[test]
    fn login_state_maps_to_bool() {
        let logged_in = proto::DeviceState {
            state: proto::device_state::State::LoggedIn as i32,
        };
        let logged_out = proto::DeviceState {
            state: proto::device_state::State::LoggedOut as i32,
        };
        let revoked = proto::DeviceState {
            state: proto::device_state::State::Revoked as i32,
        };
        let unknown = proto::DeviceState { state: 99 };
        assert!(is_logged_in(&logged_in));
        assert!(!is_logged_in(&logged_out));
        assert!(!is_logged_in(&revoked));
        // A future/unknown wire value must not be treated as logged in.
        assert!(!is_logged_in(&unknown));
    }

    #[test]
    fn networks_builds_tree_and_propagates_codes() {
        let list = proto::RelayList {
            countries: vec![proto::RelayListCountry {
                name: "Sweden".to_owned(),
                code: "se".to_owned(),
                cities: vec![proto::RelayListCity {
                    name: "Gothenburg".to_owned(),
                    code: "got".to_owned(),
                    relays: vec![proto::Relay {
                        hostname: "se-got-wg-001".to_owned(),
                        active: true,
                        location: None,
                    }],
                }],
            }],
        };
        let tree = networks(list);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].code, "se");
        let city = &tree[0].cities[0];
        assert_eq!(city.code, "got");
        let network = &city.networks[0];
        assert_eq!(network.hostname, "se-got-wg-001");
        assert_eq!(network.country_code, "se");
        assert_eq!(network.city_code, "got");
        assert!(network.active);
    }

    fn normal(settings: &proto::RelaySettings) -> &proto::NormalRelaySettings {
        match settings.endpoint.as_ref().expect("endpoint set") {
            proto::relay_settings::Endpoint::Normal(normal) => normal,
        }
    }

    fn geographic(settings: &proto::RelaySettings) -> &proto::GeographicLocationConstraint {
        match normal(settings)
            .location
            .as_ref()
            .expect("location set")
            .r#type
            .as_ref()
            .expect("type set")
        {
            proto::location_constraint::Type::Location(geo) => geo,
        }
    }

    #[test]
    fn relay_settings_fresh_when_no_current() {
        // No existing settings -> fresh normal setting with just the location
        // (and a present wireguard_constraints so the daemon accepts it).
        let settings = relay_settings_with_location(None, &NetworkTarget::city("se", "got"));
        assert!(normal(&settings).wireguard_constraints.is_some());
        let geo = geographic(&settings);
        assert_eq!(geo.country, "se");
        assert_eq!(geo.city.as_deref(), Some("got"));
        assert_eq!(geo.hostname, None);
    }

    #[test]
    fn relay_settings_preserves_existing_constraints() {
        // The user's providers/ownership/multihop constraints must survive a
        // connect that only changes the location.
        let current = proto::RelaySettings {
            endpoint: Some(proto::relay_settings::Endpoint::Normal(
                proto::NormalRelaySettings {
                    location: Some(proto::LocationConstraint {
                        r#type: Some(proto::location_constraint::Type::Location(
                            proto::GeographicLocationConstraint {
                                country: "us".to_owned(),
                                city: None,
                                hostname: None,
                            },
                        )),
                    }),
                    providers: vec!["mullvad".to_owned()],
                    ownership: proto::Ownership::MullvadOwned as i32,
                    wireguard_constraints: Some(proto::WireguardConstraints {
                        use_multihop: true,
                        ..Default::default()
                    }),
                },
            )),
        };

        let settings = relay_settings_with_location(Some(current), &NetworkTarget::country("se"));
        let normal = normal(&settings);
        // Location changed to the new target...
        assert_eq!(geographic(&settings).country, "se");
        // ...but every other constraint is preserved.
        assert_eq!(normal.providers, vec!["mullvad".to_owned()]);
        assert_eq!(normal.ownership, proto::Ownership::MullvadOwned as i32);
        assert_eq!(
            normal
                .wireguard_constraints
                .as_ref()
                .map(|w| w.use_multihop),
            Some(true)
        );
    }

    #[test]
    fn backend_event_translates_tracked_variants() {
        let tunnel = proto::DaemonEvent {
            event: Some(proto::daemon_event::Event::TunnelState(
                proto::TunnelState {
                    state: Some(proto::tunnel_state::State::Disconnected(
                        proto::tunnel_state::Disconnected {},
                    )),
                },
            )),
        };
        assert_eq!(
            backend_event(tunnel),
            Some(BackendEvent::Tunnel(TunnelStatus {
                state: ConnectionState::Disconnected,
                network: None,
            }))
        );

        let device = proto::DaemonEvent {
            event: Some(proto::daemon_event::Event::Device(proto::DeviceEvent {
                new_state: Some(proto::DeviceState {
                    state: proto::device_state::State::LoggedIn as i32,
                }),
            })),
        };
        assert_eq!(backend_event(device), Some(BackendEvent::LoggedIn(true)));

        assert!(backend_event(proto::DaemonEvent { event: None }).is_none());
    }
}
