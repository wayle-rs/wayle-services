pub(crate) mod activity;
pub(crate) mod types;

use std::{collections::HashMap, ptr, sync::Arc};

use derive_more::Debug;
pub use types::{AdvertisingData, DeviceSet, ManufacturerData, ServiceData};
use wayle_core::Property;
use zbus::zvariant::OwnedObjectPath;

use crate::{
    dispatcher::{
        Call,
        command::{Command, Commands, Query},
    },
    error::Error,
    props::{self, PropertyMap},
    types::{
        ADAPTER_INTERFACE, DEVICE_INTERFACE, UUID,
        adapter::AddressType,
        device::{DeviceAction, DeviceActivity, DeviceError, PreferredBearer},
    },
};

/// Device1 properties carried by [`DeviceSignal`] rather than [`DeviceInfo`].
const SIGNAL_PROPERTIES: [&str; 6] = [
    "RSSI",
    "TxPower",
    "ManufacturerData",
    "ServiceData",
    "AdvertisingFlags",
    "AdvertisingData",
];

/// Bluetooth device from BlueZ.
///
/// Every `Device` handed out by [`BluetoothService`](crate::BluetoothService) is
/// **live**: its state tracks BlueZ for as long as the device exists. There is
/// exactly one instance per device (look one up by path with
/// [`BluetoothService::device`](crate::BluetoothService::device)), shared as
/// an `Arc`, and devices compare equal only if they are the same instance. A
/// device BlueZ removes and re-adds is a new instance.
///
/// State is split by how often it changes: [`info`](Self::info) holds
/// everything about the device, and [`signal`](Self::signal) its advertisement
/// data (RSSI and friends), which changes with nearly every advertisement
/// while scanning. Each is replaced as a whole when any of its fields change.
///
/// # Actions
///
/// Actions return nothing: each is queued to the service, which sends them to
/// BlueZ in call order, and its outcome shows up only as state, the same way a
/// change made by any other BlueZ client would: `connected`, `paired`, the
/// device being removed, and so on. While one is in flight
/// [`DeviceInfo::activity`] says so, and a failure is recorded in
/// [`DeviceInfo::last_error`]. An action acts on this instance: once the
/// device is removed (or replaced by a new instance), it does nothing.
///
/// - [`connect()`](Self::connect) / [`disconnect()`](Self::disconnect) - Manage connection
/// - [`pair()`](Self::pair) / [`cancel_pairing()`](Self::cancel_pairing) - Pairing flow
/// - [`connect_profile()`](Self::connect_profile) /
///   [`disconnect_profile()`](Self::disconnect_profile) - Profile-specific
/// - [`set_trusted()`](Self::set_trusted) / [`set_blocked()`](Self::set_blocked) -
///   Trust and block settings
/// - [`set_alias()`](Self::set_alias) - Custom display name
/// - [`forget()`](Self::forget) - Remove from adapter and clear bonding
#[derive(Debug)]
pub struct Device {
    /// Where actions and queries are queued.
    #[debug(skip)]
    commands: Commands,

    /// D-Bus object path for this device.
    pub object_path: OwnedObjectPath,

    /// Everything about the device except its advertisement data (live).
    pub info: Property<Arc<DeviceInfo>>,

    /// The device's advertisement data (live). Changes with nearly every
    /// advertisement while scanning.
    pub signal: Property<Arc<DeviceSignal>>,
}

