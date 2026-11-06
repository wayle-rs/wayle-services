//! IWD's devices and networks, and the station state derived from them.
//!
//! The registry holds what IWD exports (from `GetManagedObjects`,
//! `InterfacesAdded`/`InterfacesRemoved` and `PropertiesChanged`) and publishes
//! one live [`Station`], for the first device, and one live [`Network`] per
//! network object. Everything it publishes is IWD's state.

use std::{collections::BTreeMap, sync::Arc};

use wayle_core::Property;
use zbus::{
    Message,
    fdo::ManagedObjects,
    names::OwnedInterfaceName,
    zvariant::{ObjectPath, OwnedObjectPath},
};

use super::command::Commands;
use crate::{
    network::Network,
    props::{self, PropertyMap},
    station::Station,
    types::{ConnectionState, PassphraseRequest, SecurityType},
};

pub(crate) const ADAPTER_INTERFACE: &str = "net.connman.iwd.Adapter";
pub(crate) const DEVICE_INTERFACE: &str = "net.connman.iwd.Device";
pub(crate) const STATION_INTERFACE: &str = "net.connman.iwd.Station";
pub(crate) const NETWORK_INTERFACE: &str = "net.connman.iwd.Network";

const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const OBJECT_MANAGER_INTERFACE: &str = "org.freedesktop.DBus.ObjectManager";

/// The interfaces of one object, as `GetManagedObjects` and `InterfacesAdded`
/// deliver them.
type Interfaces = std::collections::HashMap<OwnedInterfaceName, PropertyMap>;

/// An object path as a map key (`OwnedObjectPath` isn't ordered).
type Path = ObjectPath<'static>;

/// The service-level properties.
#[derive(Clone)]
pub(crate) struct Published {
    pub station: Property<Option<Arc<Station>>>,
    pub passphrase_request: Property<Option<PassphraseRequest>>,
}

impl Published {
    pub(crate) fn new() -> Self {
        Self {
            station: Property::new(None),
            passphrase_request: Property::new(None),
        }
    }
}

/// A device's `Station` interface, present while the device is powered on.
#[derive(Debug, Default)]
struct StationProps {
    state: String,
    connected_network: Option<OwnedObjectPath>,
    scanning: bool,
}

#[derive(Debug, Default)]
struct DeviceEntry {
    /// `Device.Powered`: the interface is up.
    powered: bool,
    /// The adapter the device belongs to.
    adapter: Option<OwnedObjectPath>,
    station: Option<StationProps>,
}

struct NetworkEntry {
    network: Arc<Network>,
    known_network: Option<OwnedObjectPath>,
}

/// What a signal changed that the dispatcher reacts to.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    /// The station's `Station` interface appeared: its signal-level agent
    /// needs registering.
    pub station_up: bool,
    /// The station's networks or their order may have changed.
    pub networks: bool,
    /// The station's connection became active: its diagnostics need reading.
    pub link_up: bool,
    /// Networks that went away.
    pub removed_networks: Vec<OwnedObjectPath>,
}

pub(crate) struct Registry {
    commands: Commands,
    /// `Adapter.Powered` by adapter: whether rfkill leaves its radio on.
    adapters: BTreeMap<Path, bool>,
    devices: BTreeMap<Path, DeviceEntry>,
    networks: BTreeMap<Path, NetworkEntry>,
    /// A network's name, security or saved credentials changed since the
    /// station's networks were last published.
    details_changed: bool,
    /// The published station, for the first device.
    station: Option<Arc<Station>>,
    published: Published,
}

impl Registry {
    pub(crate) fn new(commands: &Commands, published: Published) -> Self {
        Self {
            commands: commands.clone(),
            adapters: BTreeMap::new(),
            devices: BTreeMap::new(),
            networks: BTreeMap::new(),
            details_changed: false,
            station: None,
            published,
        }
    }

    /// The published station.
    pub(crate) fn station(&self) -> Option<&Arc<Station>> {
        self.station.as_ref()
    }

    /// Whether `station` is the published instance.
    pub(crate) fn is_live(&self, station: &Arc<Station>) -> bool {
        self.station
            .as_ref()
            .is_some_and(|live| Arc::ptr_eq(live, station))
    }

