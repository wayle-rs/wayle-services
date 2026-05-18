//! IPC layer: command-socket client and event-stream subscription.

mod events;
mod messenger;

use std::{env, ffi::OsString, path::PathBuf};

pub(crate) use events::subscribe_events;
pub(crate) use messenger::NiriCommandClient;
use niri_ipc::socket::SOCKET_PATH_ENV;

use crate::error::{Error, Result};

/// Reads `$NIRI_SOCKET` and turns it into a path. Both the command socket
/// and the event-stream socket use the same env var.
pub(super) fn niri_socket_path() -> Result<PathBuf> {
    let raw: OsString = env::var_os(SOCKET_PATH_ENV).ok_or(Error::NiriNotRunning)?;
    Ok(PathBuf::from(raw))
}