/// Everything about a [`Device`] except its advertisement data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The Bluetooth device address of the remote device.
    pub address: String,

    /// The Bluetooth device Address Type. For dual-mode and BR/EDR only devices this
    /// defaults to "public". Single mode LE devices may have either value.
    ///
    /// If remote device uses privacy than before pairing this represents address type
    /// used for connection and Identity Address after pairing.
    pub address_type: AddressType,

    /// The Bluetooth remote name.
    ///
    /// This value is only present for completeness. It is better to always use the
    /// Alias property when displaying the devices name.
    ///
    /// If the Alias property is unset, it will reflect this value which makes it
    /// more convenient.
    pub name: Option<String>,

    /// Proposed icon name according to the freedesktop.org icon naming specification.
    pub icon: Option<String>,

    /// Battery charge percentage of the device (0-100).
    ///
    /// Only available for devices that support battery reporting.
    /// `None` if the device doesn't have a battery or doesn't report battery status.
    pub battery_percentage: Option<u8>,

    /// The Bluetooth class of device of the remote device.
    pub class: Option<u32>,

    /// External appearance of device, as found on GAP service.
    pub appearance: Option<u16>,

    /// List of 128-bit UUIDs that represents the available remote services.
    pub uuids: Option<Vec<UUID>>,

    /// Indicates if the remote device is paired. Paired means the pairing process where
    /// devices exchange the information to establish an encrypted connection has been
    /// completed.
    pub paired: bool,

    /// Indicates if the remote device is bonded. Bonded means the information exchanged
    /// on pairing process has been stored and will be persisted.
    pub bonded: bool,

    /// Indicates if the remote device is currently connected.
    ///
    /// A PropertiesChanged signal indicate changes to this status.
    pub connected: bool,

    /// Indicates if the remote is seen as trusted.
    ///
    /// This setting can be changed by the application.
    pub trusted: bool,

    /// If set to true any incoming connections from the device will be immediately
    /// rejected.
    ///
    /// Any device drivers will also be removed and no new ones will be probed as long
    /// as the device is blocked.
    pub blocked: bool,

    /// If set to true this device will be allowed to wake the host from system suspend.
    pub wake_allowed: bool,

    /// The name alias for the remote device. The alias can be used to have a different
    /// friendly name for the remote device.
    ///
    /// In case no alias is set, it will return the remote device name. Setting an empty
    /// string as alias will convert it back to the remote device name.
    ///
    /// When resetting the alias with an empty string, the property will default back to
    /// the remote name.
    pub alias: String,

    /// The object path of the adapter the device belongs to.
    pub adapter: OwnedObjectPath,

    /// Set to true if the device only supports the pre-2.1 pairing mechanism.
    ///
    /// This property is useful during device discovery to anticipate whether legacy or
    /// simple pairing will occur if pairing is initiated.
    ///
    /// Note that this property can exhibit false-positives in the case of Bluetooth 2.1
    /// (or newer) devices that have disabled Extended Inquiry Response support.
    pub legacy_pairing: bool,

    /// Set to true if the device was cable paired and it doesn't support the canonical
    /// bonding with encryption, e.g. the Sixaxis gamepad.
    ///
    /// If true, BlueZ will establish a connection without enforcing encryption.
    pub cable_pairing: bool,

    /// Remote Device ID information in modalias format used by the kernel and udev.
    pub modalias: Option<String>,

    /// Indicate whether or not service discovery has been resolved.
    pub services_resolved: bool,

    /// The object paths of the sets the device belongs to followed by a dictionary
    /// which can contain the following:
    ///
    /// - byte Rank: Rank of the device in the Set.
    ///
    /// (BlueZ experimental)
    pub sets: Vec<DeviceSet>,

    /// Indicate the preferred bearer when initiating a connection, only available for
    /// dual-mode devices.
    ///
    /// When changing from "bredr" to "le" the device will be removed from the
    /// 'auto-connect' list so it won't automatically be connected when adverting.
    ///
    /// Note: Changes only take effect when the device is disconnected.
    ///
    /// (BlueZ experimental)
    pub preferred_bearer: Option<PreferredBearer>,

    /// The operation this service is currently performing on the device. See
    /// [`DeviceActivity`] for which operations are tracked.
    pub activity: DeviceActivity,

    /// The most recent action on this device that failed, with the complete
    /// error. Cleared when BlueZ reports the outcome that action was after
    /// (e.g. the device connects, by any client, after a failed connect), when
    /// the same action later succeeds, when another connect, disconnect, pair
    /// or forget starts, or by [`Device::dismiss_error`].
    pub last_error: Option<DeviceError>,
}

