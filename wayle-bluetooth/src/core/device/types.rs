use std::collections::HashMap;

use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// Manufacturer-specific advertisement data keyed by company ID.
pub type ManufacturerData = HashMap<u16, Vec<u8>>;
/// Advertisement data keyed by AD type.
pub type AdvertisingData = HashMap<u8, Vec<u8>>;
/// Service-specific advertisement data keyed by UUID.
pub type ServiceData = HashMap<String, Vec<u8>>;
/// Device set membership from the `Device.Sets` property.
///
/// For full set properties (Adapter, Devices, Size), query
/// `org.bluez.DeviceSet1` at `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSet {
    /// Object path of the device set.
    pub path: OwnedObjectPath,
    /// Rank of this device within the set.
    pub rank: Option<u8>,
}

impl DeviceSet {
    pub(crate) fn from_dbus(path: OwnedObjectPath, props: HashMap<String, OwnedValue>) -> Self {
        let rank = props.get("Rank").and_then(|value| u8::try_from(value).ok());

        Self { path, rank }
    }
}
