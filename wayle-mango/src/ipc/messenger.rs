//! One-shot command client for `get` and `dispatch` requests.
//!
//! Mango closes the connection after answering a `get` or `dispatch`, so each
//! request opens a fresh connection rather than reusing a persistent one.

use std::path::PathBuf;

use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};
use tracing::instrument;

use super::mango_socket_path;
use crate::error::{Error, Result, SocketKind};

/// Sends one-shot requests to Mango over the command socket.
pub(crate) struct MangoCommandClient {
    socket_path: PathBuf,
}

impl MangoCommandClient {
    /// Resolves the socket path from `$MANGO_INSTANCE_SIGNATURE`.
    ///
    /// # Errors
    ///
    /// [`Error::MangoNotRunning`] if the environment variable is unset.
    pub(crate) fn connect() -> Result<Self> {
        Ok(Self {
            socket_path: mango_socket_path()?,
        })
    }

    /// Opens a connection, sends a single command line, and returns the parsed
    /// JSON reply.
    ///
    /// # Errors
    ///
    /// - [`Error::IpcConnectionFailed`] if the socket cannot be reached.
    /// - [`Error::Io`] on read/write failure.
    /// - [`Error::SocketClosed`] if Mango closed the socket before replying.
    /// - [`Error::JsonParse`] if the reply is not valid JSON.
    /// - [`Error::MangoRejected`] if the reply is an `{"error": ...}` object.
    async fn request(&self, command: &str) -> Result<Value> {
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|source| Error::IpcConnectionFailed {
                kind: SocketKind::Command,
                source,
            })?;
        let mut reader = BufReader::new(stream);

        let command_line = format!("{command}\n");
        reader.get_mut().write_all(command_line.as_bytes()).await?;

        let mut reply_line = String::new();
        let bytes_read = reader.read_line(&mut reply_line).await?;
        if bytes_read == 0 {
            return Err(Error::SocketClosed {
                kind: SocketKind::Command,
            });
        }

        let reply: Value = serde_json::from_str(&reply_line)?;
        if let Some(message) = reply.get("error").and_then(Value::as_str) {
            return Err(Error::MangoRejected(message.to_owned()));
        }

        Ok(reply)
    }

    /// Sends a `dispatch <command>` request and discards the success reply.
    ///
    /// # Errors
    /// See [`request`](Self::request).
    pub(crate) async fn dispatch(&self, command: &str) -> Result<()> {
        self.request(&format!("dispatch {command}")).await?;
        Ok(())
    }

    /// Sends `get version` and returns the reported version string.
    ///
    /// # Errors
    ///
    /// See [`request`](Self::request), plus [`Error::UnexpectedResponse`] if the
    /// reply has no `version` field.
    #[instrument(skip(self), err)]
    pub(crate) async fn query_version(&self) -> Result<String> {
        let reply = self.request("get version").await?;

        reply
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(Error::UnexpectedResponse { request: "version" })
    }
}
