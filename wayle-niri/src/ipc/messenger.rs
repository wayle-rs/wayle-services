//! Persistent command-socket client that sends [`Request`]s and returns replies.

use niri_ipc::{Reply, Request, Response};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::Mutex,
};
use tracing::instrument;

use super::niri_socket_path;
use crate::error::{Error, Result, SocketKind};

/// Owns the long-lived command socket for request/reply traffic with niri.
pub(crate) struct NiriCommandClient {
    stream: Mutex<BufReader<UnixStream>>,
}

impl NiriCommandClient {
    /// Opens the command socket using the path from `$NIRI_SOCKET`.
    ///
    /// # Errors
    ///
    /// - [`Error::NiriNotRunning`] if the environment variable is unset.
    /// - [`Error::IpcConnectionFailed`] with [`SocketKind::Command`] if the
    ///   socket path is set but the connection fails.
    pub(crate) async fn connect() -> Result<Self> {
        let socket_path = niri_socket_path()?;
        let stream = UnixStream::connect(&socket_path).await.map_err(|source| {
            Error::IpcConnectionFailed {
                kind: SocketKind::Command,
                source,
            }
        })?;

        Ok(Self {
            stream: Mutex::new(BufReader::new(stream)),
        })
    }

    /// Sends a request and awaits the matching reply.
    ///
    /// niri processes requests sequentially, so the mutex is held only for
    /// the duration of one write + read-line pair.
    ///
    /// # Errors
    ///
    /// - [`Error::Io`] on socket read/write failure.
    /// - [`Error::JsonParse`] if the request cannot be serialized or the reply
    ///   cannot be parsed.
    /// - [`Error::NiriRejected`] if niri replied `Reply::Err`.
    /// - [`Error::SocketClosed`] if the socket reached EOF before a reply
    ///   arrived.
    pub(crate) async fn request(&self, request: Request) -> Result<Response> {
        let mut serialized_request = serde_json::to_string(&request)?;
        serialized_request.push('\n');

        let mut guard = self.stream.lock().await;
        guard
            .get_mut()
            .write_all(serialized_request.as_bytes())
            .await?;

        let mut reply_line = String::new();
        let bytes_read = guard.read_line(&mut reply_line).await?;
        if bytes_read == 0 {
            return Err(Error::SocketClosed {
                kind: SocketKind::Command,
            });
        }

        let reply: Reply = serde_json::from_str(&reply_line)?;
        reply.map_err(Error::NiriRejected)
    }

    /// Sends [`Request::Version`] and returns the niri version string.
    #[instrument(skip(self), err)]
    pub(crate) async fn query_version(&self) -> Result<String> {
        match self.request(Request::Version).await? {
            Response::Version(version) => Ok(version),
            _ => Err(Error::UnexpectedResponse { request: "version" }),
        }
    }
}