    /// The live network at `network`'s path, if it is that instance.
    pub(crate) fn live_network(&self, network: &Arc<Network>) -> bool {
        self.networks
            .get(&**network.object_path())
            .is_some_and(|entry| Arc::ptr_eq(&entry.network, network))
    }

    /// The live network at `path`.
    pub(crate) fn network(&self, path: &OwnedObjectPath) -> Option<&Arc<Network>> {
        self.networks.get(&**path).map(|entry| &entry.network)
    }

    /// The `KnownNetwork` holding `network`'s saved credentials, if any.
    pub(crate) fn known_network(&self, network: &Arc<Network>) -> Option<&OwnedObjectPath> {
        self.networks
            .get(&**network.object_path())
            .and_then(|entry| entry.known_network.as_ref())
    }

    /// Every adapter, with whether its radio is powered.
    pub(crate) fn adapters(&self) -> impl Iterator<Item = (&ObjectPath<'static>, bool)> {
        self.adapters.iter().map(|(path, powered)| (path, *powered))
    }

    /// Whether the station's device is up, and whether its adapter is powered
    /// (true if IWD didn't name one).
    pub(crate) fn station_power(&self) -> Option<(bool, bool)> {
        let entry = self.station_entry()?;
        Some((entry.powered, self.adapter_powered(entry)))
    }

    fn adapter_powered(&self, entry: &DeviceEntry) -> bool {
        entry
            .adapter
            .as_ref()
            .and_then(|adapter| self.adapters.get(&**adapter))
            .copied()
            .unwrap_or(true)
    }

    /// Replaces everything with a fresh enumeration, then applies the signals
    /// received while enumerating.
    pub(crate) fn load(&mut self, objects: ManagedObjects, buffered: &[Message]) -> Changes {
        self.adapters.clear();
        self.devices.clear();
        self.networks.clear();

        let mut changes = Changes::default();
        for (path, interfaces) in objects {
            self.add_interfaces(&path, interfaces, &mut changes);
        }
        for message in buffered {
            self.apply(message, &mut changes);
        }

        // Everything about the station is new.
        changes.networks = true;
        changes.station_up = self
            .station_entry()
            .is_some_and(|entry| entry.station.is_some());
        self.publish(&mut changes);
        changes
    }

    /// Forgets everything: IWD went away.
    pub(crate) fn clear(&mut self) {
        self.adapters.clear();
        self.devices.clear();
        self.networks.clear();
        self.publish(&mut Changes::default());
    }

    /// Applies one signal and republishes whatever it changed.
    pub(crate) fn handle(&mut self, message: &Message) -> Changes {
        let mut changes = Changes::default();
        self.apply(message, &mut changes);
        self.publish(&mut changes);
        changes
    }

    /// Publishes IWD's order of the station's networks, with their signal
    /// (in 100 × dBm).
    ///
    /// The station's networks notify watchers when the list changes, and also
    /// when a listed network's signal, name, security or saved credentials
    /// changed, so a list watcher sees those without watching every network.
    pub(crate) fn set_ordered(
        &mut self,
        station: &Arc<Station>,
        ordered: Vec<(OwnedObjectPath, i16)>,
    ) {
        if !self.is_live(station) {
            return;
        }
        let mut changed = std::mem::take(&mut self.details_changed);
        let networks: Vec<Arc<Network>> = ordered
            .into_iter()
            .filter_map(|(path, signal)| {
                let network = &self.networks.get(&*path)?.network;
                // IWD reports 100 × dBm. Floored, so -74.5 dBm is the weaker -75.
                let dbm = signal.div_euclid(100);
                changed |= network.signal.get() != dbm;
                network.signal.set(dbm);
                Some(Arc::clone(network))
            })
            .collect();
        changed |= station.networks.get() != networks;
        if changed {
            station.networks.replace(networks);
        }
    }

