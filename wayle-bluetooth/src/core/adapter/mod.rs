use std::{collections::HashMap, ptr, sync::Arc};

use derive_more::Debug;
use tracing::warn;
use wayle_core::Property;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::{
    dispatcher::{
        Call,
        command::{Command, Commands, Query},
    },
    error::Error,
    props::{self, PropertyMap},
    types::{
        ADAPTER_INTERFACE, UUID,
        adapter::{
            AdapterAction, AdapterError, AdapterRole, AddressType, DiscoveryFilter,
            DiscoveryFilterOptions, PowerState,
        },
    },
};

/// Bluetooth adapter from BlueZ.
///
/// Every `Adapter` handed out by [`BluetoothService`](crate::BluetoothService)
/// is **live**: its properties track BlueZ for as long as the adapter exists.
/// There is exactly one instance per adapter (look one up by path with
/// [`BluetoothService::adapter`](crate::BluetoothService::adapter)), shared as
/// an `Arc`, and adapters compare equal only if they are the same instance.
///
/// # Actions
///
/// Actions return nothing: each is queued to the service, which sends them to
/// BlueZ in call order, and its outcome shows up only as state, the same way a
/// change made by any other BlueZ client would. A failure is recorded in
/// [`AdapterInfo::last_error`]. An action acts on this instance: once the
/// adapter is removed (or replaced by a new instance), it does nothing.
///
/// - [`set_powered()`](Self::set_powered) - Power on/off
/// - [`set_discoverable()`](Self::set_discoverable) /
///   [`set_discoverable_timeout()`](Self::set_discoverable_timeout) - Visibility
/// - [`set_pairable()`](Self::set_pairable) /
///   [`set_pairable_timeout()`](Self::set_pairable_timeout) - Pairing acceptance
/// - [`start_discovery()`](Self::start_discovery) /
///   [`stop_discovery()`](Self::stop_discovery) - Device scanning
/// - [`set_discovery_filter()`](Self::set_discovery_filter) - Filter discovered devices
/// - [`connect_device()`](Self::connect_device) - Direct connection without discovery
#[derive(Debug)]
pub struct Adapter {
    /// Where actions and queries are queued.
    #[debug(skip)]
    commands: Commands,

    /// D-Bus object path for this adapter.
    pub object_path: OwnedObjectPath,

    /// The adapter's state (live).
    pub info: Property<Arc<AdapterInfo>>,
}

