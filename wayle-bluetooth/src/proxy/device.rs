#![allow(missing_docs)]

use zbus::{Result, proxy};

/// The `Device1` queries the service makes. (Actions and state go through the
/// dispatcher, which sends its own requests and follows BlueZ's signals.)
#[proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
pub(crate) trait Device1 {
    async fn get_service_records(&self) -> Result<Vec<Vec<u8>>>;
}