    fn apply(&mut self, message: &Message, changes: &mut Changes) {
        let header = message.header();
        let (Some(interface), Some(member), Some(path)) =
            (header.interface(), header.member(), header.path())
        else {
            return;
        };

        match (interface.as_str(), member.as_str()) {
            (PROPERTIES_INTERFACE, "PropertiesChanged") => {
                let Ok((interface, changed, invalidated)) =
                    message
                        .body()
                        .deserialize::<(String, PropertyMap, Vec<String>)>()
                else {
                    return;
                };
                self.apply_properties(&path.to_owned(), &interface, changed, &invalidated, changes);
            }
            (OBJECT_MANAGER_INTERFACE, "InterfacesAdded") => {
                if let Ok((path, interfaces)) = message
                    .body()
                    .deserialize::<(OwnedObjectPath, Interfaces)>()
                {
                    self.add_interfaces(&path, interfaces, changes);
                }
            }
            (OBJECT_MANAGER_INTERFACE, "InterfacesRemoved") => {
                if let Ok((path, interfaces)) = message
                    .body()
                    .deserialize::<(OwnedObjectPath, Vec<String>)>()
                {
                    self.remove_interfaces(&path, &interfaces, changes);
                }
            }
            _ => {}
        }
    }

    fn add_interfaces(
        &mut self,
        path: &OwnedObjectPath,
        interfaces: Interfaces,
        changes: &mut Changes,
    ) {
        let key: &Path = path;
        for (interface, properties) in interfaces {
            match interface.as_str() {
                ADAPTER_INTERFACE => {
                    self.adapters.entry(key.clone()).or_default();
                }
                DEVICE_INTERFACE => {
                    self.devices.entry(key.clone()).or_default();
                }
                STATION_INTERFACE => {
                    self.devices.entry(key.clone()).or_default().station =
                        Some(StationProps::default());
                    if self.is_station_path(key) {
                        changes.station_up = true;
                        changes.networks = true;
                    }
                }
                NETWORK_INTERFACE => {
                    let commands = &self.commands;
                    self.networks
                        .entry(key.clone())
                        .or_insert_with(|| NetworkEntry {
                            network: Arc::new(Network::new(commands, path.clone())),
                            known_network: None,
                        });
                    changes.networks |= !self.scanning();
                }
                _ => continue,
            }
            self.apply_properties(key, interface.as_str(), properties, &[], changes);
        }
    }

    fn remove_interfaces(
        &mut self,
        path: &OwnedObjectPath,
        interfaces: &[String],
        changes: &mut Changes,
    ) {
        let path: &Path = path;
        for interface in interfaces {
            match interface.as_str() {
                ADAPTER_INTERFACE => {
                    self.adapters.remove(path);
                }
                DEVICE_INTERFACE => {
                    self.devices.remove(path);
                }
                STATION_INTERFACE => {
                    if let Some(device) = self.devices.get_mut(path) {
                        device.station = None;
                    }
                }
                NETWORK_INTERFACE => {
                    if self.networks.remove(path).is_some() {
                        changes.removed_networks.push(path.clone().into());
                        changes.networks |= !self.scanning();
                    }
                }
                _ => {}
            }
        }
    }

