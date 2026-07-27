//! Error type for the Mullvad service.

/// Errors produced while connecting to or driving the Mullvad daemon.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The daemon management socket does not exist (the daemon is likely not
    /// installed or not running).
    #[error("Mullvad daemon socket not found at {path} (is the daemon running?)")]
    SocketNotFound {
        /// Filesystem path that was checked for the socket.
        path: String,
    },

    /// The gRPC transport to the daemon could not be established.
    #[error("failed to connect to the Mullvad daemon: {0}")]
    Transport(#[from] tonic::transport::Error),

    /// A gRPC request to the daemon failed.
    #[error("Mullvad daemon RPC failed: {0}")]
    Rpc(#[from] tonic::Status),

    /// The running daemon version has no registered backend.
    #[error("unsupported Mullvad daemon version {version:?}; supported ranges: {supported}")]
    UnsupportedVersion {
        /// The version string reported by the daemon.
        version: String,
        /// Comma-separated descriptions of the supported version ranges.
        supported: String,
    },

    /// Monitoring could not start because no cancellation token was provided.
    #[error("cannot start monitoring: no cancellation token was provided")]
    MissingCancellationToken,
}