/// A [`Device`]'s advertisement data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSignal {
    /// Received Signal Strength Indicator of the remote device (inquiry or advertising).
    pub rssi: Option<i16>,

    /// Advertised transmitted power level (inquiry or advertising).
    pub tx_power: Option<i16>,

    /// Manufacturer specific advertisement data. Keys are 16 bits Manufacturer ID
    /// followed by its byte array value.
    pub manufacturer_data: Option<ManufacturerData>,

    /// Service advertisement data. Keys are the UUIDs in string format followed by its
    /// byte array value.
    pub service_data: Option<ServiceData>,

    /// The Advertising Data Flags of the remote device.
    pub advertising_flags: Vec<u8>,

    /// The Advertising Data of the remote device. Keys are 1 byte AD Type followed by
    /// data as byte array.
    ///
    /// Note: Only types considered safe to be handled by application are exposed.
    pub advertising_data: AdvertisingData,
}

/// A device is equal only to itself: there is one instance per BlueZ device
/// object, so a replaced instance (removed and re-added) is a change.
impl PartialEq for Device {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self, other)
    }
}

impl Eq for Device {}

impl Device {
    /// Connects all profiles the remote device supports that can be connected to and
    /// have been flagged as auto-connectable. If only subset of profiles is already
    /// connected it will try to connect currently disconnected ones.
    ///
    /// If at least one profile was connected successfully this method will indicate
    /// success.
    ///
    /// For dual-mode devices only one bearer is connected at time, the conditions are
    /// in the following order:
    ///
    /// 1. Connect the disconnected bearer if already connected.
    ///
    /// 2. Connect first the bonded bearer. If no bearers are bonded or both are skip
    ///    and check latest seen bearer.
    ///
    /// 3. Connect last used bearer, in case the timestamps are the same BR/EDR
    ///    takes precedence, or in case PreferredBearer has been set to a specific
    ///    bearer then that is used instead.
    ///
    /// # Failures
    ///
    /// Recorded in [`DeviceInfo::last_error`]; BlueZ may report:
    /// - `NotReady` - Adapter not ready
    /// - `Failed` - Operation failed
    /// - `AlreadyConnected` - Already connected
    /// - `BrEdrProfileUnavailable` - BR/EDR profile unavailable
    ///
    /// While BlueZ is already connecting the device (for this service or
    /// another client), it turns the request down as `InProgress`. That isn't
    /// recorded: the connect under way reports the outcome.
    pub fn connect(self: &Arc<Self>) {
        self.request(DeviceAction::Connect, self.method("Connect"));
    }

    /// Disconnects all connected profiles and terminates the low-level ACL connection.
    ///
    /// ACL connection terminates even if some profiles fail to disconnect properly
    /// (e.g., due to misbehaving device).
    ///
    /// Can also cancel a pending Connect call before receiving its reply.
    ///
    /// For non-trusted LE devices, disables incoming connections until Connect is called again.
    ///
    /// # Failures
    ///
    /// A failure is recorded in [`DeviceInfo::last_error`]. BlueZ documents
    /// `NotConnected`, but since 5.6 it answers a disconnect of a device that
    /// isn't connected with success.
    pub fn disconnect(self: &Arc<Self>) {
        let (action, call) = self.disconnect_request();
        self.request(action, call);
    }

    /// Connects to the remote device and initiate pairing procedure then proceed with
    /// service discovery.
    ///
    /// If the application has registered its own agent, then that specific agent will
    /// be used. Otherwise it will use the default agent.
    ///
    /// Only for applications like a pairing wizard it would make sense to have its own
    /// agent. In almost all other cases the default agent will handle this just fine.
    ///
    /// In case there is no application agent and also no default agent present, this
    /// method will fail.
    ///
    /// # Failures
    ///
    /// Recorded in [`DeviceInfo::last_error`]; BlueZ may report:
    /// - `InvalidArguments` - Invalid arguments
    /// - `Failed` - Operation failed
    /// - `AuthenticationFailed` - Authentication failed
    /// - `AuthenticationRejected` - Authentication rejected
    /// - `AuthenticationTimeout` - Authentication timeout
    /// - `ConnectionAttemptFailed` - Connection attempt failed
    ///
    /// Not recorded, as they aren't failures: `AlreadyExists` (already
    /// paired), `AuthenticationCanceled` (the pairing was cancelled, by
    /// [`cancel_pairing`](Self::cancel_pairing) or the link dropping), and
    /// `InProgress` while BlueZ is already pairing the device (the pairing
    /// under way reports the outcome).
    pub fn pair(self: &Arc<Self>) {
        self.request(DeviceAction::Pair, self.method("Pair"));
    }

