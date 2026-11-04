use std::sync::Arc;

/// Bluetooth adapter type definitions
pub mod adapter;
/// Bluetooth agent type definitions
pub mod agent;
/// Bluetooth device type definitions
pub mod device;

pub(crate) const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";
pub(crate) const DEVICE_INTERFACE: &str = "org.bluez.Device1";
pub(crate) const BATTERY_INTERFACE: &str = "org.bluez.Battery1";
pub(crate) const BLUEZ_SERVICE: &str = "org.bluez";
pub(crate) const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
pub(crate) const OBJECT_MANAGER_INTERFACE: &str = "org.freedesktop.DBus.ObjectManager";

/// A request to BlueZ that failed.
///
/// BlueZ reports a failed request only in its error reply, never as a state
/// change, so the service records the most recent one on the device or adapter
/// it concerned (`last_error`), where every consumer sees it. It is cleared
/// once BlueZ reports the outcome that request was after, by another tracked
/// request starting, or by `dismiss_error()`.
#[derive(Debug, Clone)]
pub struct ActionError<A> {
    /// The request that failed.
    pub action: A,
    /// The complete error, e.g. a [`zbus::Error::MethodError`] carrying
    /// BlueZ's error name, message and reply.
    pub error: Arc<crate::Error>,
}

impl<A> ActionError<A> {
    pub(crate) fn new(action: A, error: crate::Error) -> Self {
        Self {
            action,
            error: Arc::new(error),
        }
    }

    /// The D-Bus error name (e.g. `org.bluez.Error.AuthenticationFailed`),
    /// if BlueZ replied with an error.
    pub fn name(&self) -> Option<&str> {
        self.error.bluez_error().map(|(name, _)| name)
    }

    /// BlueZ's description of the failure (e.g. `br-connection-page-timeout`),
    /// if it gave one.
    pub fn message(&self) -> Option<&str> {
        self.error.bluez_error().and_then(|(_, message)| message)
    }
}

/// Two errors are equal only if they are the same recorded failure, so a new
/// failure is always seen as a change.
impl<A: PartialEq> PartialEq for ActionError<A> {
    fn eq(&self, other: &Self) -> bool {
        self.action == other.action && Arc::ptr_eq(&self.error, &other.error)
    }
}

impl<A: Eq> Eq for ActionError<A> {}

/// Whether the kernel's radio kill switches (rfkill) block Bluetooth.
///
/// Every Bluetooth radio has a switch, which software can block (as
/// [`disable`](crate::BluetoothService::disable) does) and hardware can block
/// too (a switch, or the firmware). A blocked radio stays off, whatever BlueZ
/// is asked, until it is unblocked. As in KDE, Bluetooth counts as blocked
/// while any of its radios is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RadioBlock {
    /// No Bluetooth radio is blocked, or there is no switch to block one.
    #[default]
    None,
    /// A Bluetooth radio is blocked in software, and none by hardware:
    /// [`enable`](crate::BluetoothService::enable) lifts the block.
    Software,
    /// A Bluetooth radio is blocked by hardware, which software can't lift.
    Hardware,
}

/// Bluetooth UUID represented as a string.
#[allow(clippy::upper_case_acronyms)]
pub type UUID = String;
