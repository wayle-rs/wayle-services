//! The one live instance of every BlueZ adapter and device, and the
//! service-level state derived from them.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use wayle_core::Property;
use zbus::{Message, fdo::ManagedObjects, names::OwnedInterfaceName, zvariant::OwnedObjectPath};

use super::command::Commands;
use crate::{
    core::{
        adapter::{Adapter, AdapterInfo, outcome_reported as adapter_outcome_reported},
        device::{
            Device, DeviceInfo, DeviceSignal,
            activity::{Reported, Ticket, Tracker},
            is_signal_property, outcome_reported, split_signal,
        },
    },
    error::Error,
    props::PropertyMap,
    types::{
        ADAPTER_INTERFACE, BATTERY_INTERFACE, DEVICE_INTERFACE, OBJECT_MANAGER_INTERFACE,
        PROPERTIES_INTERFACE, RadioBlock,
        adapter::{AdapterAction, AdapterError},
        agent::PairingRequest,
        device::{DeviceAction, DeviceActivity},
    },
};

/// The interfaces of one object, as `GetManagedObjects` and `InterfacesAdded`
/// deliver them.
pub(crate) type Interfaces = HashMap<OwnedInterfaceName, PropertyMap>;

/// The service-level properties, maintained by the registry (the pairing
/// request by the dispatcher's pairing state). The registry also takes
/// rfkill's block, which `available` counts.
#[derive(Clone)]
pub(crate) struct Published {
    pub adapters: Property<Vec<Arc<Adapter>>>,
    pub primary_adapter: Property<Option<Arc<Adapter>>>,
    pub devices: Property<Vec<Arc<Device>>>,
    /// Whether an adapter is present, or rfkill blocks a Bluetooth radio.
    pub available: Property<bool>,
    /// Whether no radio is blocked, and an adapter is on or turning on.
    pub enabled: Property<bool>,
    /// Whether no radio is blocked, and an adapter is on and not turning off.
    pub powered: Property<bool>,
    pub discovering: Property<bool>,
    pub connected: Property<Vec<Arc<Device>>>,
    pub pairing_request: Property<Option<PairingRequest>>,
    pub radio_block: Property<RadioBlock>,
}

impl Published {
    pub(crate) fn new() -> Self {
        Self {
            adapters: Property::new(Vec::new()),
            primary_adapter: Property::new(None),
            devices: Property::new(Vec::new()),
            available: Property::new(false),
            enabled: Property::new(false),
            powered: Property::new(false),
            discovering: Property::new(false),
            connected: Property::new(Vec::new()),
            pairing_request: Property::new(None),
            radio_block: Property::new(RadioBlock::None),
        }
    }
}

/// A device and the bookkeeping behind its activity.
pub(crate) struct DeviceEntry {
    pub device: Arc<Device>,
    tracker: Tracker,
}

impl DeviceEntry {
    /// Starts `activity` for a request about to be sent.
    pub(crate) fn begin(&mut self, activity: Option<DeviceActivity>) -> Ticket {
        self.update(|info, tracker| tracker.begin(info, activity))
    }

    /// Applies the reply to a request.
    pub(crate) fn finish(
        &mut self,
        action: DeviceAction,
        ticket: Ticket,
        result: Result<(), Error>,
    ) {
        self.update(|info, tracker| tracker.finish(info, action, ticket, result));
    }

    pub(crate) fn dismiss_error(&mut self) {
        self.update(|info, _| info.last_error = None);
    }