    /// Cancels a pairing operation initiated by the Pair method.
    ///
    /// # Failures
    ///
    /// Recorded in [`DeviceInfo::last_error`]; BlueZ may report:
    /// - `Failed` - Operation failed
    ///
    /// `DoesNotExist` (no pairing in progress, so nothing to cancel) isn't
    /// recorded.
    pub fn cancel_pairing(self: &Arc<Self>) {
        self.request(DeviceAction::CancelPairing, self.method("CancelPairing"));
    }

    /// Connects a specific profile of this device. The UUID provided is the remote
    /// service UUID for the profile.
    ///
    /// # Failures
    ///
    /// Recorded in [`DeviceInfo::last_error`]; BlueZ may report:
    /// - `Failed` - Operation failed
    /// - `InProgress` - Connection in progress
    /// - `InvalidArguments` - Invalid UUID
    /// - `NotAvailable` - Profile not available
    /// - `NotReady` - Adapter not ready
    pub fn connect_profile(self: &Arc<Self>, profile_uuid: UUID) {
        let call = Call::method_with_str(
            &self.object_path,
            DEVICE_INTERFACE,
            "ConnectProfile",
            profile_uuid,
        );
        self.request(DeviceAction::ConnectProfile, call);
    }

    /// Disconnects a specific profile of this device. The profile needs to be
    /// registered client profile.
    ///
    /// There is no connection tracking for a profile, so as long as the profile is
    /// registered this will always succeed.
    ///
    /// # Failures
    ///
    /// Recorded in [`DeviceInfo::last_error`]; BlueZ may report:
    /// - `Failed` - Operation failed
    /// - `InProgress` - Disconnection in progress
    /// - `InvalidArguments` - Invalid UUID
    /// - `NotSupported` - Profile not supported
    pub fn disconnect_profile(self: &Arc<Self>, profile_uuid: UUID) {
        let call = Call::method_with_str(
            &self.object_path,
            DEVICE_INTERFACE,
            "DisconnectProfile",
            profile_uuid,
        );
        self.request(DeviceAction::DisconnectProfile, call);
    }

    /// Sets whether the remote device is trusted.
    ///
    /// Trusted devices can connect without user authorization.
    ///
    /// A failure is recorded in [`DeviceInfo::last_error`].
    pub fn set_trusted(self: &Arc<Self>, trusted: bool) {
        self.request(DeviceAction::SetTrusted, self.property("Trusted", trusted));
    }

    /// Sets whether the remote device is blocked.
    ///
    /// Blocked devices will be automatically disconnected and further connections will be denied.
    ///
    /// A failure is recorded in [`DeviceInfo::last_error`].
    pub fn set_blocked(self: &Arc<Self>, blocked: bool) {
        self.request(DeviceAction::SetBlocked, self.property("Blocked", blocked));
    }

    /// Sets whether the device is allowed to wake up the host from system suspend.
    ///
    /// A failure is recorded in [`DeviceInfo::last_error`].
    pub fn set_wake_allowed(self: &Arc<Self>, wake_allowed: bool) {
        let call = self.property("WakeAllowed", wake_allowed);
        self.request(DeviceAction::SetWakeAllowed, call);
    }

    /// Sets a custom alias for the remote device.
    ///
    /// Setting an empty string will revert to the remote device's name.
    ///
    /// A failure is recorded in [`DeviceInfo::last_error`].
    pub fn set_alias(self: &Arc<Self>, alias: &str) {
        self.request(
            DeviceAction::SetAlias,
            self.property("Alias", alias.to_owned()),
        );
    }

