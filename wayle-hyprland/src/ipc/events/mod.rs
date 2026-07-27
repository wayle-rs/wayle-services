mod dispatcher;
pub(crate) mod layer;
pub(crate) mod monitor;
pub(crate) mod types;
pub(crate) mod window;
pub(crate) mod workspace;

use std::env;

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::UnixStream,
    sync::broadcast::Sender,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::{Error, HyprlandEvent, Result};

pub(crate) async fn subscribe(
    event_tx: Sender<HyprlandEvent>,
    cancel_token: CancellationToken,
) -> Result<()> {
    let his = env::var("HYPRLAND_INSTANCE_SIGNATURE").map_err(|_| Error::HyprlandNotRunning)?;
    let runtime_dir = env::var("XDG_RUNTIME_DIR")
        .map_err(|_| Error::InvalidInstanceSignature("XDG_RUNTIME_DIR not set".to_string()))?;

    let socket_path = format!("{runtime_dir}/hypr/{his}/.socket2.sock");
    let event_stream =
        UnixStream::connect(&socket_path)
            .await
            .map_err(|source| Error::IpcConnectionFailed {
                socket_type: "event",
                source,
            })?;

    tokio::spawn(async move {
        let mut reader = BufReader::new(event_stream);
        let mut buf = vec![];

        loop {
            buf.clear();

            tokio::select! {
                () = cancel_token.cancelled() => {
                    debug!("Hyprland event subscription cancelled");
                    break;
                }
                line_result = reader.read_until(b'\n', &mut buf) => {
                    match line_result {
                        Ok(0) => {
                            warn!("Hyprland event stream closed");
                            break;
                        }
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&buf);
                            let line = line.trim_end_matches(['\n', '\r']);

                            let Some((event, data)) = line.split_once(">>") else {
                                warn!(raw_data = %line, "cannot parse hyprland event: missing '>>' separator");
                                continue;
                            };

                            if let Err(e) = dispatcher::dispatch(event, data, event_tx.clone()).await {
                                warn!(error = %e, event, "cannot handle event");
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "Error reading event stream");
                            break;
                        }
                    }
                }
            }
        }
    });

    Ok(())
}