    fn apply_properties(
        &mut self,
        path: &Path,
        interface: &str,
        changed: PropertyMap,
        invalidated: &[String],
        changes: &mut Changes,
    ) {
        match interface {
            ADAPTER_INTERFACE => {
                let Some(powered) = self.adapters.get_mut(path) else {
                    return;
                };
                if let Some(value) = changed.get("Powered").cloned() {
                    props::assign(powered, "Powered", value);
                }
            }
            DEVICE_INTERFACE => {
                let Some(device) = self.devices.get_mut(path) else {
                    return;
                };
                for (name, value) in changed {
                    match name.as_str() {
                        "Powered" => props::assign(&mut device.powered, &name, value),
                        "Adapter" => props::assign_some(&mut device.adapter, &name, value),
                        _ => {}
                    }
                }
            }
            STATION_INTERFACE => {
                let is_station = self.is_station_path(path);
                let Some(station) = self
                    .devices
                    .get_mut(path)
                    .and_then(|device| device.station.as_mut())
                else {
                    return;
                };
                for (name, value) in changed {
                    match name.as_str() {
                        "State" => {
                            props::assign(&mut station.state, &name, value);
                            // The connected network moves to the top.
                            changes.networks |= is_station;
                        }
                        "ConnectedNetwork" => {
                            props::assign_some(&mut station.connected_network, &name, value);
                        }
                        "Scanning" => {
                            let was = station.scanning;
                            props::assign(&mut station.scanning, &name, value);
                            // A finished scan means fresh results.
                            changes.networks |= is_station && was && !station.scanning;
                        }
                        _ => {}
                    }
                }
                if invalidated.iter().any(|name| name == "ConnectedNetwork") {
                    station.connected_network = None;
                }
            }
            NETWORK_INTERFACE => {
                let Some(entry) = self.networks.get_mut(path) else {
                    return;
                };
                for (name, value) in changed {
                    match name.as_str() {
                        "Name" => {
                            let mut ssid = String::new();
                            props::assign(&mut ssid, &name, value);
                            self.details_changed |= entry.network.ssid.get() != ssid;
                            entry.network.ssid.set(ssid);
                        }
                        "Type" => {
                            let mut kind = String::new();
                            props::assign(&mut kind, &name, value);
                            let security = SecurityType::from_iwd_type(&kind);
                            self.details_changed |= entry.network.security.get() != security;
                            entry.network.security.set(security);
                        }
                        "KnownNetwork" => {
                            props::assign_some(&mut entry.known_network, &name, value);
                            self.details_changed |= !entry.network.known.get();
                            entry.network.known.set(entry.known_network.is_some());
                            changes.networks = true;
                        }
                        _ => {}
                    }
                }
                if invalidated.iter().any(|name| name == "KnownNetwork") {
                    entry.known_network = None;
                    self.details_changed |= entry.network.known.get();
                    entry.network.known.set(false);
                    changes.networks = true;
                }
            }
            _ => {}
        }
    }

    /// The device the station is published for: the current one while it
    /// exists, else the first.
    fn station_path(&self) -> Option<&Path> {
        self.station
            .as_ref()
            .map(|station| &**station.object_path())
            .filter(|path| self.devices.contains_key(*path))
            .or_else(|| self.devices.keys().next())
    }

    fn is_station_path(&self, path: &Path) -> bool {
        self.station_path() == Some(path)
    }

    fn station_entry(&self) -> Option<&DeviceEntry> {
        self.devices.get(self.station_path()?)
    }

    fn scanning(&self) -> bool {
        self.station_entry()
            .and_then(|entry| entry.station.as_ref())
            .is_some_and(|station| station.scanning)
    }

