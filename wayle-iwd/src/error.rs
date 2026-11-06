/// IWD's error for a connection that failed, most often a rejected passphrase
/// (IWD has no dedicated error for that).
pub(crate) const IWD_FAILED: &str = "net.connman.iwd.Failed";

/// IWD's error for a passphrase it can't use (e.g. too short for WPA).
pub(crate) const IWD_INVALID_FORMAT: &str = "net.connman.iwd.InvalidFormat";

/// IWD's error for a request cancelled before it completed: a connect
/// superseded by another, interrupted by a disconnect, or whose passphrase
/// request was turned down.
pub(crate) const IWD_ABORTED: &str = "net.connman.iwd.Aborted";

/// IWD's error for a request it is too busy to take (e.g. a scan while one is
/// under way).
pub(crate) const IWD_BUSY: &str = "net.connman.iwd.Busy";

/// IWD's error for a disconnect with nothing to disconnect.
pub(crate) const IWD_NOT_CONNECTED: &str = "net.connman.iwd.NotConnected";

/// IWD's error for powering on an adapter a hardware switch (or the firmware)
/// blocks.
pub(crate) const IWD_NOT_AVAILABLE: &str = "net.connman.iwd.NotAvailable";

/// IWD's error for registering something twice.
pub(crate) const IWD_ALREADY_EXISTS: &str = "net.connman.iwd.AlreadyExists";

/// IWD service errors.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// D-Bus communication error, including IWD's error replies.
    #[error("dbus error: {0}")]
    Dbus(#[from] zbus::Error),

    /// Service initialization failed.
    #[error("cannot initialize iwd service")]
    ServiceInitialization(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Enumerating IWD's objects failed.
    #[error("cannot enumerate iwd objects")]
    Enumeration(#[source] zbus::fdo::Error),
}

impl Error {
    /// The D-Bus error name and message, if this is an error reply (e.g.
    /// `net.connman.iwd.Failed`).
    pub fn iwd_error(&self) -> Option<(&str, Option<&str>)> {
        let Self::Dbus(zbus::Error::MethodError(name, message, _)) = self else {
            return None;
        };
        Some((name.as_str(), message.as_deref()))
    }

    /// Whether this is the IWD D-Bus error `name`.
    pub(crate) fn is_iwd_error(&self, name: &str) -> bool {
        self.iwd_error().is_some_and(|(error, _)| error == name)
    }
}