    /// Applies changed and invalidated `org.bluez.Device1` properties, and the
    /// outcomes they report, noting in `changes` what the service-level state
    /// and the pairing agent follow.
    fn apply_device1(
        &mut self,
        changed: PropertyMap,
        invalidated: &[String],
        changes: &mut Changes,
    ) {
        let (signal_changes, info_changes) = split_signal(changed);
        let (signal_invalidated, info_invalidated): (Vec<String>, Vec<String>) = invalidated
            .iter()
            .cloned()
            .partition(|name| is_signal_property(name));

        if !signal_changes.is_empty() || !signal_invalidated.is_empty() {
            let mut signal = DeviceSignal::clone(&self.device.signal.get());
            signal.apply(signal_changes, &signal_invalidated);
            self.device.signal.set(Arc::new(signal));
        }

        if info_changes.is_empty() && info_invalidated.is_empty() {
            return;
        }

        let reported = |property: &str| {
            info_changes.contains_key(property)
                || info_invalidated.iter().any(|name| name == property)
        };
        let reported = Reported {
            connected: reported("Connected"),
            paired: reported("Paired"),
        };
        changes.connected |= reported.connected;

        // As published: the state before this update.
        let old = self.device.info.get();
        let (became_paired, became_disconnected) = self.update(|info, tracker| {
            info.apply_device1(info_changes, &info_invalidated);

            if info
                .last_error
                .as_ref()
                .is_some_and(|error| outcome_reported(error.action, &old, info))
            {
                info.last_error = None;
            }
            tracker.settle(info, reported);

            (info.paired && !old.paired, old.connected && !info.connected)
        });

        let path = self.device.object_path.as_str();
        if became_paired {
            changes.paired.push(path.to_owned());
        }
        if became_disconnected {
            changes.disconnected.push(path.to_owned());
        }
    }

    fn apply_battery1(&mut self, changed: PropertyMap, invalidated: &[String]) {
        self.update(|info, _| info.apply_battery1(changed, invalidated));
    }

    /// Updates the device's info (and its tracker) in one step, publishing
    /// the result (with the tracker's activity) if it changed.
    fn update<R>(&mut self, f: impl FnOnce(&mut DeviceInfo, &mut Tracker) -> R) -> R {
        let mut info = DeviceInfo::clone(&self.device.info.get());
        let result = f(&mut info, &mut self.tracker);
        info.activity = self.tracker.activity();
        self.device.info.set(Arc::new(info));
        result
    }
}

/// Updates an adapter's info in one step, publishing the result if it changed.
pub(crate) fn update_adapter<R>(adapter: &Adapter, f: impl FnOnce(&mut AdapterInfo) -> R) -> R {
    let mut info = AdapterInfo::clone(&adapter.info.get());
    let result = f(&mut info);
    adapter.info.set(Arc::new(info));
    result
}

/// Applies changed and invalidated `org.bluez.Adapter1` properties to
/// `adapter`, clearing a failure whose outcome they report. Returns whether
/// the adapter powered off.
fn apply_adapter1(adapter: &Adapter, changed: PropertyMap, invalidated: &[String]) -> bool {
    // As published: the state before this update.
    let old = adapter.info.get();
    update_adapter(adapter, |info| {
        info.apply_adapter1(changed, invalidated);

        if info
            .last_error
            .as_ref()
            .is_some_and(|error| adapter_outcome_reported(error.action, &old, info))
        {
            info.last_error = None;
        }

        old.powered && !info.powered
    })
}

/// Records the reply to an adapter request: a failure becomes `last_error`,
/// and a success replaces an earlier failure of the same action.
pub(crate) fn finish_adapter(adapter: &Adapter, action: AdapterAction, result: Result<(), Error>) {
    match result {
        Ok(()) => update_adapter(adapter, |info| {
            info.last_error.take_if(|error| error.action == action);
        }),
        Err(err) => {
            tracing::warn!(adapter = %adapter.object_path, ?action, error = %err, "bluetooth adapter action failed");
            update_adapter(adapter, |info| {
                info.last_error = Some(AdapterError::new(action, err));
            });
        }
    }
}

/// The one live instance of every BlueZ adapter and device, keyed by object
/// path (as a string: `OwnedObjectPath` is not `Ord`, and signals are looked
/// up by `&str`). Ordered, so the published lists have a stable order.
pub(crate) struct Registry {
    commands: Commands,
    adapters: BTreeMap<String, Arc<Adapter>>,
    devices: BTreeMap<String, DeviceEntry>,
    /// rfkill's block on Bluetooth, which outlasts bluetoothd.
    radio_block: RadioBlock,
    published: Published,
}

/// What a signal changed, so only the affected published state is recomputed
/// and the dispatcher can react to lifecycle changes.
#[derive(Debug, Default)]
pub(crate) struct Changes {
    /// The set of adapters changed.
    pub adapters: bool,
    /// The set of devices changed.
    pub devices: bool,
    /// An adapter property (e.g. `Powered`, `Discovering`) changed.
    pub adapter_state: bool,
    /// A device's `Connected` property was reported.
    pub connected: bool,
    /// Devices that became paired.
    pub paired: Vec<String>,
    /// Devices that became disconnected.
    pub disconnected: Vec<String>,
    /// Devices that went away.
    pub removed: Vec<String>,
    /// Adapters that went away.
    pub removed_adapters: Vec<String>,
    /// Adapters that powered off (BlueZ ends their discovery sessions).
    pub powered_off: Vec<String>,
}

