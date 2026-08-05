//! WiFi management via IWD (`net.connman.iwd`).
//!
//! This crate mirrors the WiFi-relevant surface of `wayle-network` but talks to
//! IWD instead of NetworkManager. It is WiFi-only: IWD does not manage wired
//! connections or IP configuration.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use wayle_iwd::IwdService;
//!
//! # async fn example() -> Result<(), wayle_iwd::Error> {
//! let iwd = IwdService::new().await?;
//!
//! if let Some(station) = iwd.station.get() {
//!     println!("powered: {}", station.powered.get());
//!     for network in station.networks.get().iter() {
//!         println!("  {} ({:?})", network.ssid.get(), network.strength.get());
//!     }
//! }
//! # Ok(())
//! # }
//! ```

mod agent;
mod discovery;
mod error;
mod monitoring;
mod network;
mod proxy;
mod service;
mod signal_agent;
/// WiFi station model and connection controls.
pub mod station;
mod types;

pub use error::Error;
pub use network::Network;
pub use service::IwdService;
pub use station::Station;
pub use types::{ConnectionState, SecurityType, SignalStrength};