    /// Sets the preferred bearer for dual-mode devices.
    ///
    /// Note: Changes only take effect when the device is disconnected.
    ///
    /// (BlueZ experimental)
    ///
    /// A failure is recorded in [`DeviceInfo::last_error`].
    pub fn set_preferred_bearer(self: &Arc<Self>, bearer: PreferredBearer) {
        let call = self.property("PreferredBearer", bearer.to_string());
        self.request(DeviceAction::SetPreferredBearer, call);
    }

    /// Removes this device from the adapter and forgets all stored information.
    ///
    /// This will remove the device from the adapter's device list and delete all
    /// pairing/bonding information. The device will need to be rediscovered and
    /// re-paired to connect again.
    ///
    /// # Failures
    ///
    /// Recorded in [`DeviceInfo::last_error`]; BlueZ may report:
    /// - `InvalidArguments` - Invalid device path
    /// - `DoesNotExist` - Device does not exist
    /// - `Failed` - Operation failed
    pub fn forget(self: &Arc<Self>) {
        // BlueZ removes devices through the adapter that owns them.
        let adapter = self.info.get().adapter.clone();
        let call = Call::method_with_path(
            &adapter,
            ADAPTER_INTERFACE,
            "RemoveDevice",
            &self.object_path,
        );
        self.request(DeviceAction::Forget, call);
    }

    /// Stops reporting the most recent failure: clears
    /// [`DeviceInfo::last_error`] for every consumer. BlueZ is not involved;
    /// the device's own state is unchanged.
    pub fn dismiss_error(self: &Arc<Self>) {
        self.commands
            .send(Command::DismissDeviceError(Arc::clone(self)));
    }

    /// Returns all currently known BR/EDR service records for the device. Each
    /// individual byte array represents a raw SDP record, as defined by the Bluetooth
    /// Service Discovery Protocol specification.
    ///
    /// Intended for compatibility layers like Wine that need raw SDP records
    /// for foreign Bluetooth APIs.
    ///
    /// General applications should instead use the Profile API for services-related
    /// functionality.
    ///
    /// (BlueZ experimental)
    ///
    /// # Errors
    ///
    /// BlueZ may report:
    /// - `Failed` - Operation failed
    /// - `NotReady` - Adapter not ready
    /// - `NotConnected` - Device not connected
    /// - `DoesNotExist` - No service records
    ///
    /// [`Error::ServiceStopped`] once the service is gone.
    pub async fn get_service_records(&self) -> Result<Vec<Vec<u8>>, Error> {
        let device = self.object_path.clone();
        self.commands
            .query(|reply| Query::ServiceRecords { device, reply })
            .await
    }

    /// Builds a device from the property maps of its `Device1` and (if
    /// present) `Battery1` interfaces.
    pub(crate) fn new(
        commands: &Commands,
        object_path: OwnedObjectPath,
        device1: PropertyMap,
        battery1: Option<PropertyMap>,
    ) -> Self {
        let mut info = DeviceInfo::empty();
        let mut signal = DeviceSignal::empty();
        let (signal_changes, info_changes) = split_signal(device1);
        info.apply_device1(info_changes, &[]);
        signal.apply(signal_changes, &[]);
        if let Some(battery1) = battery1 {
            info.apply_battery1(battery1, &[]);
        }

        Self {
            commands: commands.clone(),
            object_path,
            info: Property::new(Arc::new(info)),
            signal: Property::new(Arc::new(signal)),
        }
    }

    /// The request disconnecting this device (also sent by the dispatcher
    /// when a pairing with it is turned down).
    pub(crate) fn disconnect_request(&self) -> (DeviceAction, Call) {
        (DeviceAction::Disconnect, self.method("Disconnect"))
    }

    /// Queues `call`, the request for `action` on this instance.
    fn request(self: &Arc<Self>, action: DeviceAction, call: Call) {
        self.commands.send(Command::Device {
            device: Arc::clone(self),
            action,
            call,
        });
    }

    /// A call to the `org.bluez.Device1` method `member`, without arguments.
    fn method(&self, member: &'static str) -> Call {
        Call::method(&self.object_path, DEVICE_INTERFACE, member)
    }

