use thiserror::Error;

/// Errors produced while hosting an XDG Desktop Portal backend.
#[derive(Debug, Error)]
pub enum Error {
    /// The provided bus name is not a valid D-Bus well-known name.
    #[error("invalid portal bus name '{0}'")]
    InvalidName(String),

    /// The session-bus connection could not be established.
    #[error("failed to create portal D-Bus connection")]
    Connection(#[source] zbus::Error),

    /// Requesting the well-known name on the bus failed.
    #[error("failed to request portal bus name '{name}'")]
    RequestName {
        /// The well-known name that could not be acquired.
        name: String,
        /// The underlying D-Bus error.
        #[source]
        source: zbus::Error,
    },

    /// The interfaces actually registered on the connection do not match the manifest
    /// that will be advertised to xdg-desktop-portal via the `.portal` file.
    #[error(
        "registered portal interfaces do not match the manifest \
         (manifest: {expected:?}, registered: {registered:?})"
    )]
    InterfaceMismatch {
        /// Interfaces declared in the manifest / `.portal` file.
        expected: Vec<String>,
        /// Interfaces actually registered on the connection.
        registered: Vec<String>,
    },
}
