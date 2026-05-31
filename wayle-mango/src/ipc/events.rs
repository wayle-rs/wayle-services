//! Connects a `watch` stream.
//!
//! Mango answers a `watch` subscription by pushing the current state
//! immediately and a fresh full snapshot on every change, one JSON object per
//! line. There is no handshake to acknowledge; the first line is already a
//! snapshot.

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    net::UnixStream,
};
use tracing::instrument;

use super::mango_socket_path;
use crate::error::{Error, Result, SocketKind};

/// Line reader over the subscribed watch connection; each line is one frame.
pub(crate) type WatchStream = Lines<BufReader<UnixStream>>;

/// Connects the watch socket and sends the given `watch` subscription.
///
/// The returned reader yields one JSON frame per line, starting with the
/// initial state snapshot.
///
/// # Errors
///
/// - [`Error::MangoNotRunning`] if `$MANGO_INSTANCE_SIGNATURE` is unset.
/// - [`Error::IpcConnectionFailed`] if the socket cannot be reached.
/// - [`Error::Io`] if writing the subscription fails.
#[instrument(err)]
pub(crate) async fn connect_watch_stream(subscription: &str) -> Result<WatchStream> {
    let socket_path = mango_socket_path()?;
    let stream =
        UnixStream::connect(&socket_path)
            .await
            .map_err(|source| Error::IpcConnectionFailed {
                kind: SocketKind::Watch,
                source,
            })?;
    let mut reader = BufReader::new(stream);

    let command = format!("{subscription}\n");
    reader.get_mut().write_all(command.as_bytes()).await?;

    Ok(reader.lines())
}
