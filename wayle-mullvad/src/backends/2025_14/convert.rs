//! Conversions from the 2025.14-line wire types into the crate's public types.
//!
//! Keeping these in one place makes the wire→public boundary explicit and easy
//! to audit when the schema changes.

use super::proto;
use crate::{
    backend::BackendEvent,
    types::{
        ConnectionStatus, ErrorCause, LoginState, MullvadNetwork, NetworkCity, NetworkCountry,
        NetworkTarget, RelayLocation, country_code_from_hostname,
    },
};

/// Converts a wire [`proto::TunnelState`] into a [`ConnectionStatus`] (the
/// tunnel dimension only; the login dimension is overlaid in the core model).
pub(super) fn connection_status(state: proto::TunnelState) -> ConnectionStatus {
    use proto::tunnel_state::State;

    match state.state {
        None | Some(State::Disconnected(_)) => ConnectionStatus::Disconnected,
        Some(State::Connecting(inner)) => {
            ConnectionStatus::Connecting(inner.relay_info.and_then(relay_location))
        }
        Some(State::Connected(inner)) => match inner.relay_info.and_then(relay_location) {
            Some(relay) => ConnectionStatus::Connected(relay),
            // The daemon always reports a relay when connected; if it somehow
            // doesn't, fall back to "connecting" rather than fabricate one.
            None => ConnectionStatus::Connecting(None),
        },
        Some(State::Disconnecting(_)) => ConnectionStatus::Disconnecting,
        Some(State::Error(inner)) => ConnectionStatus::Error(error_cause(inner.error_state)),
    }
}

/// Maps the daemon's blocked/error `ErrorState` cause to the display subset.
/// Unmapped or unknown causes fold into [`ErrorCause::Other`].
fn error_cause(error_state: Option<proto::ErrorState>) -> ErrorCause {
    use proto::error_state::Cause;

    let Some(error_state) = error_state else {
        return ErrorCause::Other;
    };
    match Cause::try_from(error_state.cause) {
        Ok(Cause::AuthFailed) => ErrorCause::AuthFailed,
        Ok(Cause::IsOffline) => ErrorCause::Offline,
        _ => ErrorCause::Other,
    }
}

/// Builds a display-ready [`RelayLocation`] from tunnel relay info, deriving the
/// ISO country code from the hostname as a fallback (the daemon reports only
/// names here; the core prefers the authoritative code from the relay tree).
fn relay_location(info: proto::TunnelStateRelayInfo) -> Option<RelayLocation> {
    let location = info.location?;
    let country_code = location
        .hostname
        .as_deref()
        .and_then(country_code_from_hostname)
        .unwrap_or_default();
    Some(RelayLocation {
        country_code,
        country: location.country,
        city: location.city,
        hostname: location.hostname,
    })
}

