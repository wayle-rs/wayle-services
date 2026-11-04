use zbus::{Connection, proxy::CacheProperties, zvariant::OwnedObjectPath};

/// Bluetooth adapter proxy
pub mod adapter;
/// Bluetooth agent manager proxy
pub mod agent_manager;
/// Bluetooth device proxy
pub mod device;

use adapter::Adapter1Proxy;
use device::Device1Proxy;

/// A `Device1` proxy for queries (actions go through the dispatcher).
///
/// Property state is tracked centrally by the dispatcher, so these proxies
/// skip zbus's property cache (which would otherwise cost an `AddMatch` and a
/// `GetAll` per call).
pub(crate) async fn device1(
    connection: &Connection,
    path: &OwnedObjectPath,
) -> zbus::Result<Device1Proxy<'static>> {
    Device1Proxy::builder(connection)
        .path(path.clone())?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

/// An `Adapter1` proxy for queries. See [`device1`].
pub(crate) async fn adapter1(
    connection: &Connection,
    path: &OwnedObjectPath,
) -> zbus::Result<Adapter1Proxy<'static>> {
    Adapter1Proxy::builder(connection)
        .path(path.clone())?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}