impl Changes {
    fn all() -> Self {
        Self {
            adapters: true,
            devices: true,
            adapter_state: true,
            connected: true,
            ..Self::default()
        }
    }
}

impl Registry {
    pub(crate) fn new(commands: &Commands, published: Published) -> Self {
        Self {
            commands: commands.clone(),
            adapters: BTreeMap::new(),
            devices: BTreeMap::new(),
            radio_block: RadioBlock::None,
            published,
        }
    }

    /// Takes rfkill's block on Bluetooth: published as is, and counted by
    /// `available` (on some machines a block takes the adapter away),
    /// `enabled` and `powered` (a blocked radio turns Bluetooth off, as in
    /// KDE).
    pub(crate) fn set_radio_block(&mut self, block: RadioBlock) {
        if self.radio_block == block {
            return;
        }
        self.radio_block = block;
        self.published.radio_block.set(block);
        self.publish(&Changes {
            adapter_state: true,
            ..Changes::default()
        });
    }

    /// The entry of `device`, if that instance is still the live one.
    pub(crate) fn entry_of(&mut self, device: &Arc<Device>) -> Option<&mut DeviceEntry> {
        self.devices
            .get_mut(device.object_path.as_str())
            .filter(|entry| Arc::ptr_eq(&entry.device, device))
    }

    pub(crate) fn adapter(&self, path: &str) -> Option<&Arc<Adapter>> {
        self.adapters.get(path)
    }

    /// Whether `adapter` is still the live instance.
    pub(crate) fn is_live(&self, adapter: &Arc<Adapter>) -> bool {
        self.adapters
            .get(adapter.object_path.as_str())
            .is_some_and(|live| Arc::ptr_eq(live, adapter))
    }

    pub(crate) fn adapters(&self) -> impl Iterator<Item = &Arc<Adapter>> {
        self.adapters.values()
    }

    pub(crate) fn primary_adapter(&self) -> Option<Arc<Adapter>> {
        self.published.primary_adapter.get()
    }

    /// Replaces the registry's contents with a freshly enumerated session.
    pub(crate) fn load(&mut self, objects: ManagedObjects, buffered: &[Message]) {
        self.adapters.clear();
        self.devices.clear();

        let mut changes = Changes::default();
        for (path, interfaces) in objects {
            self.add_interfaces(path, interfaces, &mut changes);
        }
        for message in buffered {
            self.apply(message, &mut changes);
        }

        self.publish(&Changes::all());
    }

    /// Drops every object, e.g. because bluetoothd went away.
    pub(crate) fn clear(&mut self) {
        self.adapters.clear();
        self.devices.clear();
        self.publish(&Changes::all());
    }

