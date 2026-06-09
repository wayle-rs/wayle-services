//! Tuning values and well-known names used across the crate.

/// Environment variable Mango sets to the path of its IPC socket.
pub(crate) const SOCKET_PATH_ENV: &str = "MANGO_INSTANCE_SIGNATURE";

/// The `watch` subscription that carries every monitor's tags, focused
/// client, keyboard layout, and key mode in one stream.
pub(crate) const WATCH_ALL_MONITORS: &str = "watch all-monitors";

/// The `watch` subscription that carries every client and the tags it occupies.
pub(crate) const WATCH_ALL_CLIENTS: &str = "watch all-clients";
