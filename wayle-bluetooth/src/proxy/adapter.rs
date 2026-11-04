#![allow(missing_docs)]
use std::collections::HashMap;

use zbus::{
    Result, proxy,
    zvariant::{OwnedObjectPath, Value},
};

/// The `Adapter1` queries the service makes. (Actions and state go through
/// the dispatcher, which sends its own requests and follows BlueZ's signals.)
#[proxy(interface = "org.bluez.Adapter1", default_service = "org.bluez")]
pub(crate) trait Adapter1 {
    async fn get_discovery_filters(&self) -> Result<Vec<String>>;

    async fn connect_device(
        &self,
        properties: HashMap<String, Value<'_>>,
    ) -> Result<OwnedObjectPath>;
}
