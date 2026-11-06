//! WiFi management via IWD (`net.connman.iwd`).
//!
//! This crate mirrors the WiFi-relevant surface of `wayle-network` but talks to
//! IWD instead of NetworkManager. It is WiFi-only: IWD does not manage wired
//! connections or IP configuration.
//!
//! State comes from IWD: the service publishes it as reactive properties, kept
//! in sync from IWD's own signals, so it reflects changes made by any client
//! (e.g. `iwctl`), not just this one. Actions return nothing; their outcome
//! shows up as state, and a failure is recorded in the station's
//! [`last_error`](Station::last_error). The passphrase IWD asks for while
//! connecting is published as [`passphrase_request`](IwdService::passphrase_request).
//!
//! # Quick start
//!
//! ```rust,no_run
//! use wayle_iwd::{IwdService, SignalStrength};
//!
//! # async fn example() -> Result<(), wayle_iwd::Error> {
//! let iwd = IwdService::new().await?;
//!
//! if let Some(station) = iwd.station.get() {
//!     println!("powered: {}", station.powered.get());
//!     for network in station.networks.get().iter() {
//!         let strength = SignalStrength::from_dbm(network.signal.get());
//!         println!("  {} ({strength:?})", network.ssid.get());
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Connecting
//!
//! ```rust,no_run
//! # use wayle_iwd::IwdService;
//! use futures::StreamExt;
//!
//! # async fn example() -> Result<(), wayle_iwd::Error> {
//! # let iwd = IwdService::new().await?;
//! let Some(station) = iwd.station.get() else { return Ok(()) };
//! let Some(network) = station.networks.get().into_iter().next() else { return Ok(()) };
//!
//! network.connect();
//!
//! // For a secured network IWD asks for the passphrase, and asks again for as
//! // long as connecting with the one given fails.
//! let mut requests = iwd.passphrase_request.watch();
//! while let Some(request) = requests.next().await {
//!     let Some(request) = request else { continue };
//!     if request.rejected {
//!         println!("couldn't connect to {}", request.network.ssid.get());
//!         iwd.cancel_passphrase_request();
//!         break;
//!     }
//!     iwd.provide_passphrase("correct horse battery staple".to_owned());
//! }
//! # Ok(())
//! # }
//! ```

mod agent;
mod dispatcher;
mod error;
mod network;
mod props;
mod service;
mod station;
#[cfg(test)]
mod test_support;
mod types;

pub use error::Error;
pub use network::Network;
pub use service::IwdService;
pub use station::Station;
pub use types::{
    ConnectionState, PassphraseRequest, SecurityType, SignalStrength, StationAction, StationError,
};
