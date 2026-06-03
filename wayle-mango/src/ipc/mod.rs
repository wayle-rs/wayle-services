//! IPC layer: the one-shot command client and the `watch` streams.

mod events;
mod messenger;

use std::{env, ffi::OsString, path::PathBuf};

pub(crate) use events::{WatchStream, connect_watch_stream};
pub(crate) use messenger::MangoCommandClient;

use crate::{
    constants::SOCKET_PATH_ENV,
    error::{Error, Result},
};

/// Reads `$MANGO_INSTANCE_SIGNATURE` and turns it into a path. Both the command
/// connection and the watch stream connect to this same socket.
pub(super) fn mango_socket_path() -> Result<PathBuf> {
    let raw: OsString = env::var_os(SOCKET_PATH_ENV).ok_or(Error::MangoNotRunning)?;
    Ok(PathBuf::from(raw))
}
