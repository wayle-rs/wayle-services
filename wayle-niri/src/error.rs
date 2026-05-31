//! Error types for the niri service.

use std::{
    fmt::{self, Display, Formatter},
    io,
};

/// Which socket role a connection error refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    /// The persistent socket used for request/reply command traffic.
    Command,
    /// The subscribed socket used for the event stream.
    EventStream,
}

impl Display for SocketKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command => formatter.write_str("command"),
            Self::EventStream => formatter.write_str("event-stream"),
        }
    }
}

/// Errors produced by the niri service.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// `NIRI_SOCKET` is unset, so niri is not reachable.
    #[error("niri is not running or NIRI_SOCKET is not set")]
    NiriNotRunning,

    /// Connecting the named socket failed.
    #[error("cannot connect to niri {kind} socket")]
    IpcConnectionFailed {
        /// Which socket role the connection attempt was for.
        kind: SocketKind,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// Reading or writing the socket failed.
    #[error("niri socket I/O error")]
    Io(#[from] io::Error),

    /// A JSON message could not be serialized or parsed.
    #[error("cannot parse niri JSON message")]
    JsonParse(#[from] serde_json::Error),

    /// niri replied with `Reply::Err(message)`.
    #[error("niri rejected request: {0}")]
    NiriRejected(String),

    /// niri's reply did not match the expected [`Response`](niri_ipc::Response) variant.
    #[error("unexpected response for {request} request")]
    UnexpectedResponse {
        /// Name of the request that produced the mismatch, for diagnostics.
        request: &'static str,
    },

    /// niri closed the named socket unexpectedly.
    #[error("niri closed the {kind} socket")]
    SocketClosed {
        /// Which socket role was closed.
        kind: SocketKind,
    },
}

/// Convenience alias for results produced by this crate.
pub type Result<T> = std::result::Result<T, Error>;