    /// Applies one signal and republishes whatever it changed.
    pub(crate) fn handle(&mut self, message: &Message) -> Changes {
        let mut changes = Changes::default();
        self.apply(message, &mut changes);
        self.publish(&changes);
        changes
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
                // Signals for objects we don't model (GATT characteristics,
                // media players, ...) are dropped here, before their body is
                // decoded.
                let path = path.as_str();
                if !self.adapters.contains_key(path) && !self.devices.contains_key(path) {
                    return;
                }

                let Ok((interface, changed, invalidated)) =
                    message
                        .body()
                        .deserialize::<(String, PropertyMap, Vec<String>)>()
                else {
                    return;
                };

                match interface.as_str() {
                    ADAPTER_INTERFACE => {
                        if let Some(adapter) = self.adapters.get(path) {
                            if apply_adapter1(adapter, changed, &invalidated) {
                                changes.powered_off.push(path.to_owned());
                            }
                            changes.adapter_state = true;
                        }
                    }
                    DEVICE_INTERFACE => {
                        if let Some(entry) = self.devices.get_mut(path) {
                            entry.apply_device1(changed, &invalidated, changes);
                        }
                    }
                    BATTERY_INTERFACE => {
                        if let Some(entry) = self.devices.get_mut(path) {
                            entry.apply_battery1(changed, &invalidated);
                        }
                    }
                    _ => {}
                }
            }
            (OBJECT_MANAGER_INTERFACE, "InterfacesAdded") => {
                if let Ok((path, interfaces)) = message
                    .body()
                    .deserialize::<(OwnedObjectPath, Interfaces)>()
                {
                    self.add_interfaces(path, interfaces, changes);
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
        path: OwnedObjectPath,
        mut interfaces: Interfaces,
        changes: &mut Changes,
    ) {
        if let Some(adapter1) = interfaces.remove(ADAPTER_INTERFACE) {
            match self.adapters.get(path.as_str()) {
                Some(adapter) => {
                    if apply_adapter1(adapter, adapter1, &[]) {
                        changes.powered_off.push(path.to_string());
                    }
                    changes.adapter_state = true;
                }
                None => {
                    let adapter = Adapter::new(&self.commands, path.clone(), adapter1);
                    self.adapters.insert(path.to_string(), Arc::new(adapter));
                    changes.adapters = true;
                }
            }
        }

        let battery1 = interfaces.remove(BATTERY_INTERFACE);
        let key = path.to_string();
        match (
            interfaces.remove(DEVICE_INTERFACE),
            self.devices.get_mut(&key),
        ) {
            (Some(device1), Some(entry)) => {
                entry.apply_device1(device1, &[], changes);
                if let Some(battery1) = battery1 {
                    entry.apply_battery1(battery1, &[]);
                }
            }
            (Some(device1), None) => {
                let device = Device::new(&self.commands, path, device1, battery1);
                self.devices.insert(
                    key,
                    DeviceEntry {
                        device: Arc::new(device),
                        tracker: Tracker::default(),
                    },
                );
                changes.devices = true;
            }
            (None, Some(entry)) => {
                if let Some(battery1) = battery1 {
                    entry.apply_battery1(battery1, &[]);
                }
            }
            (None, None) => {}
        }
    }

    fn remove_interfaces(
        &mut self,
        path: &OwnedObjectPath,
        interfaces: &[String],
        changes: &mut Changes,
    ) {
        for interface in interfaces {
            match interface.as_str() {
                ADAPTER_INTERFACE => {
                    if self.adapters.remove(path.as_str()).is_some() {
                        changes.adapters = true;
                        changes.removed_adapters.push(path.to_string());
                    }
                }
                DEVICE_INTERFACE => {
                    if self.devices.remove(path.as_str()).is_some() {
                        changes.devices = true;
                        changes.removed.push(path.to_string());
                    }
                }
                BATTERY_INTERFACE => {
                    if let Some(entry) = self.devices.get_mut(path.as_str()) {
                        entry.apply_battery1(PropertyMap::new(), &["Percentage".to_owned()]);
                    }
                }
                _ => {}
            }
        }
    }

    /// Republishes whatever `changes` affected: the object lists, and the
    /// service-level state derived from them.
    fn publish(&self, changes: &Changes) {
        let published = &self.published;

        if changes.adapters {
            published
                .adapters
                .set(self.adapters.values().cloned().collect());
        }
        if changes.devices {
            published.devices.set(
                self.devices
                    .values()
                    .map(|entry| entry.device.clone())
                    .collect(),
            );
        }

        if changes.adapters || changes.adapter_state {
            let primary =
                select_primary_adapter(published.primary_adapter.get(), self.adapters.values());
            let available = primary.is_some() || self.radio_block != RadioBlock::None;
            let discovering = primary
                .as_ref()
                .is_some_and(|adapter| adapter.info.get().discovering);

            // The primary adapter first: whoever reacts to the others reads it.
            published.primary_adapter.set(primary);
            published.available.set(available);
            let unblocked = self.radio_block == RadioBlock::None;
            published.enabled.set(
                unblocked
                    && self
                        .adapters
                        .values()
                        .any(|adapter| adapter.info.get().power_target()),
            );
            published.powered.set(
                unblocked
                    && self
                        .adapters
                        .values()
                        .any(|adapter| adapter.info.get().usable()),
            );
            published.discovering.set(discovering);
        }

        if changes.devices || changes.connected {
            published.connected.set(
                self.devices
                    .values()
                    .filter(|entry| entry.device.info.get().connected)
                    .map(|entry| entry.device.clone())
                    .collect(),
            );
        }
    }
}

/// Keeps the current primary adapter while it is present and powered;
/// otherwise prefers a powered adapter, falling back to the current one and
/// then to any adapter.
fn select_primary_adapter<'a>(
    current: Option<Arc<Adapter>>,
    adapters: impl Iterator<Item = &'a Arc<Adapter>> + Clone,
) -> Option<Arc<Adapter>> {
    let current = current.filter(|current| adapters.clone().any(|adapter| adapter == current));

    if let Some(current) = &current
        && current.info.get().powered
    {
        return Some(current.clone());
    }

    adapters
        .clone()
        .find(|adapter| adapter.info.get().powered)
        .cloned()
        .or(current)
        .or_else(|| adapters.clone().next().cloned())
}