/// A BlueZ adapter's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterInfo {
    /// The Bluetooth device address.
    pub address: String,

    /// The Bluetooth Address Type. For dual-mode and BR/EDR only adapter this defaults
    /// to "public". Single mode LE adapters may have either value. With privacy enabled
    /// this contains type of Identity Address and not type of address used for
    /// connection.
    pub address_type: AddressType,

    /// The Bluetooth system name (pretty hostname).
    ///
    /// This property is either a static system default or controlled by an external
    /// daemon providing access to the pretty hostname configuration.
    pub name: String,

    /// The Bluetooth friendly name. This value can be changed.
    ///
    /// In case no alias is set, it will return the system provided name. Setting an
    /// empty string as alias will convert it back to the system provided name.
    ///
    /// When resetting the alias with an empty string, the property will default back to
    /// system name.
    ///
    /// On a well configured system, this property never needs to be changed since it
    /// defaults to the system name and provides the pretty hostname.
    ///
    /// Only if the local name needs to be different from the pretty hostname, this
    /// property should be used as last resort.
    pub alias: String,

    /// The Bluetooth class of device.
    ///
    /// This property represents the value that is either automatically configured by
    /// DMI/ACPI information or provided as static configuration.
    pub class: u32,

    /// Set an adapter to connectable or non-connectable. This is a global setting and
    /// should only be used by the settings application.
    ///
    /// Setting this property to false will set the Discoverable property of the adapter
    /// to false as well, which will not be reverted if Connectable is set back to true.
    ///
    /// If required, the application will need to manually set Discoverable to true.
    ///
    /// Note that this property only affects incoming connections.
    pub connectable: bool,

    /// Switch an adapter on or off. This will also set the appropriate connectable
    /// state of the controller.
    ///
    /// The value of this property is not persistent. After restart or unplugging of the
    /// adapter it will reset back to false.
    pub powered: bool,

    /// The power state of an adapter.
    ///
    /// The power state will show whether the adapter is turning off, or turning on, as
    /// well as being on or off.
    ///
    /// (BlueZ experimental)
    pub power_state: PowerState,

    /// Switch an adapter to discoverable or non-discoverable to either make it visible
    /// or hide it. This is a global setting and should only be used by the settings
    /// application.
    ///
    /// If the DiscoverableTimeout is set to a non-zero value then the system will set
    /// this value back to false after the timer expired.
    ///
    /// In case the adapter is switched off, setting this value will fail.
    ///
    /// When changing the Powered property the new state of this property will be
    /// updated via a PropertiesChanged signal.
    ///
    /// Default: false
    pub discoverable: bool,

    /// The discoverable timeout in seconds. A value of zero means that the timeout is
    /// disabled and it will stay in discoverable/limited mode forever.
    ///
    /// Default: 180
    pub discoverable_timeout: u32,

    /// Indicates that a device discovery procedure is active.
    pub discovering: bool,

    /// Switch an adapter to pairable or non-pairable. This is a global setting and
    /// should only be used by the settings application.
    ///
    /// Note that this property only affects incoming pairing requests.
    ///
    /// Default: true
    pub pairable: bool,

    /// The pairable timeout in seconds. A value of zero means that the timeout is
    /// disabled and it will stay in pairable mode forever.
    ///
    /// Default: 0
    pub pairable_timeout: u32,

    /// List of 128-bit UUIDs that represents the available local services.
    pub uuids: Vec<UUID>,

    /// Local Device ID information in modalias format used by the kernel and udev.
    pub modalias: Option<String>,

    /// List of supported roles.
    pub roles: Vec<AdapterRole>,

    /// List of 128-bit UUIDs that represents the experimental features currently
    /// enabled.
    pub experimental_features: Vec<UUID>,

    /// The manufacturer of the device, as a uint16 company identifier defined by the
    /// Core Bluetooth Specification.
    pub manufacturer: u16,

    /// The Bluetooth version supported by the device, as a core version code defined by
    /// the Core Bluetooth Specification.
    pub version: u8,

    /// The most recent action on this adapter that failed, with the complete
    /// error. Cleared when BlueZ reports the outcome that action was after
    /// (e.g. `Powered` changing, by any client, after a failed power change;
    /// not the `PowerState` transitions BlueZ reverts when a request fails),
    /// when the same action later succeeds, or by [`Adapter::dismiss_error`].
    pub last_error: Option<AdapterError>,
}

/// An adapter is equal only to itself: there is one instance per BlueZ adapter
/// object, so a replaced instance (e.g. after bluetoothd restarts) is a change.
impl PartialEq for Adapter {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self, other)
    }
}

impl Eq for Adapter {}

impl Adapter {
    /// Sets the Bluetooth friendly name (alias) of the adapter.
    ///
    /// Setting an empty string will revert to the system-provided name.
    ///
    /// A failure is recorded in [`AdapterInfo::last_error`].
    pub fn set_alias(self: &Arc<Self>, alias: &str) {
        self.request(
            AdapterAction::SetAlias,
            self.property("Alias", alias.to_owned()),
        );
    }

    /// Sets whether the adapter is connectable.
    ///
    /// Note: Setting this to false will also set Discoverable to false.
    ///
    /// A failure is recorded in [`AdapterInfo::last_error`].
    pub fn set_connectable(self: &Arc<Self>, connectable: bool) {
        let call = self.property("Connectable", connectable);
        self.request(AdapterAction::SetConnectable, call);
    }

    /// Powers the adapter on or off.
    ///
    /// This will also set the appropriate connectable state of the controller.
    ///
    /// A failure is recorded in [`AdapterInfo::last_error`].
    pub fn set_powered(self: &Arc<Self>, powered: bool) {
        let (action, call) = self.power_request(powered);
        self.request(action, call);
    }

    /// Sets whether the adapter is discoverable.
    ///
    /// This is a global setting and should only be used by a settings application.
    ///
    /// A failure is recorded in [`AdapterInfo::last_error`].
    pub fn set_discoverable(self: &Arc<Self>, discoverable: bool) {
        let call = self.property("Discoverable", discoverable);
        self.request(AdapterAction::SetDiscoverable, call);
    }