/// Maps the wire device state to a [`LoginState`].
///
/// Matches on the raw wire value so any unrecognized/future enum value is
/// treated as logged out (the prost `state()` accessor would instead map
/// unknown values to the `LoggedIn` default).
pub(super) fn login_state(device: &proto::DeviceState) -> LoginState {
    use proto::device_state::State;

    match State::try_from(device.state) {
        Ok(State::LoggedIn) => LoginState::LoggedIn,
        Ok(State::Revoked) => LoginState::Revoked,
        Ok(State::LoggedOut) | Err(_) => LoginState::LoggedOut,
    }
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
/// constraints intact (including a multihop entry pinned to a custom list, which
/// the proto models so it round-trips). A custom or absent current setting falls
/// back to a fresh normal setting with default constraints.
pub(super) fn relay_settings_with_location(
    current: Option<proto::RelaySettings>,
    target: &NetworkTarget,
) -> proto::RelaySettings {
    let (country, city, hostname) = match target {
        NetworkTarget::Country { code } => (code.clone(), None, None),
        NetworkTarget::City { country, code } => (country.clone(), Some(code.clone()), None),
        NetworkTarget::Relay {
            country,
            city,
            hostname,
        } => (country.clone(), Some(city.clone()), Some(hostname.clone())),
    };
    let geographic = proto::GeographicLocationConstraint {
        country,
        city,
        hostname,
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

/// Decodes the daemon's persisted relay settings into the selected
/// [`NetworkTarget`], or `None` when there is no single geographic selection
/// (a custom list, or an unconstrained "any" location).
pub(super) fn selected_target(settings: proto::RelaySettings) -> Option<NetworkTarget> {
    let proto::relay_settings::Endpoint::Normal(normal) = settings.endpoint?;
    match normal.location?.r#type? {
        proto::location_constraint::Type::Location(geo) => location_target(geo),
        // A custom list has no single geographic location to display.
        proto::location_constraint::Type::CustomList(_) => None,
    }
}

/// Maps a geographic constraint to the most specific selection it names
/// (relay > city > country), or `None` when it is unconstrained.
fn location_target(geo: proto::GeographicLocationConstraint) -> Option<NetworkTarget> {
    if geo.country.is_empty() {
        return None;
    }
    let city = geo.city.filter(|city| !city.is_empty());
    let hostname = geo.hostname.filter(|hostname| !hostname.is_empty());
    Some(match (city, hostname) {
        (Some(city), Some(hostname)) => NetworkTarget::relay(geo.country, city, hostname),
        (Some(city), None) => NetworkTarget::city(geo.country, city),
        // A hostname with no city can't name a relay; fall back to the country.
        (None, _) => NetworkTarget::country(geo.country),
    })
}

/// Translates a daemon event into a public [`BackendEvent`], or `None` for
/// event kinds the service does not track.
pub(super) fn backend_event(event: proto::DaemonEvent) -> Option<BackendEvent> {
    use proto::daemon_event::Event;

    match event.event? {
        Event::TunnelState(state) => Some(BackendEvent::Tunnel(connection_status(state))),
        Event::Settings(settings) => Some(BackendEvent::Selected(
            settings.relay_settings.and_then(selected_target),
        )),
        Event::RelayList(list) => Some(BackendEvent::Networks(networks(list))),
        Event::Device(device) => Some(BackendEvent::Login(login_state(&device.new_state?))),
    }
}

#[cfg(test)]
mod tests {
    use super::proto;
    use super::{
        backend_event, connection_status, login_state, networks, relay_settings_with_location,
        selected_target,
    };
    use crate::{
        backend::BackendEvent,
        types::{ConnectionStatus, ErrorCause, LoginState, NetworkTarget, RelayLocation},
    };

    #[test]
    fn connection_status_none_is_disconnected() {
        assert_eq!(
            connection_status(proto::TunnelState { state: None }),
            ConnectionStatus::Disconnected
        );
    }

    #[test]
    fn connection_status_connected_extracts_relay() {
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
        // Connected carries the relay, with the country code derived from the
        // hostname prefix (the core may later replace it with the tree's code).
        assert_eq!(
            connection_status(state),
            ConnectionStatus::Connected(RelayLocation {
                country_code: "se".to_owned(),
                country: "Sweden".to_owned(),
                city: Some("Gothenburg".to_owned()),
                hostname: Some("se-got-wg-001".to_owned()),
            })
        );
    }

    #[test]
    fn connection_status_error_maps_cause() {
        let state = proto::TunnelState {
            state: Some(proto::tunnel_state::State::Error(
                proto::tunnel_state::Error {
                    error_state: Some(proto::ErrorState {
                        cause: proto::error_state::Cause::AuthFailed as i32,
                    }),
                },
            )),
        };
        assert_eq!(
            connection_status(state),
            ConnectionStatus::Error(ErrorCause::AuthFailed)
        );

        // A missing/unmapped cause folds into Other.
        let blocked = proto::TunnelState {
            state: Some(proto::tunnel_state::State::Error(
                proto::tunnel_state::Error { error_state: None },
            )),
        };
        assert_eq!(
            connection_status(blocked),
            ConnectionStatus::Error(ErrorCause::Other)
        );
    }

    #[test]
    fn login_state_maps_device_state() {
        let device = |s: proto::device_state::State| proto::DeviceState { state: s as i32 };
        assert_eq!(
            login_state(&device(proto::device_state::State::LoggedIn)),
            LoginState::LoggedIn
        );
        assert_eq!(
            login_state(&device(proto::device_state::State::LoggedOut)),
            LoginState::LoggedOut
        );
        assert_eq!(
            login_state(&device(proto::device_state::State::Revoked)),
            LoginState::Revoked
        );
        // A future/unknown wire value is treated as logged out.
        assert_eq!(
            login_state(&proto::DeviceState { state: 99 }),
            LoginState::LoggedOut
        );
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

    fn geographic(settings: &proto::RelaySettings) -> Option<&proto::GeographicLocationConstraint> {
        match normal(settings).location.as_ref()?.r#type.as_ref()? {
            proto::location_constraint::Type::Location(geo) => Some(geo),
            proto::location_constraint::Type::CustomList(_) => None,
        }
    }

    #[test]
    fn relay_settings_fresh_when_no_current() {
        // No existing settings -> fresh normal setting with just the location
        // (and a present wireguard_constraints so the daemon accepts it).
        let settings = relay_settings_with_location(None, &NetworkTarget::city("se", "got"));
        assert!(normal(&settings).wireguard_constraints.is_some());
        let geo = geographic(&settings).expect("geographic location");
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
        assert_eq!(
            geographic(&settings).expect("geographic location").country,
            "se"
        );
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

    fn normal_settings(location: proto::LocationConstraint) -> proto::RelaySettings {
        proto::RelaySettings {
            endpoint: Some(proto::relay_settings::Endpoint::Normal(
                proto::NormalRelaySettings {
                    location: Some(location),
                    ..Default::default()
                },
            )),
        }
    }

    fn geo_location(geo: proto::GeographicLocationConstraint) -> proto::LocationConstraint {
        proto::LocationConstraint {
            r#type: Some(proto::location_constraint::Type::Location(geo)),
        }
    }

    #[test]
    fn selected_target_decodes_geographic_levels() {
        assert_eq!(
            selected_target(normal_settings(geo_location(
                proto::GeographicLocationConstraint {
                    country: "se".to_owned(),
                    city: None,
                    hostname: None,
                }
            ))),
            Some(NetworkTarget::country("se"))
        );
        assert_eq!(
            selected_target(normal_settings(geo_location(
                proto::GeographicLocationConstraint {
                    country: "se".to_owned(),
                    city: Some("got".to_owned()),
                    hostname: None,
                }
            ))),
            Some(NetworkTarget::city("se", "got"))
        );
        assert_eq!(
            selected_target(normal_settings(geo_location(
                proto::GeographicLocationConstraint {
                    country: "se".to_owned(),
                    city: Some("got".to_owned()),
                    hostname: Some("se-got-wg-001".to_owned()),
                }
            ))),
            Some(NetworkTarget::relay("se", "got", "se-got-wg-001"))
        );
    }

    #[test]
    fn selected_target_none_for_custom_list_and_unconstrained() {
        // A custom list has no single geographic location.
        let custom = normal_settings(proto::LocationConstraint {
            r#type: Some(proto::location_constraint::Type::CustomList(
                "list".to_owned(),
            )),
        });
        assert_eq!(selected_target(custom), None);

        // No location at all -> nothing selected.
        let empty = proto::RelaySettings {
            endpoint: Some(proto::relay_settings::Endpoint::Normal(
                proto::NormalRelaySettings::default(),
            )),
        };
        assert_eq!(selected_target(empty), None);
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
            Some(BackendEvent::Tunnel(ConnectionStatus::Disconnected))
        );

        let device = proto::DaemonEvent {
            event: Some(proto::daemon_event::Event::Device(proto::DeviceEvent {
                new_state: Some(proto::DeviceState {
                    state: proto::device_state::State::LoggedIn as i32,
                }),
            })),
        };
        assert_eq!(
            backend_event(device),
            Some(BackendEvent::Login(LoginState::LoggedIn))
        );

        let settings = proto::DaemonEvent {
            event: Some(proto::daemon_event::Event::Settings(proto::Settings {
                relay_settings: Some(normal_settings(geo_location(
                    proto::GeographicLocationConstraint {
                        country: "se".to_owned(),
                        city: None,
                        hostname: None,
                    },
                ))),
            })),
        };
        assert_eq!(
            backend_event(settings),
            Some(BackendEvent::Selected(Some(NetworkTarget::country("se"))))
        );

        assert!(backend_event(proto::DaemonEvent { event: None }).is_none());
    }
}