    /// Republishes the station from the device it is for.
    fn publish(&mut self, changes: &mut Changes) {
        let path = self.station_path().cloned();
        let replaced = match (&self.station, &path) {
            (Some(station), Some(path)) => &**station.object_path() != path,
            (None, None) => false,
            _ => true,
        };
        if replaced {
            self.station = path.map(|path| Arc::new(Station::new(&self.commands, path.into())));
            self.published.station.set(self.station.clone());
            changes.networks |= self.station.is_some();
            // A device taking over may already have its `Station` interface.
            changes.station_up |= self
                .station_entry()
                .is_some_and(|entry| entry.station.is_some());
        }

        let (Some(station), Some(entry)) = (&self.station, self.station_entry()) else {
            return;
        };

        let powered = entry.powered && self.adapter_powered(entry);
        // IWD may add the `Station` interface before the device reports being
        // up, when the networks couldn't be read yet.
        changes.networks |= powered && !station.powered.get();
        station.powered.set(powered);
        let Some(props) = &entry.station else {
            // Powered off: IWD removed the `Station` interface.
            station.scanning.set(false);
            station.connection.set(ConnectionState::Idle);
            station.strength.set(None);
            station.frequency.set(None);
            station.networks.set(Vec::new());
            return;
        };

        station.scanning.set(props.scanning);

        let network = props
            .connected_network
            .as_ref()
            .and_then(|path| self.networks.get(&**path))
            .map(|entry| Arc::clone(&entry.network));
        let connection = ConnectionState::from_raw_state(&props.state, network);
        let previous = station.connection.get();
        if connection == previous {
            return;
        }

        let active = |state: &ConnectionState| {
            matches!(
                state,
                ConnectionState::Connected { .. } | ConnectionState::Roaming { .. }
            )
        };
        if active(&connection) && !active(&previous) {
            changes.link_up = true;
        }
        if !active(&connection) {
            station.strength.set(None);
            station.frequency.set(None);
        }
        // A connection starting supersedes an earlier failure.
        if previous == ConnectionState::Idle {
            station.last_error.set(None);
        }
        station.connection.set(connection);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use futures::{FutureExt, StreamExt};
    use zbus::zvariant::{OwnedValue, Value};

    use super::*;

    const DEVICE: &str = "/net/connman/iwd/0/4";
    const HOME: &str = "/net/connman/iwd/0/4/686f6d65_psk";
    const CAFE: &str = "/net/connman/iwd/0/4/63616665_open";

    fn path(at: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(at).unwrap()
    }

    fn props(entries: &[(&str, Value<'_>)]) -> PropertyMap {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), OwnedValue::try_from(value).unwrap()))
            .collect()
    }

    fn interfaces(entries: Vec<(&str, PropertyMap)>) -> HashMap<OwnedInterfaceName, PropertyMap> {
        entries
            .into_iter()
            .map(|(name, props)| (OwnedInterfaceName::try_from(name).unwrap(), props))
            .collect()
    }

    fn device(
        powered: bool,
        state: &str,
    ) -> (OwnedObjectPath, HashMap<OwnedInterfaceName, PropertyMap>) {
        let mut entries = vec![(
            DEVICE_INTERFACE,
            props(&[("Powered", Value::from(powered))]),
        )];
        if powered {
            entries.push((
                STATION_INTERFACE,
                props(&[
                    ("State", Value::from(state)),
                    ("Scanning", Value::from(false)),
                ]),
            ));
        }
        (path(DEVICE), interfaces(entries))
    }

    fn network(
        at: &str,
        name: &str,
        kind: &str,
    ) -> (OwnedObjectPath, HashMap<OwnedInterfaceName, PropertyMap>) {
        (
            path(at),
            interfaces(vec![(
                NETWORK_INTERFACE,
                props(&[
                    ("Name", Value::from(name)),
                    ("Type", Value::from(kind)),
                    ("Device", Value::from(ObjectPathValue(DEVICE))),
                ]),
            )]),
        )
    }

    /// An object path as a property value.
    struct ObjectPathValue(&'static str);

    impl From<ObjectPathValue> for Value<'static> {
        fn from(path: ObjectPathValue) -> Self {
            Value::from(zbus::zvariant::ObjectPath::from_static_str_unchecked(
                path.0,
            ))
        }
    }

    fn loaded(
        objects: Vec<(OwnedObjectPath, HashMap<OwnedInterfaceName, PropertyMap>)>,
    ) -> (Registry, Published) {
        let (commands, _) = Commands::channel();
        let published = Published::new();
        let mut registry = Registry::new(&commands, published.clone());
        let changes = registry.load(objects.into_iter().collect(), &[]);
        assert!(changes.networks);
        (registry, published)
    }

    fn properties_changed(
        at: &str,
        interface: &str,
        changed: &[(&str, Value<'_>)],
        invalidated: &[&str],
    ) -> Message {
        let invalidated: Vec<String> = invalidated.iter().map(|name| (*name).to_owned()).collect();
        Message::signal(at, PROPERTIES_INTERFACE, "PropertiesChanged")
            .unwrap()
            .build(&(interface, props(changed), invalidated))
            .unwrap()
    }

    #[test]
    fn a_powered_device_is_published_as_the_station() {
        let (_registry, published) = loaded(vec![
            device(true, "disconnected"),
            network(HOME, "home", "psk"),
        ]);

        let station = published.station.get().unwrap();
        assert!(station.powered.get());
        assert_eq!(station.connection.get(), ConnectionState::Idle);
    }

    #[test]
    fn the_connection_follows_iwds_state_and_network() {
        let (mut registry, published) = loaded(vec![
            device(true, "disconnected"),
            network(HOME, "home", "psk"),
        ]);
        let station = published.station.get().unwrap();
        let home = Arc::clone(registry.network(&path(HOME)).unwrap());

        let changes = registry.handle(&properties_changed(
            DEVICE,
            STATION_INTERFACE,
            &[
                ("State", Value::from("connecting")),
                ("ConnectedNetwork", Value::from(ObjectPathValue(HOME))),
            ],
            &[],
        ));
        assert_eq!(
            station.connection.get(),
            ConnectionState::Connecting {
                network: Arc::clone(&home)
            }
        );
        assert!(!changes.link_up);

        let changes = registry.handle(&properties_changed(
            DEVICE,
            STATION_INTERFACE,
            &[("State", Value::from("connected"))],
            &[],
        ));
        assert_eq!(
            station.connection.get(),
            ConnectionState::Connected { network: home }
        );
        assert!(changes.link_up, "diagnostics are read once the link is up");

        registry.handle(&properties_changed(
            DEVICE,
            STATION_INTERFACE,
            &[("State", Value::from("disconnected"))],
            &["ConnectedNetwork"],
        ));
        assert_eq!(station.connection.get(), ConnectionState::Idle);
    }

    #[test]
    fn networks_are_published_in_iwds_order() {
        let (mut registry, published) = loaded(vec![
            device(true, "disconnected"),
            network(HOME, "home", "psk"),
            network(CAFE, "cafe", "open"),
        ]);
        let station = published.station.get().unwrap();

        registry.set_ordered(&station, vec![(path(CAFE), -5_500), (path(HOME), -8_000)]);

        let networks = station.networks.get();
        let names: Vec<String> = networks.iter().map(|network| network.ssid.get()).collect();
        assert_eq!(names, ["cafe", "home"]);
        assert_eq!(networks[0].signal.get(), -55);
        assert_eq!(networks[1].security.get(), SecurityType::Psk);
    }

    #[test]
    fn a_network_is_known_while_iwd_reports_its_known_network() {
        let (mut registry, published) = loaded(vec![
            device(true, "disconnected"),
            network(HOME, "home", "psk"),
        ]);
        let home = Arc::clone(registry.network(&path(HOME)).unwrap());
        let _ = published;

        registry.handle(&properties_changed(
            HOME,
            NETWORK_INTERFACE,
            &[(
                "KnownNetwork",
                Value::from(ObjectPathValue("/net/connman/iwd/686f6d65_psk")),
            )],
            &[],
        ));
        assert!(home.known.get());
        assert!(registry.known_network(&home).is_some());

        registry.handle(&properties_changed(
            HOME,
            NETWORK_INTERFACE,
            &[],
            &["KnownNetwork"],
        ));
        assert!(!home.known.get());
    }

    #[test]
    fn powering_off_clears_the_station() {
        let (mut registry, published) = loaded(vec![
            device(true, "connected"),
            network(HOME, "home", "psk"),
        ]);
        let station = published.station.get().unwrap();

        let removed = Message::signal("/", OBJECT_MANAGER_INTERFACE, "InterfacesRemoved")
            .unwrap()
            .build(&(path(DEVICE), vec![STATION_INTERFACE.to_owned()]))
            .unwrap();
        registry.handle(&removed);
        registry.handle(&properties_changed(
            DEVICE,
            DEVICE_INTERFACE,
            &[("Powered", Value::from(false))],
            &[],
        ));

        assert!(
            published
                .station
                .get()
                .is_some_and(|live| Arc::ptr_eq(&live, &station))
        );
        assert!(!station.powered.get());
        assert!(station.networks.get().is_empty());
    }

    #[test]
    fn wifi_is_on_only_while_the_adapter_is_unblocked() {
        const ADAPTER: &str = "/net/connman/iwd/0";
        let adapter = (
            path(ADAPTER),
            interfaces(vec![(
                ADAPTER_INTERFACE,
                props(&[("Powered", Value::from(true))]),
            )]),
        );
        let (device_path, mut device_interfaces) = device(true, "disconnected");
        // Only the Device interface reads it.
        for properties in device_interfaces.values_mut() {
            properties.extend(props(&[("Adapter", Value::from(ObjectPathValue(ADAPTER)))]));
        }
        let (mut registry, published) = loaded(vec![adapter, (device_path, device_interfaces)]);
        let station = published.station.get().unwrap();
        assert!(station.powered.get());
        assert_eq!(registry.station_power(), Some((true, true)));

        // An rfkill block (e.g. `rfkill block wlan`, or this service's toggle).
        registry.handle(&properties_changed(
            ADAPTER,
            ADAPTER_INTERFACE,
            &[("Powered", Value::from(false))],
            &[],
        ));

        assert!(!station.powered.get());
        assert_eq!(
            registry
                .adapters()
                .map(|(adapter, powered)| (adapter.as_str(), powered))
                .collect::<Vec<_>>(),
            vec![(ADAPTER, false)]
        );
    }

    #[test]
    fn a_connection_starting_clears_an_earlier_failure() {
        let (mut registry, published) = loaded(vec![
            device(true, "disconnected"),
            network(HOME, "home", "psk"),
        ]);
        let station = published.station.get().unwrap();
        let call = Message::method_call(HOME, "Connect")
            .unwrap()
            .build(&())
            .unwrap();
        station.last_error.set(Some(crate::types::StationError::new(
            crate::types::StationAction::Connect {
                network: Arc::clone(registry.network(&path(HOME)).unwrap()),
            },
            crate::Error::Dbus(zbus::Error::MethodError(
                zbus::names::ErrorName::from_static_str("net.connman.iwd.Failed")
                    .unwrap()
                    .into(),
                None,
                call,
            )),
        )));

        registry.handle(&properties_changed(
            DEVICE,
            STATION_INTERFACE,
            &[
                ("State", Value::from("connecting")),
                ("ConnectedNetwork", Value::from(ObjectPathValue(HOME))),
            ],
            &[],
        ));

        assert!(station.last_error.get().is_none());
    }

    #[test]
    fn the_networks_notify_when_a_listed_network_changes_in_place() {
        let (mut registry, published) = loaded(vec![
            device(true, "disconnected"),
            network(HOME, "home", "psk"),
            network(CAFE, "cafe", "open"),
        ]);
        let station = published.station.get().unwrap();
        let order = || vec![(path(HOME), -5_500), (path(CAFE), -8_000)];
        registry.set_ordered(&station, order());
        let mut networks = Box::pin(station.networks.watch());
        let mut notified = move || networks.next().now_or_never().is_some();
        assert!(notified(), "a watch starts with the current value");

        // The same order and signals: nothing to notify.
        registry.set_ordered(&station, order());
        assert!(!notified());

        // A signal changed, in the same order.
        registry.set_ordered(&station, vec![(path(HOME), -6_000), (path(CAFE), -8_000)]);
        assert!(notified());

        // A network became known, in the same order.
        registry.handle(&properties_changed(
            HOME,
            NETWORK_INTERFACE,
            &[(
                "KnownNetwork",
                Value::from(ObjectPathValue("/net/connman/iwd/686f6d65_psk")),
            )],
            &[],
        ));
        registry.set_ordered(&station, vec![(path(HOME), -6_000), (path(CAFE), -8_000)]);
        assert!(notified());
    }

    #[test]
    fn a_device_taking_over_as_the_station_gets_its_agent() {
        const OTHER_DEVICE: &str = "/net/connman/iwd/1/5";
        let (_, other_interfaces) = device(true, "connected");
        let other = (path(OTHER_DEVICE), other_interfaces);
        let (mut registry, published) = loaded(vec![device(true, "disconnected"), other]);
        assert_eq!(
            published.station.get().unwrap().object_path().as_str(),
            DEVICE
        );

        let removed = Message::signal("/", OBJECT_MANAGER_INTERFACE, "InterfacesRemoved")
            .unwrap()
            .build(&(
                path(DEVICE),
                vec![STATION_INTERFACE.to_owned(), DEVICE_INTERFACE.to_owned()],
            ))
            .unwrap();
        let changes = registry.handle(&removed);

        assert_eq!(
            published.station.get().unwrap().object_path().as_str(),
            OTHER_DEVICE
        );
        assert!(
            changes.station_up,
            "the new station's signal-level agent is registered"
        );
    }
}