    /// Sets the discoverable timeout in seconds.
    ///
    /// A value of 0 means that the timeout is disabled and the adapter will stay in discoverable mode indefinitely.
    ///
    /// A failure is recorded in [`AdapterInfo::last_error`].
    pub fn set_discoverable_timeout(self: &Arc<Self>, timeout: u32) {
        let call = self.property("DiscoverableTimeout", timeout);
        self.request(AdapterAction::SetDiscoverableTimeout, call);
    }

    /// Sets whether the adapter is pairable.
    ///
    /// This is a global setting and should only be used by a settings application.
    ///
    /// A failure is recorded in [`AdapterInfo::last_error`].
    pub fn set_pairable(self: &Arc<Self>, pairable: bool) {
        self.request(
            AdapterAction::SetPairable,
            self.property("Pairable", pairable),
        );
    }

    /// Sets the pairable timeout in seconds.
    ///
    /// A value of 0 means that the timeout is disabled and the adapter will stay in pairable mode indefinitely.
    ///
    /// A failure is recorded in [`AdapterInfo::last_error`].
    pub fn set_pairable_timeout(self: &Arc<Self>, timeout: u32) {
        let call = self.property("PairableTimeout", timeout);
        self.request(AdapterAction::SetPairableTimeout, call);
    }

    /// Sets the device discovery filter for the caller. When this method is called with
    /// no filter parameter, filter is removed.
    ///
    /// When discovery filter is set, Device objects will be created as new devices with
    /// matching criteria are discovered regardless of they are connectable or
    /// discoverable which enables listening to non-connectable and non-discoverable
    /// devices.
    ///
    /// When multiple clients call SetDiscoveryFilter, their filters are internally
    /// merged, and notifications about new devices are sent to all clients. Therefore,
    /// each client must check that device updates actually match its filter.
    ///
    /// When SetDiscoveryFilter is called multiple times by the same client, last filter
    /// passed will be active for given client.
    ///
    /// SetDiscoveryFilter can be called before StartDiscovery.
    /// It is useful when client will create first discovery session, to ensure that
    /// proper scan will be started right after call to StartDiscovery.
    ///
    /// A failure is recorded in [`AdapterInfo::last_error`].
    pub fn set_discovery_filter(self: &Arc<Self>, options: DiscoveryFilterOptions<'_>) {
        let filter = options
            .to_filter()
            .into_iter()
            .map(|(key, value)| value.try_to_owned().map(|value| (key, Value::from(value))))
            .collect::<Result<DiscoveryFilter<'static>, _>>();

        match filter {
            Ok(filter) => {
                let call = Call::method_with_dict(
                    &self.object_path,
                    ADAPTER_INTERFACE,
                    "SetDiscoveryFilter",
                    filter,
                );
                self.request(AdapterAction::SetDiscoveryFilter, call);
            }
            Err(err) => {
                warn!(adapter = %self.object_path, error = %err, "invalid discovery filter")
            }
        }
    }

    /// Starts device discovery session which may include starting an inquiry and/or
    /// scanning procedures and remote device name resolving.
    ///
    /// This process will start creating Device objects as new devices are discovered.
    /// Each client can request a single device discovery session per adapter,
    /// shared with this service's own [`start_discovery`](crate::BluetoothService::start_discovery)
    /// (whose timeout, if any, this cancels). It lasts until stopped, or until
    /// the adapter powers off.
    ///
    /// Nothing is sent while this client's session is already running, as far
    /// as BlueZ's answers go. A BlueZ bug can end the discovery early (see
    /// [`BluetoothService::start_discovery`](crate::BluetoothService::start_discovery));
    /// it can then be started again only after
    /// [`stop_discovery`](Self::stop_discovery).
    ///
    /// # Failures
    ///
    /// Recorded in [`AdapterInfo::last_error`]; BlueZ may report:
    /// - `NotReady` - Adapter not ready
    /// - `InProgress` - The controller refused to start discovering
    /// - `Failed` - Operation failed
    pub fn start_discovery(self: &Arc<Self>) {
        self.commands.send(Command::Discover {
            adapter: Arc::clone(self),
            start: true,
        });
    }

    /// Stops this client's device discovery session on the adapter.
    ///
    /// Note that a discovery procedure is shared between all discovery sessions thus
    /// calling stop_discovery will only release a single session and discovery will stop
    /// when all sessions from all clients have finished.
    ///
    /// Stopping is not recorded as a failure if it is refused: BlueZ ends a
    /// client's session by itself (when the adapter powers off, and even when
    /// the controller refuses to stop), and a client without a session has
    /// nothing to stop.
    pub fn stop_discovery(self: &Arc<Self>) {
        self.commands.send(Command::Discover {
            adapter: Arc::clone(self),
            start: false,
        });
    }