    /// Setting the `org.bluez.Device1` property `name`.
    fn property(
        &self,
        name: &'static str,
        value: impl Into<zbus::zvariant::Value<'static>>,
    ) -> Call {
        Call::set_property(&self.object_path, DEVICE_INTERFACE, name, value)
    }
}

/// Whether BlueZ now reports the outcome a failed `action` was after, which
/// makes its error stale (e.g. the device connected through another client
/// after our connect failed). Changes that merely follow the failure (a
/// connect bringing the link up and dropping it again) don't count.
pub(crate) fn outcome_reported(action: DeviceAction, old: &DeviceInfo, new: &DeviceInfo) -> bool {
    match action {
        DeviceAction::Connect | DeviceAction::ConnectProfile => new.connected && !old.connected,
        DeviceAction::Disconnect | DeviceAction::DisconnectProfile => {
            !new.connected && old.connected
        }
        DeviceAction::Pair | DeviceAction::CancelPairing => new.paired != old.paired,
        DeviceAction::SetTrusted => new.trusted != old.trusted,
        DeviceAction::SetBlocked => new.blocked != old.blocked,
        DeviceAction::SetWakeAllowed => new.wake_allowed != old.wake_allowed,
        DeviceAction::SetAlias => new.alias != old.alias,
        DeviceAction::SetPreferredBearer => new.preferred_bearer != old.preferred_bearer,
        // A forgotten device is removed, error and all.
        DeviceAction::Forget => false,
    }
}

/// Whether BlueZ reports the outcome `activity` is working towards.
pub(crate) fn reached(info: &DeviceInfo, activity: DeviceActivity) -> bool {
    match activity {
        DeviceActivity::Idle => true,
        DeviceActivity::Connecting => info.connected,
        DeviceActivity::Disconnecting => !info.connected,
        DeviceActivity::Pairing => info.paired,
        DeviceActivity::Forgetting => false,
    }
}

pub(crate) fn is_signal_property(name: &str) -> bool {
    SIGNAL_PROPERTIES.contains(&name)
}

/// Splits `Device1` properties into those for [`DeviceSignal`] and the rest.
pub(crate) fn split_signal(changed: PropertyMap) -> (PropertyMap, PropertyMap) {
    changed
        .into_iter()
        .partition(|(name, _)| is_signal_property(name))
}

impl DeviceInfo {
    pub(crate) fn empty() -> Self {
        Self {
            address: String::new(),
            address_type: AddressType::from(""),
            name: None,
            icon: None,
            battery_percentage: None,
            class: None,
            appearance: None,
            uuids: None,
            paired: false,
            bonded: false,
            connected: false,
            trusted: false,
            blocked: false,
            wake_allowed: false,
            alias: String::new(),
            adapter: OwnedObjectPath::default(),
            legacy_pairing: false,
            cable_pairing: false,
            modalias: None,
            services_resolved: false,
            sets: Vec::new(),
            preferred_bearer: None,
            activity: DeviceActivity::Idle,
            last_error: None,
        }
    }

