//! Error types for the Mango service.

use std::{
    fmt::{self, Display, Formatter},
    io,
};

/// Which socket role a connection error refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    /// A short-lived connection used for one `get` or `dispatch` request.
    Command,
    /// The subscribed connection used for a `watch` stream.
    Watch,
}

impl Display for SocketKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command => formatter.write_str("command"),
            Self::Watch => formatter.write_str("watch"),
        }
    }
}

/// Errors produced by the Mango service.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// `MANGO_INSTANCE_SIGNATURE` is unset, so Mango is not reachable.
    #[error("mango is not running or MANGO_INSTANCE_SIGNATURE is not set")]
    MangoNotRunning,

    /// Connecting the named socket failed.
    #[error("cannot connect to mango {kind} socket")]
    IpcConnectionFailed {
        /// Which socket role the connection attempt was for.
        kind: SocketKind,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// Reading or writing the socket failed.
    #[error("mango socket I/O error")]
    Io(#[from] io::Error),

    /// A JSON message could not be parsed.
    #[error("cannot parse mango JSON message")]
    JsonParse(#[from] serde_json::Error),

    /// Mango replied with an `{"error": ...}` object.
    #[error("mango rejected request: {0}")]
    MangoRejected(String),

    /// Mango's reply did not match the expected shape.
    #[error("unexpected response for {request} request")]
    UnexpectedResponse {
        /// Name of the request that produced the mismatch, for diagnostics.
        request: &'static str,
    },

    /// Mango closed the named socket before a reply arrived.
    #[error("mango closed the {kind} socket")]
    SocketClosed {
        /// Which socket role was closed.
        kind: SocketKind,
    },
}

/// Convenience alias for results produced by this crate.
pub type Result<T> = std::result::Result<T, Error>;