    /// Stops reporting the most recent failure: clears
    /// [`AdapterInfo::last_error`] for every consumer. BlueZ is not involved;
    /// the adapter's own state is unchanged.
    pub fn dismiss_error(self: &Arc<Self>) {
        self.commands
            .send(Command::DismissAdapterError(Arc::clone(self)));
    }

    /// Returns available filters that can be given to set_discovery_filter.
    ///
    /// # Errors
    ///
    /// Returns error if the D-Bus operation fails or the adapter is not
    /// available, or [`Error::ServiceStopped`] once the service is gone.
    pub async fn get_discovery_filters(&self) -> Result<Vec<String>, Error> {
        let adapter = self.object_path.clone();
        self.commands
            .query(|reply| Query::DiscoveryFilters { adapter, reply })
            .await
    }

    /// Connects to device without need of performing General Discovery.
    ///
    /// Connection mechanism is similar to Device connect method with exception that this
    /// method returns success when physical connection is established and you can specify
    /// bearer to connect with parameter.
    ///
    /// After this method returns, services discovery will continue and any supported
    /// profile will be connected. Returns object path to created device object or device that already exists.
    ///
    /// (BlueZ experimental)
    ///
    /// # Errors
    ///
    /// BlueZ may report:
    /// - `InvalidArguments` - Invalid properties
    /// - `AlreadyExists` - Device already exists
    /// - `NotSupported` - Not supported
    /// - `NotReady` - Adapter not ready
    /// - `Failed` - Operation failed
    ///
    /// [`Error::ServiceStopped`] once the service is gone.
    pub async fn connect_device(
        &self,
        properties: HashMap<String, Value<'_>>,
    ) -> Result<OwnedObjectPath, Error> {
        let properties = properties
            .into_iter()
            .map(|(key, value)| value.try_to_owned().map(|value| (key, value)))
            .collect::<Result<HashMap<String, OwnedValue>, _>>()
            .map_err(zbus::Error::from)?;
        let adapter = self.object_path.clone();

        self.commands
            .query(|reply| Query::ConnectDevice {
                adapter,
                properties,
                reply,
            })
            .await
    }

    /// Builds an adapter from the property map of its `Adapter1` interface.
    pub(crate) fn new(
        commands: &Commands,
        object_path: OwnedObjectPath,
        adapter1: PropertyMap,
    ) -> Self {
        let mut info = AdapterInfo::empty();
        info.apply_adapter1(adapter1, &[]);

        Self {
            commands: commands.clone(),
            object_path,
            info: Property::new(Arc::new(info)),
        }
    }

    /// The request powering this adapter on or off (also sent by the
    /// dispatcher for [`BluetoothService::enable`](crate::BluetoothService::enable)
    /// and [`disable`](crate::BluetoothService::disable)).
    pub(crate) fn power_request(&self, powered: bool) -> (AdapterAction, Call) {
        (AdapterAction::SetPowered, self.property("Powered", powered))
    }

    /// Queues `call`, the request for `action` on this instance.
    fn request(self: &Arc<Self>, action: AdapterAction, call: Call) {
        self.commands.send(Command::Adapter {
            adapter: Arc::clone(self),
            action,
            call,
        });
    }

    /// Setting the `org.bluez.Adapter1` property `name`.
    fn property(&self, name: &'static str, value: impl Into<Value<'static>>) -> Call {
        Call::set_property(&self.object_path, ADAPTER_INTERFACE, name, value)
    }
}

/// Whether BlueZ now reports the outcome a failed `action` was after, which
/// makes its error stale.
pub(crate) fn outcome_reported(
    action: AdapterAction,
    old: &AdapterInfo,
    new: &AdapterInfo,
) -> bool {
    match action {
        AdapterAction::SetPowered => new.powered != old.powered,
        AdapterAction::StartDiscovery => new.discovering && !old.discovering,
        AdapterAction::SetAlias => new.alias != old.alias,
        AdapterAction::SetConnectable => new.connectable != old.connectable,
        AdapterAction::SetDiscoverable => new.discoverable != old.discoverable,
        AdapterAction::SetDiscoverableTimeout => {
            new.discoverable_timeout != old.discoverable_timeout
        }
        AdapterAction::SetPairable => new.pairable != old.pairable,
        AdapterAction::SetPairableTimeout => new.pairable_timeout != old.pairable_timeout,
        AdapterAction::SetDiscoveryFilter => false,
    }
}

