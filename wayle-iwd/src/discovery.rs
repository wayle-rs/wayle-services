//! Discovery of the IWD WiFi device via `org.freedesktop.DBus.ObjectManager`.

use zbus::{Connection, zvariant::OwnedObjectPath};

use crate::{
    error::Error,
    proxy::object_manager::{ManagedObjects, ObjectManagerProxy},
};

/// D-Bus interface implemented by WiFi devices. The `Device` interface persists
/// across power toggling, whereas `Station`/`StationDiagnostic` only exist while
/// the device is powered on — so device presence is keyed on this interface.
pub(crate) const DEVICE_INTERFACE: &str = "net.connman.iwd.Device";

/// D-Bus interface carrying station (connection/scan) state. Present only while
/// the device is powered on; it is added/removed as the device is toggled, which
/// is how station state subscriptions are (re)established (mirroring iwgtk).
pub(crate) const STATION_INTERFACE: &str = "net.connman.iwd.Station";

/// D-Bus interface implemented by visible networks.
pub(crate) const NETWORK_INTERFACE: &str = "net.connman.iwd.Network";

pub(crate) struct IwdDiscovery;

impl IwdDiscovery {
    /// Returns the object path of the first WiFi device, or `None` if no device
    /// is present.
    pub(crate) async fn device_path(
        connection: &Connection,
    ) -> Result<Option<OwnedObjectPath>, Error> {
        let proxy = ObjectManagerProxy::new(connection).await?;
        let objects = proxy.get_managed_objects().await.map_err(Error::DbusError)?;
        Ok(find_device(&objects))
    }
}

fn find_device(objects: &ManagedObjects) -> Option<OwnedObjectPath> {
    objects.iter().find_map(|(path, interfaces)| {
        interfaces
            .contains_key(DEVICE_INTERFACE)
            .then(|| path.clone())
    })
}