#[cfg(test)]
mod tests {
    use zbus::{
        names::InterfaceName,
        zvariant::{ObjectPath, OwnedValue, Value},
    };

    use super::*;

    const ADAPTER: &str = "/org/bluez/hci0";
    const HEADPHONES: &str = "/org/bluez/hci0/dev_00_11_22_33_44_55";
    const BEACON: &str = "/org/bluez/hci0/dev_66_77_88_99_AA_BB";

    fn props(entries: &[(&str, Value<'_>)]) -> PropertyMap {
        entries
            .iter()
            .map(|(name, value)| {
                (
                    (*name).to_owned(),
                    OwnedValue::try_from(value.try_clone().unwrap()).unwrap(),
                )
            })
            .collect()
    }

    fn interfaces(entries: Vec<(&'static str, PropertyMap)>) -> Interfaces {
        entries
            .into_iter()
            .map(|(name, props)| (InterfaceName::from_static_str(name).unwrap().into(), props))
            .collect()
    }

    fn path(path: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(path).unwrap()
    }

    fn adapter(powered: bool) -> (OwnedObjectPath, Interfaces) {
        (
            path(ADAPTER),
            interfaces(vec![(
                ADAPTER_INTERFACE,
                props(&[
                    ("Powered", Value::from(powered)),
                    ("Alias", Value::from("laptop")),
                ]),
            )]),
        )
    }

    fn device(at: &str, alias: &str, connected: bool) -> (OwnedObjectPath, Interfaces) {
        (
            path(at),
            interfaces(vec![(
                DEVICE_INTERFACE,
                props(&[
                    ("Alias", Value::from(alias)),
                    ("Connected", Value::from(connected)),
                    ("RSSI", Value::from(-60_i16)),
                    (
                        "Adapter",
                        Value::from(ObjectPath::try_from(ADAPTER).unwrap()),
                    ),
                ]),
            )]),
        )
    }

    fn properties_changed(
        at: &str,
        interface: &str,
        changed: &[(&str, Value<'_>)],
        invalidated: &[&str],
    ) -> Message {
        let changed: HashMap<&str, &Value<'_>> =
            changed.iter().map(|(name, value)| (*name, value)).collect();
        Message::signal(at, PROPERTIES_INTERFACE, "PropertiesChanged")
            .unwrap()
            .build(&(interface, changed, invalidated))
            .unwrap()
    }

    fn interfaces_added(at: &str, added: &[(&str, &[(&str, Value<'_>)])]) -> Message {
        let added: HashMap<&str, HashMap<&str, &Value<'_>>> = added
            .iter()
            .map(|(interface, props)| {
                (
                    *interface,
                    props.iter().map(|(name, value)| (*name, value)).collect(),
                )
            })
            .collect();
        Message::signal("/", OBJECT_MANAGER_INTERFACE, "InterfacesAdded")
            .unwrap()
            .build(&(ObjectPath::try_from(at).unwrap(), added))
            .unwrap()
    }

    fn interfaces_removed(at: &str, removed: &[&str]) -> Message {
        Message::signal("/", OBJECT_MANAGER_INTERFACE, "InterfacesRemoved")
            .unwrap()
            .build(&(ObjectPath::try_from(at).unwrap(), removed))
            .unwrap()
    }

    fn loaded(objects: Vec<(OwnedObjectPath, Interfaces)>) -> (Registry, Published) {
        let (commands, _) = Commands::channel();
        let published = Published::new();
        let mut registry = Registry::new(&commands, published.clone());
        registry.load(objects.into_iter().collect(), &[]);
        (registry, published)
    }

    fn device_at(published: &Published, at: &str) -> Arc<Device> {
        published
            .devices
            .get()
            .into_iter()
            .find(|device| device.object_path.as_str() == at)
            .unwrap()
    }

    #[test]
    fn load_publishes_objects_and_derived_state() {
        let (_registry, published) = loaded(vec![
            adapter(true),
            device(HEADPHONES, "Headphones", true),
            device(BEACON, "Beacon", false),
        ]);

        assert_eq!(published.adapters.get().len(), 1);
        assert_eq!(published.devices.get().len(), 2);
        assert!(published.available.get());
        assert!(published.enabled.get());
        assert!(!published.discovering.get());

        let connected = published.connected.get();
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].info.get().alias, "Headphones");
        assert_eq!(device_at(&published, BEACON).signal.get().rssi, Some(-60));
    }

    #[test]
    fn properties_changed_updates_the_same_instance() {
        let (mut registry, published) =
            loaded(vec![adapter(true), device(BEACON, "Beacon", false)]);
        let beacon = device_at(&published, BEACON);

        registry.handle(&properties_changed(
            BEACON,
            DEVICE_INTERFACE,
            &[
                ("Connected", Value::from(true)),
                ("Alias", Value::from("Speaker")),
            ],
            &[],
        ));

        assert!(beacon.info.get().connected);
        assert_eq!(beacon.info.get().alias, "Speaker");
        assert!(Arc::ptr_eq(&beacon, &device_at(&published, BEACON)));
        assert!(Arc::ptr_eq(&beacon, &published.connected.get()[0]));
    }

    #[test]
    fn signal_changes_leave_info_untouched() {
        let (mut registry, published) =
            loaded(vec![adapter(true), device(BEACON, "Beacon", false)]);
        let beacon = device_at(&published, BEACON);
        let info = beacon.info.get();

        registry.handle(&properties_changed(
            BEACON,
            DEVICE_INTERFACE,
            &[("RSSI", Value::from(-50_i16))],
            &[],
        ));

        assert_eq!(beacon.signal.get().rssi, Some(-50));
        assert!(Arc::ptr_eq(&info, &beacon.info.get()));
    }

    #[test]
    fn invalidated_signal_properties_are_cleared() {
        let (mut registry, published) =
            loaded(vec![adapter(true), device(BEACON, "Beacon", false)]);

        registry.handle(&properties_changed(
            BEACON,
            DEVICE_INTERFACE,
            &[],
            &["RSSI", "AdvertisingFlags"],
        ));

        let signal = device_at(&published, BEACON).signal.get();
        assert_eq!(signal.rssi, None);
        assert!(signal.advertising_flags.is_empty());
    }

    #[test]
    fn interfaces_added_and_removed_track_devices_and_battery() {
        let (mut registry, published) = loaded(vec![adapter(true)]);

        registry.handle(&interfaces_added(
            HEADPHONES,
            &[
                (DEVICE_INTERFACE, &[("Alias", Value::from("Headphones"))]),
                (BATTERY_INTERFACE, &[("Percentage", Value::from(80_u8))]),
            ],
        ));
        let headphones = device_at(&published, HEADPHONES);
        assert_eq!(headphones.info.get().battery_percentage, Some(80));

        registry.handle(&interfaces_removed(HEADPHONES, &[BATTERY_INTERFACE]));
        assert_eq!(headphones.info.get().battery_percentage, None);
        assert_eq!(published.devices.get().len(), 1);

        let changes = registry.handle(&interfaces_removed(HEADPHONES, &[DEVICE_INTERFACE]));
        assert!(published.devices.get().is_empty());
        assert_eq!(changes.removed, vec![HEADPHONES.to_owned()]);
    }

    #[test]
    fn a_re_added_device_is_a_new_instance() {
        let (mut registry, published) =
            loaded(vec![adapter(true), device(BEACON, "Beacon", false)]);
        let before = device_at(&published, BEACON);

        registry.handle(&interfaces_removed(BEACON, &[DEVICE_INTERFACE]));
        registry.handle(&interfaces_added(
            BEACON,
            &[(DEVICE_INTERFACE, &[("Alias", Value::from("Beacon"))])],
        ));

        assert!(!Arc::ptr_eq(&before, &device_at(&published, BEACON)));
        assert_ne!(*before, *device_at(&published, BEACON));
    }

    #[test]
    fn becoming_paired_is_reported() {
        let (mut registry, _published) =
            loaded(vec![adapter(true), device(BEACON, "Beacon", false)]);

        let changes = registry.handle(&properties_changed(
            BEACON,
            DEVICE_INTERFACE,
            &[("Paired", Value::from(true))],
            &[],
        ));

        assert_eq!(changes.paired, vec![BEACON.to_owned()]);
    }

    #[test]
    fn derived_state_follows_primary_adapter_properties() {
        let (mut registry, published) = loaded(vec![adapter(false)]);
        assert!(published.available.get());
        assert!(!published.enabled.get());

        registry.handle(&properties_changed(
            ADAPTER,
            ADAPTER_INTERFACE,
            &[
                ("Powered", Value::from(true)),
                ("Discovering", Value::from(true)),
            ],
            &[],
        ));
        assert!(published.enabled.get());
        assert!(published.discovering.get());

        registry.handle(&interfaces_removed(ADAPTER, &[ADAPTER_INTERFACE]));
        assert!(!published.available.get());
        assert!(published.primary_adapter.get().is_none());
    }

    #[test]
    fn a_blocked_radio_turns_bluetooth_off() {
        let (mut registry, published) = loaded(vec![adapter(true)]);
        assert!(published.enabled.get() && published.powered.get());

        // Even before BlueZ reports the adapter off, and even if another
        // adapter stays on.
        registry.set_radio_block(RadioBlock::Software);
        assert!(!published.enabled.get());
        assert!(!published.powered.get());

        registry.set_radio_block(RadioBlock::None);
        assert!(published.enabled.get() && published.powered.get());
    }

    #[test]
    fn a_blocked_radio_counts_as_available() {
        // On some machines a block takes the adapter away.
        let (mut registry, published) = loaded(vec![]);
        assert!(!published.available.get());

        registry.set_radio_block(RadioBlock::Software);
        assert!(published.available.get());
        assert_eq!(published.radio_block.get(), RadioBlock::Software);

        registry.set_radio_block(RadioBlock::None);
        assert!(!published.available.get());
    }

    #[test]
    fn enabled_means_any_adapter_is_powered() {
        const SECOND: &str = "/org/bluez/hci1";
        let second = (
            path(SECOND),
            interfaces(vec![(
                ADAPTER_INTERFACE,
                props(&[("Powered", Value::from(true))]),
            )]),
        );
        let (mut registry, published) = loaded(vec![adapter(true), second]);
        assert!(published.enabled.get());

        let powered_off = |at| {
            properties_changed(
                at,
                ADAPTER_INTERFACE,
                &[("Powered", Value::from(false))],
                &[],
            )
        };

        registry.handle(&powered_off(ADAPTER));
        assert!(published.enabled.get(), "hci1 is still powered");

        registry.handle(&powered_off(SECOND));
        assert!(!published.enabled.get());
        assert!(published.available.get());
    }

    #[test]
    fn enabled_follows_bluez_power_transitions() {
        let (mut registry, published) = loaded(vec![adapter(false)]);
        let power_state = |state: &'static str| {
            properties_changed(
                ADAPTER,
                ADAPTER_INTERFACE,
                &[("PowerState", Value::from(state))],
                &[],
            )
        };

        // Any client asked to power on; BlueZ accepted it.
        registry.handle(&power_state("off-enabling"));
        assert!(published.enabled.get());

        // The request failed and BlueZ reverted.
        registry.handle(&power_state("off"));
        assert!(!published.enabled.get());
    }

    #[test]
    fn an_adapter_error_survives_the_revert_of_its_failed_request() {
        let (mut registry, published) = loaded(vec![adapter(false)]);
        let hci0 = published.primary_adapter.get().unwrap();

        registry.handle(&properties_changed(
            ADAPTER,
            ADAPTER_INTERFACE,
            &[("PowerState", Value::from("off-enabling"))],
            &[],
        ));
        finish_adapter(
            &hci0,
            AdapterAction::SetPowered,
            Err(Error::Dbus(zbus::Error::Failure("rfkill".into()))),
        );
        registry.handle(&properties_changed(
            ADAPTER,
            ADAPTER_INTERFACE,
            &[("PowerState", Value::from("off"))],
            &[],
        ));
        assert!(hci0.info.get().last_error.is_some());

        // Powering on (by any client) is the outcome it was after.
        registry.handle(&properties_changed(
            ADAPTER,
            ADAPTER_INTERFACE,
            &[("Powered", Value::from(true))],
            &[],
        ));
        assert!(hci0.info.get().last_error.is_none());
    }

    #[test]
    fn signals_for_unmodelled_objects_are_ignored() {
        let (mut registry, published) =
            loaded(vec![adapter(true), device(BEACON, "Beacon", false)]);
        let characteristic = format!("{BEACON}/service000a/char000b");

        registry.handle(&properties_changed(
            &characteristic,
            "org.bluez.GattCharacteristic1",
            &[("Value", Value::from(vec![1_u8, 2, 3]))],
            &[],
        ));

        assert_eq!(published.devices.get().len(), 1);
    }

    #[test]
    fn clear_drops_everything() {
        let (mut registry, published) =
            loaded(vec![adapter(true), device(HEADPHONES, "Headphones", true)]);

        registry.clear();

        assert!(published.devices.get().is_empty());
        assert!(published.connected.get().is_empty());
        assert!(!published.available.get());
    }

    #[test]
    fn becoming_disconnected_is_reported() {
        let (mut registry, _published) =
            loaded(vec![adapter(true), device(HEADPHONES, "Headphones", true)]);

        let changes = registry.handle(&properties_changed(
            HEADPHONES,
            DEVICE_INTERFACE,
            &[("Connected", Value::from(false))],
            &[],
        ));

        assert_eq!(changes.disconnected, vec![HEADPHONES.to_owned()]);
        assert!(changes.paired.is_empty());
    }

    #[test]
    fn powered_waits_for_a_power_on_but_not_a_power_off() {
        let (mut registry, published) = loaded(vec![adapter(false)]);

        registry.handle(&properties_changed(
            ADAPTER,
            ADAPTER_INTERFACE,
            &[("PowerState", Value::from("off-enabling"))],
            &[],
        ));
        assert!(published.enabled.get());
        assert!(!published.powered.get());

        registry.handle(&properties_changed(
            ADAPTER,
            ADAPTER_INTERFACE,
            &[
                ("Powered", Value::from(true)),
                ("PowerState", Value::from("on")),
            ],
            &[],
        ));
        assert!(published.powered.get());

        // BlueZ refuses connects and discovery from here on.
        registry.handle(&properties_changed(
            ADAPTER,
            ADAPTER_INTERFACE,
            &[("PowerState", Value::from("on-disabling"))],
            &[],
        ));
        assert!(!published.enabled.get());
        assert!(!published.powered.get());
    }

    #[test]
    fn an_adapter_powering_off_is_reported() {
        let (mut registry, _published) = loaded(vec![adapter(true)]);

        let changes = registry.handle(&properties_changed(
            ADAPTER,
            ADAPTER_INTERFACE,
            &[("Powered", Value::from(false))],
            &[],
        ));

        assert_eq!(changes.powered_off, vec![ADAPTER.to_owned()]);
    }

    #[test]
    fn a_later_success_clears_an_adapter_failure() {
        let (_registry, published) = loaded(vec![adapter(true)]);
        let hci0 = published.primary_adapter.get().unwrap();
        let failed = || Err(Error::Dbus(zbus::Error::Failure("not ready".into())));

        finish_adapter(&hci0, AdapterAction::SetDiscoveryFilter, failed());
        finish_adapter(&hci0, AdapterAction::SetAlias, Ok(()));
        assert!(
            hci0.info.get().last_error.is_some(),
            "another action's success"
        );

        finish_adapter(&hci0, AdapterAction::SetDiscoveryFilter, Ok(()));
        assert!(hci0.info.get().last_error.is_none());
    }

    #[test]
    fn a_removed_adapter_is_reported() {
        let (mut registry, _published) = loaded(vec![adapter(true)]);

        let changes = registry.handle(&interfaces_removed(ADAPTER, &[ADAPTER_INTERFACE]));

        assert_eq!(changes.removed_adapters, vec![ADAPTER.to_owned()]);
    }
}
