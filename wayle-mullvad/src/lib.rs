//! Reactive Mullvad VPN status and control for Wayle.
//!
//! This crate exposes a small, backend-independent public API for observing and
//! controlling a Mullvad VPN daemon. State is published as reactive
//! [`Property`](wayle_core::Property) fields on the [`Mullvad`] model, and the
//! tunnel is driven with [`Mullvad::connect`]/[`Mullvad::disconnect`].
//!
//! # Versioned backends
//!
//! The daemon's gRPC management interface is not a stable, versioned API, so the
//! wire schema is confined behind the [`MullvadBackend`] trait. On startup the
//! service queries the daemon version and selects a backend from a small
//! registry (see [`backend`]); if the version is unsupported it returns
//! [`Error::UnsupportedVersion`]. The public API here stays stable across
//! backend changes.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use wayle_mullvad::{MullvadService, NetworkTarget};
//!
//! # async fn example() -> Result<(), wayle_mullvad::Error> {
//! let service = MullvadService::new().await?;
//!
//! // Read a snapshot of reactive state.
//! let _logged_in: bool = service.mullvad.logged_in.get();
//!
//! // Connect to a whole country (by code), then disconnect. These are
//! // non-blocking — observe the result via the reactive state.
//! service.mullvad.connect(&NetworkTarget::country("se"));
//! service.mullvad.disconnect();
//! # Ok(())
//! # }
//! ```

mod backend;
mod backends;
mod core;
mod error;
mod service;
pub mod types;

pub use backend::{BackendEvent, BackendKind, DaemonVersion, MullvadBackend};
pub use error::Error;
pub use service::MullvadService;
pub use types::{
    ConnectedNetwork, ConnectionState, MullvadNetwork, NetworkCity, NetworkCountry, NetworkTarget,
    TunnelStatus,
};

pub use crate::core::Mullvad;

#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDocTests;
