/// BlueZ's error for a request it is already doing, or busy with something
/// else for: a second `Connect` or `Pair`, a `StartDiscovery` from a client
/// already discovering, or a start or stop of discovery while the client's
/// previous one is still being carried out.
pub(crate) const BLUEZ_IN_PROGRESS: &str = "org.bluez.Error.InProgress";

/// Bluetooth service errors.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// D-Bus communication error.
    #[error("dbus error: {0}")]
    Dbus(#[from] zbus::Error),

    /// Service initialization failed.
    #[error("cannot initialize bluetooth service")]
    ServiceInitialization(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Agent registration failed.
    #[error("cannot register bluetooth agent")]
    AgentRegistration(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Enumerating BlueZ's objects failed.
    #[error("cannot enumerate bluetooth objects")]
    Discovery(#[source] zbus::fdo::Error),

    /// The [`BluetoothService`](crate::BluetoothService) this device or
    /// adapter belongs to has been dropped.
    #[error("bluetooth service has stopped")]
    ServiceStopped,

    /// Setting or lifting the rfkill block on Bluetooth's radios failed.
    #[error("cannot change the rfkill block on bluetooth")]
    Rfkill(#[source] std::io::Error),
}

impl Error {
    /// The D-Bus error name and message, if this is an error reply.
    pub fn bluez_error(&self) -> Option<(&str, Option<&str>)> {
        let Self::Dbus(zbus::Error::MethodError(name, message, _)) = self else {
            return None;
        };
        Some((name.as_str(), message.as_deref()))
    }

    /// Whether this is the BlueZ D-Bus error `name` (e.g.
    /// `org.bluez.Error.InProgress`).
    pub(crate) fn is_bluez_error(&self, name: &str) -> bool {
        self.bluez_error().is_some_and(|(error, _)| error == name)
    }
}
