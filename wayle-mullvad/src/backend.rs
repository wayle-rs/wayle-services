//! The version-independent backend abstraction and the version→backend registry.
//!
//! [`MullvadBackend`] is the single seam through which the service talks to the
//! daemon. The daemon's wire schema lives entirely behind implementations of
//! this trait, so the crate's public API is stable across daemon versions.
//!
//! On startup the service opens a channel, queries the daemon version with a
//! tiny version-independent bootstrap client ([`query_version`]), and looks the
//! version up in [`REGISTRY`] to pick a backend.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::{
    backends,
    error::Error,
    types::{NetworkCountry, NetworkTarget, TunnelStatus},
};

/// Generated bootstrap client. Version-independent: only `GetCurrentVersion`,
/// whose signature is stable across all daemon releases.
#[allow(
    missing_docs,
    unsafe_code,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cognitive_complexity,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::too_many_lines,
    clippy::large_types_passed_by_value,
    clippy::inefficient_to_string,
    clippy::manual_ok_or,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
mod bootstrap_proto {
    include!(concat!(
        env!("OUT_DIR"),
        "/bootstrap/mullvad_daemon.management_interface.rs"
    ));
}

/// Unix socket the Mullvad daemon exposes its management interface on.
const SOCKET_PATH: &str = "/var/run/mullvad-vpn";

/// Bounds the connect + HTTP/2 handshake so a hung daemon can't hang startup.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

// ------------------------------------------------------------------- trait

/// A version-specific implementation of the Mullvad management client.
///
/// Everything the service needs flows through this trait, returning and
/// accepting only the crate's [public types](crate::types). To support a new
/// daemon version, implement this trait and register it in [`REGISTRY`].
#[async_trait]
pub trait MullvadBackend: Send + Sync {
    /// Fetches the current tunnel status (state and connected relay).
    ///
    /// # Errors
    /// Returns an error if the RPC fails.
    async fn tunnel_status(&self) -> Result<TunnelStatus, Error>;

    /// Fetches whether an account is currently logged in.
    ///
    /// # Errors
    /// Returns an error if the RPC fails.
    async fn logged_in(&self) -> Result<bool, Error>;

    /// Fetches the available networks as a country → city → network tree.
    ///
    /// # Errors
    /// Returns an error if the RPC fails.
    async fn networks(&self) -> Result<Vec<NetworkCountry>, Error>;

    /// Applies `target` as the relay selection and connects the tunnel.
    ///
    /// # Errors
    /// Returns an error if applying the selection or connecting fails.
    async fn connect(&self, target: &NetworkTarget) -> Result<(), Error>;

    /// Connects the tunnel using the daemon's current relay settings, without
    /// changing the selected relay. Used to reconnect after a disconnect.
    ///
    /// # Errors
    /// Returns an error if the RPC fails.
    async fn reconnect(&self) -> Result<(), Error>;

    /// Disconnects the tunnel.
    ///
    /// # Errors
    /// Returns an error if the RPC fails.
    async fn disconnect(&self) -> Result<(), Error>;

    /// Opens a stream of [`BackendEvent`]s for reactive updates.
    ///
    /// # Errors
    /// Returns an error if the event subscription cannot be established.
    async fn events(&self) -> Result<BoxStream<'static, BackendEvent>, Error>;
}

/// A change pushed from a backend's daemon event stream, already translated
/// into public types.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    /// The tunnel status changed.
    Tunnel(TunnelStatus),
    /// The login state changed.
    LoggedIn(bool),
    /// The available-network list changed.
    Networks(Vec<NetworkCountry>),
}

// ------------------------------------------------------------ version registry

/// A parsed Mullvad daemon version, e.g. `2026.2` or `2025.14-beta1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonVersion {
    /// Year component (e.g. `2026`); `0` if it could not be parsed.
    pub year: u32,
    /// Incremental component within the year (e.g. `2`); `0` if absent.
    pub incremental: u32,
    /// The raw version string reported by the daemon.
    pub raw: String,
}

impl DaemonVersion {
    /// Parses a daemon version string such as `"2026.2"` or `"2025.14-beta1"`.
    ///
    /// Best-effort: unparseable components default to `0`, which simply fails
    /// to match any registry entry and is reported as unsupported.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let stem = raw.split(['-', '+', ' ']).next().unwrap_or(raw);
        let mut parts = stem.split('.');
        let year = parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0);
        let incremental = parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0);
        Self {
            year,
            incremental,
            raw: raw.to_owned(),
        }
    }
}

