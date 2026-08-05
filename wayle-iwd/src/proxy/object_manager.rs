//! `org.freedesktop.DBus.ObjectManager` interface scoped to IWD.

use std::collections::HashMap;

use zbus::{
    proxy,
    zvariant::{OwnedObjectPath, OwnedValue},
};

/// Map of object path -> interface name -> property name -> value.
pub(crate) type ManagedObjects =
    HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

#[proxy(
    default_service = "net.connman.iwd",
    interface = "org.freedesktop.DBus.ObjectManager",
    default_path = "/"
)]
pub(crate) trait ObjectManager {
    /// Returns all objects managed by IWD with their interfaces and properties.
    fn get_managed_objects(&self) -> zbus::Result<ManagedObjects>;

    /// Emitted when an object is added or gains interfaces.
    #[zbus(signal)]
    fn interfaces_added(
        &self,
        object_path: OwnedObjectPath,
        interfaces: HashMap<String, HashMap<String, OwnedValue>>,
    ) -> zbus::Result<()>;

    /// Emitted when an object is removed or loses interfaces.
    #[zbus(signal)]
    fn interfaces_removed(
        &self,
        object_path: OwnedObjectPath,
        interfaces: Vec<String>,
    ) -> zbus::Result<()>;
}
