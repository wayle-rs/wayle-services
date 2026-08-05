use zbus::zvariant::OwnedObjectPath;

/// IWD service errors.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// D-Bus communication error.
    #[error("dbus operation failed: {0}")]
    DbusError(#[from] zbus::Error),

    /// Service initialization failed (used for top-level service startup).
    #[error("cannot initialize iwd service: {0}")]
    ServiceInitializationFailed(String),

    /// Object not found at the specified D-Bus path.
    #[error("object not found at path: {0}")]
    ObjectNotFound(OwnedObjectPath),

    /// A connection attempt failed for an unspecified reason — IWD's generic
    /// `net.connman.iwd.Failed`. For a secured network this most commonly means
    /// the passphrase was rejected (IWD exposes no dedicated auth-failure
    /// error), but it can also cover other failures such as an AP refusing the
    /// association. Every other failure (including an aborted connection) maps
    /// to [`Error::OperationFailed`] instead.
    #[error("connection failed")]
    ConnectionFailed,

    /// The connection attempt was aborted — IWD's `net.connman.iwd.Aborted`,
    /// returned when an in-progress connect is cancelled by a `Disconnect` (or
    /// superseded by another connect). A user action, not a failure to surface.
    #[error("connection aborted")]
    ConnectionAborted,

    /// A network operation failed.
    #[error("cannot {operation}")]
    OperationFailed {
        /// The operation that failed.
        operation: &'static str,
        /// Underlying error that caused the failure.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Monitoring requires a cancellation token.
    #[error("cannot start monitoring: cancellation token not provided")]
    MissingCancellationToken,
}