/// Identifies a concrete backend implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Backend for the Mullvad 2025.14-and-newer daemon line (2025.14 is the
    /// earliest release compatible with its schema snapshot).
    V2025_14,
    /// Backend for the Mullvad 2025.9–2025.13 (OpenVPN-era) daemon line (2025.9
    /// is the earliest release compatible with its schema snapshot).
    V2025_9,
}

/// One entry in the version→backend registry.
struct BackendRange {
    /// Human-readable description of the daemon versions this entry covers.
    description: &'static str,
    /// Backend used for daemon versions this entry matches.
    kind: BackendKind,
    /// Returns `true` if this entry handles `version`.
    matches: fn(&DaemonVersion) -> bool,
}

/// Registry mapping daemon version ranges to backend implementations.
///
/// **To support a new daemon version:**
/// 1. add the backend module under `src/backends/`,
/// 2. add a [`BackendKind`] variant for it,
/// 3. add an entry here mapping its version range to that variant, and
/// 4. add the matching arm in [`connect_backend`].
///
/// Entries are tested top to bottom; the first match wins. The newest backend
/// (`V2025_14`) is intentionally forward-optimistic — it matches 2025.14 and
/// every newer release, because the daemon schema evolves additively within a
/// line, so its snapshot keeps decoding newer releases until a breaking change
/// ships and a newer backend is registered. Older lines get closed ranges: the
/// 2025.14 floor exists because that release renumbered
/// `NormalRelaySettings.ownership` (OpenVPN removal), and the `V2025_9` line
/// stops at 2025.13 for the same reason and starts at 2025.9 (which moved
/// `Relay.location` to field 10).
static REGISTRY: &[BackendRange] = &[
    BackendRange {
        description: "2025.14 and newer",
        kind: BackendKind::V2025_14,
        matches: matches_2025_14_line,
    },
    BackendRange {
        description: "2025.9 through 2025.13",
        kind: BackendKind::V2025_9,
        matches: matches_2025_9_line,
    },
];

/// Matches the 2025.14-and-newer daemon line (forward-optimistic).
fn matches_2025_14_line(version: &DaemonVersion) -> bool {
    version.year > 2025 || (version.year == 2025 && version.incremental >= 14)
}

/// Matches the closed 2025.9–2025.13 (OpenVPN-era) daemon range.
fn matches_2025_9_line(version: &DaemonVersion) -> bool {
    version.year == 2025 && (9..=13).contains(&version.incremental)
}

/// Selects the backend for a daemon version, or `None` if unsupported.
pub(crate) fn select_backend(version: &DaemonVersion) -> Option<BackendKind> {
    REGISTRY
        .iter()
        .find(|range| (range.matches)(version))
        .map(|range| range.kind)
}

/// Human-readable descriptions of every supported daemon version range.
pub(crate) fn supported_ranges() -> impl Iterator<Item = &'static str> {
    REGISTRY.iter().map(|range| range.description)
}

/// Constructs the backend implementation for `kind` over an existing channel.
pub(crate) fn connect_backend(kind: BackendKind, channel: Channel) -> Arc<dyn MullvadBackend> {
    match kind {
        BackendKind::V2025_14 => Arc::new(backends::v2025_14::Backend::new(channel)),
        BackendKind::V2025_9 => Arc::new(backends::v2025_9::Backend::new(channel)),
    }
}

// ------------------------------------------------------------ connection setup

/// Opens a gRPC channel to the Mullvad daemon's management socket.
///
/// # Errors
/// Returns [`Error::SocketNotFound`] if the socket is absent, or
/// [`Error::Transport`] if the connection cannot be established.
pub(crate) async fn connect_channel() -> Result<Channel, Error> {
    connect_channel_at(SOCKET_PATH).await
}