    /// Applies changed and invalidated `org.bluez.Device1` properties (other
    /// than [`SIGNAL_PROPERTIES`]).
    ///
    /// Properties BlueZ stops exporting (reported as invalidated) are cleared:
    /// optional ones become `None`, `WakeAllowed` false and `Sets` empty. The
    /// others keep their last value.
    pub(crate) fn apply_device1(&mut self, changed: PropertyMap, invalidated: &[String]) {
        for (name, value) in changed {
            match name.as_str() {
                "Address" => props::assign(&mut self.address, &name, value),
                "AddressType" => {
                    props::assign_with(&mut self.address_type, &name, value, |raw: String| {
                        AddressType::from(raw.as_str())
                    })
                }
                "Name" => props::assign_some(&mut self.name, &name, value),
                "Icon" => props::assign_some(&mut self.icon, &name, value),
                "Class" => props::assign_some(&mut self.class, &name, value),
                "Appearance" => props::assign_some(&mut self.appearance, &name, value),
                "UUIDs" => props::assign_some(&mut self.uuids, &name, value),
                "Paired" => props::assign(&mut self.paired, &name, value),
                "Bonded" => props::assign(&mut self.bonded, &name, value),
                "Connected" => props::assign(&mut self.connected, &name, value),
                "Trusted" => props::assign(&mut self.trusted, &name, value),
                "Blocked" => props::assign(&mut self.blocked, &name, value),
                "WakeAllowed" => props::assign(&mut self.wake_allowed, &name, value),
                "Alias" => props::assign(&mut self.alias, &name, value),
                "Adapter" => props::assign(&mut self.adapter, &name, value),
                "LegacyPairing" => props::assign(&mut self.legacy_pairing, &name, value),
                "CablePairing" => props::assign(&mut self.cable_pairing, &name, value),
                "Modalias" => {
                    props::assign_with(&mut self.modalias, &name, value, |raw: String| {
                        (!raw.is_empty()).then_some(raw)
                    })
                }
                "ServicesResolved" => props::assign(&mut self.services_resolved, &name, value),
                "Sets" => props::assign_with(
                    &mut self.sets,
                    &name,
                    value,
                    |raw: HashMap<OwnedObjectPath, PropertyMap>| {
                        let mut sets: Vec<DeviceSet> = raw
                            .into_iter()
                            .map(|(path, props)| DeviceSet::from_dbus(path, props))
                            .collect();
                        sets.sort_by(|left, right| left.path.as_str().cmp(right.path.as_str()));
                        sets
                    },
                ),
                "PreferredBearer" => {
                    props::assign_with(&mut self.preferred_bearer, &name, value, |raw: String| {
                        Some(PreferredBearer::from(raw.as_str()))
                    });
                }
                _ => {}
            }
        }

        for name in invalidated {
            match name.as_str() {
                "Name" => self.name = None,
                "Icon" => self.icon = None,
                "Class" => self.class = None,
                "Appearance" => self.appearance = None,
                "UUIDs" => self.uuids = None,
                "Modalias" => self.modalias = None,
                "PreferredBearer" => self.preferred_bearer = None,
                "WakeAllowed" => self.wake_allowed = false,
                "Sets" => self.sets.clear(),
                _ => {}
            }
        }
    }

    /// Applies changed and invalidated `org.bluez.Battery1` properties.
    pub(crate) fn apply_battery1(&mut self, changed: PropertyMap, invalidated: &[String]) {
        for (name, value) in changed {
            if name == "Percentage" {
                props::assign_some(&mut self.battery_percentage, &name, value);
            }
        }

        if invalidated.iter().any(|name| name == "Percentage") {
            self.battery_percentage = None;
        }
    }
}

impl DeviceSignal {
    pub(crate) fn empty() -> Self {
        Self {
            rssi: None,
            tx_power: None,
            manufacturer_data: None,
            service_data: None,
            advertising_flags: Vec::new(),
            advertising_data: AdvertisingData::new(),
        }
    }

    /// Applies changed and invalidated [`SIGNAL_PROPERTIES`]. Invalidated
    /// values are cleared: optional ones become `None` (e.g. `RSSI` once a
    /// device is out of range), the advertisement collections empty.
    pub(crate) fn apply(&mut self, changed: PropertyMap, invalidated: &[String]) {
        for (name, value) in changed {
            match name.as_str() {
                "RSSI" => props::assign_some(&mut self.rssi, &name, value),
                "TxPower" => props::assign_some(&mut self.tx_power, &name, value),
                "ManufacturerData" => props::assign_some(&mut self.manufacturer_data, &name, value),
                "ServiceData" => props::assign_some(&mut self.service_data, &name, value),
                "AdvertisingFlags" => props::assign(&mut self.advertising_flags, &name, value),
                "AdvertisingData" => props::assign(&mut self.advertising_data, &name, value),
                _ => {}
            }
        }

        for name in invalidated {
            match name.as_str() {
                "RSSI" => self.rssi = None,
                "TxPower" => self.tx_power = None,
                "ManufacturerData" => self.manufacturer_data = None,
                "ServiceData" => self.service_data = None,
                "AdvertisingFlags" => self.advertising_flags.clear(),
                "AdvertisingData" => self.advertising_data.clear(),
                _ => {}
            }
        }
    }
}
