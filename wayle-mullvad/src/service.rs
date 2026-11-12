//! The Mullvad service: lifecycle owner around the reactive [`Mullvad`] model.

use std::sync::Arc;

use derive_more::Debug;
use tokio_util::sync::CancellationToken;

use crate::{backend, core::Mullvad, error::Error};

/// A running Mullvad VPN service.
///
/// Construct one with [`MullvadService::new`]. Reactive state and controls live
/// on the [`mullvad`](Self::mullvad) model. Dropping the service cancels all
/// background monitoring.
#[derive(Debug)]
pub struct MullvadService {
    #[debug(skip)]
    cancellation_token: CancellationToken,

    /// Reactive Mullvad VPN state and controls.
    pub mullvad: Arc<Mullvad>,
}

impl MullvadService {
    /// Connects to the Mullvad daemon, selects a backend for its version and
    /// starts serving reactive state.
    ///
    /// The daemon version is queried first; if no registered backend supports
    /// it, [`Error::UnsupportedVersion`] is returned and nothing is started.
    ///
    /// # Errors
    /// Returns an error if the daemon cannot be reached, its version is
    /// unsupported, or the initial state cannot be fetched.
    #[tracing::instrument(err)]
    pub async fn new() -> Result<Arc<Self>, Error> {
        let channel = backend::connect_channel().await?;
        let version = backend::query_version(channel.clone()).await?;
        tracing::info!(version = %version.raw, "connected to Mullvad daemon");

        let Some(kind) = backend::select_backend(&version) else {
            let supported = backend::supported_ranges().collect::<Vec<_>>().join(", ");
            tracing::error!(
                version = %version.raw,
                %supported,
                "unsupported Mullvad daemon version"
            );
            return Err(Error::UnsupportedVersion {
                version: version.raw,
                supported,
            });
        };
        tracing::debug!(?kind, "selected Mullvad backend");

        let backend = backend::connect_backend(kind, channel);
        let cancellation_token = CancellationToken::new();
        let mullvad = Mullvad::connect_live(backend, cancellation_token.child_token()).await?;

        Ok(Arc::new(Self {
            cancellation_token,
            mullvad,
        }))
    }
}

impl Drop for MullvadService {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
    }
}