/// Opens a gRPC channel to the daemon management socket at `path`.
///
/// The socket's existence is checked up front so a missing daemon produces a
/// clear [`Error::SocketNotFound`] rather than an opaque transport error.
///
/// # Errors
/// Returns [`Error::SocketNotFound`] if the socket is absent, or
/// [`Error::Transport`] if the connection cannot be established.
async fn connect_channel_at(path: &str) -> Result<Channel, Error> {
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Err(Error::SocketNotFound {
            path: path.to_owned(),
        });
    }

    // The URI is ignored for Unix sockets, but tonic still requires a valid one.
    // A connect timeout bounds the whole connect + HTTP/2 handshake so a
    // socket-present-but-unresponsive daemon fails instead of hanging forever.
    let socket_path = path.to_owned();
    let channel = Endpoint::try_from("http://[::]:0")?
        .connect_timeout(CONNECT_TIMEOUT)
        .connect_with_connector(service_fn(move |_: Uri| {
            let socket_path = socket_path.clone();
            async move { UnixStream::connect(socket_path).await }
        }))
        .await?;
    Ok(channel)
}

/// Queries the daemon version using the version-independent bootstrap client.
///
/// # Errors
/// Returns [`Error::Rpc`] if the version RPC fails.
pub(crate) async fn query_version(channel: Channel) -> Result<DaemonVersion, Error> {
    use bootstrap_proto::{Empty, management_service_client::ManagementServiceClient};

    let mut client = ManagementServiceClient::new(channel);
    let reply = client.get_current_version(Empty {}).await?;
    Ok(DaemonVersion::parse(&reply.into_inner().value))
}

#[cfg(test)]
mod tests {
    use super::{BackendKind, DaemonVersion, connect_channel_at, select_backend, supported_ranges};
    use crate::error::Error;

    #[test]
    fn parses_standard_version() {
        let version = DaemonVersion::parse("2026.2");
        assert_eq!(version.year, 2026);
        assert_eq!(version.incremental, 2);
        assert_eq!(version.raw, "2026.2");
    }

    #[test]
    fn parses_version_with_suffix() {
        let version = DaemonVersion::parse("2025.14-beta1");
        assert_eq!(version.year, 2025);
        assert_eq!(version.incremental, 14);
    }

    #[test]
    fn unparseable_version_defaults_to_zero() {
        let version = DaemonVersion::parse("not-a-version");
        assert_eq!(version.year, 0);
        assert_eq!(version.incremental, 0);
        assert_eq!(version.raw, "not-a-version");
    }

    #[test]
    fn selects_backend_for_2025_14_and_newer() {
        // 2025.14 is where OpenVPN removal renumbered the relay settings.
        assert_eq!(
            select_backend(&DaemonVersion::parse("2025.14")),
            Some(BackendKind::V2025_14)
        );
        assert_eq!(
            select_backend(&DaemonVersion::parse("2025.14-beta1")),
            Some(BackendKind::V2025_14)
        );
        assert_eq!(
            select_backend(&DaemonVersion::parse("2025.20")),
            Some(BackendKind::V2025_14)
        );
        assert_eq!(
            select_backend(&DaemonVersion::parse("2026.2")),
            Some(BackendKind::V2025_14)
        );
        assert_eq!(
            select_backend(&DaemonVersion::parse("2027.0")),
            Some(BackendKind::V2025_14)
        );
    }

    #[test]
    fn selects_backend_for_openvpn_era_range() {
        // 2025.9 (Relay.location moved to field 10) through 2025.13 (last before
        // OpenVPN removal) is served by the closed-range OpenVPN-era backend.
        assert_eq!(
            select_backend(&DaemonVersion::parse("2025.9")),
            Some(BackendKind::V2025_9)
        );
        assert_eq!(
            select_backend(&DaemonVersion::parse("2025.11")),
            Some(BackendKind::V2025_9)
        );
        assert_eq!(
            select_backend(&DaemonVersion::parse("2025.13")),
            Some(BackendKind::V2025_9)
        );
    }

    #[test]
    fn rejects_older_and_unparseable_versions() {
        // 2025.8 and older place `Relay.location` at field 11 (field 10 is
        // `endpoint_data`), so they are below every backend's floor.
        assert_eq!(select_backend(&DaemonVersion::parse("2025.8")), None);
        assert_eq!(select_backend(&DaemonVersion::parse("2024.5")), None);
        assert_eq!(select_backend(&DaemonVersion::parse("garbage")), None);
    }

    #[test]
    fn registry_lists_at_least_one_supported_range() {
        assert!(supported_ranges().count() >= 1);
    }

    #[tokio::test]
    async fn connect_errors_when_socket_missing() {
        let result = connect_channel_at("/nonexistent/wayle-mullvad-test.sock").await;
        assert!(matches!(result, Err(Error::SocketNotFound { .. })));
    }
}
