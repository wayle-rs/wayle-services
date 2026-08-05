//! IWD StationDiagnostic interface (`net.connman.iwd.StationDiagnostic`).

use std::collections::HashMap;

use zbus::{proxy, zvariant::OwnedValue};

/// Diagnostics dictionary returned by `GetDiagnostics`.
///
/// Common keys include `RSSI` (i16 dBm), `Frequency` (u32 MHz),
/// `RxBitrate`/`TxBitrate`, and `ConnectedBss`.
pub(crate) type Diagnostics = HashMap<String, OwnedValue>;

#[proxy(
    default_service = "net.connman.iwd",
    interface = "net.connman.iwd.StationDiagnostic"
)]
pub(crate) trait StationDiagnostic {
    /// Returns diagnostic information about the current connection.
    ///
    /// May require elevated privileges; callers should treat failure as
    /// "diagnostics unavailable" rather than fatal.
    fn get_diagnostics(&self) -> zbus::Result<Diagnostics>;
}