impl AdapterInfo {
    pub(crate) fn empty() -> Self {
        Self {
            address: String::new(),
            address_type: AddressType::from(""),
            name: String::new(),
            alias: String::new(),
            class: 0,
            connectable: false,
            powered: false,
            power_state: PowerState::from(""),
            discoverable: false,
            discoverable_timeout: 0,
            discovering: false,
            pairable: false,
            pairable_timeout: 0,
            uuids: Vec::new(),
            modalias: None,
            roles: Vec::new(),
            experimental_features: Vec::new(),
            manufacturer: 0,
            version: 0,
            last_error: None,
        }
    }

    /// Whether BlueZ is keeping this adapter on, or turning it on.
    ///
    /// BlueZ moves `PowerState` to `off-enabling` / `on-disabling` as soon as it
    /// accepts a power change from any client, and back if the change fails,
    /// so this follows a request immediately without assuming its outcome. In
    /// the stable states (and on BlueZ versions without `PowerState`) it is
    /// `Powered`.
    pub(crate) fn power_target(&self) -> bool {
        match self.power_state {
            PowerState::OffToOn => true,
            PowerState::OnToOff => false,
            PowerState::On | PowerState::Off | PowerState::OffBlocked => self.powered,
        }
    }

    /// Whether the adapter is on and staying on. BlueZ connects devices and
    /// discovers only then: it refuses both as soon as it accepts a power-off
    /// (`PowerState` `on-disabling`), while `Powered` is still true.
    pub(crate) fn usable(&self) -> bool {
        self.powered && self.power_state != PowerState::OnToOff
    }

    /// Applies changed and invalidated `org.bluez.Adapter1` properties.
    pub(crate) fn apply_adapter1(&mut self, changed: PropertyMap, invalidated: &[String]) {
        for (name, value) in changed {
            match name.as_str() {
                "Address" => props::assign(&mut self.address, &name, value),
                "AddressType" => {
                    props::assign_with(&mut self.address_type, &name, value, |raw: String| {
                        AddressType::from(raw.as_str())
                    })
                }
                "Name" => props::assign(&mut self.name, &name, value),
                "Alias" => props::assign(&mut self.alias, &name, value),
                "Class" => props::assign(&mut self.class, &name, value),
                "Connectable" => props::assign(&mut self.connectable, &name, value),
                "Powered" => props::assign(&mut self.powered, &name, value),
                "PowerState" => {
                    props::assign_with(&mut self.power_state, &name, value, |raw: String| {
                        PowerState::from(raw.as_str())
                    })
                }
                "Discoverable" => props::assign(&mut self.discoverable, &name, value),
                "DiscoverableTimeout" => {
                    props::assign(&mut self.discoverable_timeout, &name, value)
                }
                "Discovering" => props::assign(&mut self.discovering, &name, value),
                "Pairable" => props::assign(&mut self.pairable, &name, value),
                "PairableTimeout" => props::assign(&mut self.pairable_timeout, &name, value),
                "UUIDs" => props::assign(&mut self.uuids, &name, value),
                "Modalias" => {
                    props::assign_with(&mut self.modalias, &name, value, |raw: String| {
                        (!raw.is_empty()).then_some(raw)
                    })
                }
                "Roles" => props::assign_with(&mut self.roles, &name, value, |raw: Vec<String>| {
                    raw.iter()
                        .map(|role| AdapterRole::from(role.as_str()))
                        .collect()
                }),
                "ExperimentalFeatures" => {
                    props::assign(&mut self.experimental_features, &name, value)
                }
                "Manufacturer" => props::assign(&mut self.manufacturer, &name, value),
                "Version" => props::assign(&mut self.version, &name, value),
                _ => {}
            }
        }

        // Properties BlueZ stops exporting are reported as invalidated.
        for name in invalidated {
            match name.as_str() {
                "Modalias" => self.modalias = None,
                "ExperimentalFeatures" => self.experimental_features.clear(),
                _ => {}
            }
        }
    }
}
