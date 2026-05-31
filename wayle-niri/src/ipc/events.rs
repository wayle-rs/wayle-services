//! Opens a dedicated socket, subscribes to the niri event stream, and pumps
//! each decoded [`Event`] into the supplied broadcast channel.

use niri_ipc::{Event, Reply, Request, Response};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    net::UnixStream,
    sync::broadcast,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};

use super::niri_socket_path;
use crate::error::{Error, Result, SocketKind};

type EventLines = Lines<BufReader<UnixStream>>;

/// Connects the event-stream socket, sends [`Request::EventStream`], and
/// spawns the read loop.
///
/// The spawned task pushes each decoded [`Event`] into `inbound_event_tx`
/// and exits on cancellation or socket EOF.
///
/// # Errors
///
/// Surfaces any error that happens before the read loop starts (connect,
/// initial handshake).
#[instrument(skip(inbound_event_tx, cancellation_token), err)]
pub(crate) async fn subscribe_events(
    inbound_event_tx: broadcast::Sender<Event>,
    cancellation_token: CancellationToken,
) -> Result<()> {
    let socket_path = niri_socket_path()?;
    let stream =
        UnixStream::connect(&socket_path)
            .await
            .map_err(|source| Error::IpcConnectionFailed {
                kind: SocketKind::EventStream,
                source,
            })?;
    let mut reader = BufReader::new(stream);

    send_event_stream_request(&mut reader).await?;
    read_handshake_ack(&mut reader).await?;

    let event_lines = reader.lines();
    tokio::spawn(pump_events(
        event_lines,
        inbound_event_tx,
        cancellation_token,
    ));

    Ok(())
}

async fn send_event_stream_request(reader: &mut BufReader<UnixStream>) -> Result<()> {
    let mut serialized_request = serde_json::to_string(&Request::EventStream)?;
    serialized_request.push('\n');
    reader
        .get_mut()
        .write_all(serialized_request.as_bytes())
        .await?;
    Ok(())
}

async fn read_handshake_ack(reader: &mut BufReader<UnixStream>) -> Result<()> {
    let mut ack_line = String::new();
    let bytes_read = reader.read_line(&mut ack_line).await?;
    if bytes_read == 0 {
        return Err(Error::SocketClosed {
            kind: SocketKind::EventStream,
        });
    }

    let reply: Reply = serde_json::from_str(&ack_line)?;
    match reply.map_err(Error::NiriRejected)? {
        Response::Handled => Ok(()),
        _ => Err(Error::UnexpectedResponse {
            request: "event-stream",
        }),
    }
}

async fn pump_events(
    mut event_lines: EventLines,
    inbound_event_tx: broadcast::Sender<Event>,
    cancellation_token: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancellation_token.cancelled() => return,
            next_line = event_lines.next_line() => {
                match next_line {
                    Ok(Some(line)) => broadcast_event_from_line(&inbound_event_tx, &line),
                    Ok(None) => {
                        warn!("niri event stream closed");
                        return;
                    }
                    Err(err) => {
                        warn!(error = %err, "event stream read error");
                        return;
                    }
                }
            }
        }
    }
}

fn broadcast_event_from_line(inbound_event_tx: &broadcast::Sender<Event>, line: &str) {
    match serde_json::from_str::<Event>(line) {
        Ok(event) => {
            let _ = inbound_event_tx.send(event);
        }
        Err(err) if err.to_string().starts_with("unknown variant") => {
            debug!(error = %err, line, "niri sent event not in niri-ipc pin, ignoring");
        }
        Err(err) => {
            warn!(error = %err, line, "cannot parse niri event");
        }
    }
}
