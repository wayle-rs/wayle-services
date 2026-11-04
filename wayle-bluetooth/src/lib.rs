//! Bluetooth device management via BlueZ D-Bus.
//!
//! State comes from BlueZ: the service publishes it as reactive properties,
//! kept in sync from BlueZ's own signals (and the kernel's rfkill events), so
//! it reflects changes made by any client (e.g. `bluetoothctl`, or `rfkill`),
//! not just this one. The exceptions are what
//! BlueZ doesn't publish: the operation this service is performing on a
//! device, the most recent failure, and the pairing request BlueZ asked this
//! service's agent. Actions return nothing; their outcome shows up as state.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use wayle_bluetooth::BluetoothService;
//!
//! # async fn example() -> Result<(), wayle_bluetooth::Error> {
//! let bt = BluetoothService::new().await?;
//!
//! // Check adapter state
//! if bt.available.get() {
//!     println!("Bluetooth available, powered: {}", bt.powered.get());
//! }
//!
//! // List devices
//! for device in bt.devices.get().iter() {
//!     let info = device.info.get();
//!     println!("{}: {}", info.alias, info.address);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Watching for Changes
//!
//! ```rust,no_run
//! use wayle_bluetooth::BluetoothService;
//! use futures::StreamExt;
//!
//! # async fn example() -> Result<(), wayle_bluetooth::Error> {
//! # let bt = BluetoothService::new().await?;
//! // React to new devices
//! let mut stream = bt.devices.watch();
//! while let Some(devices) = stream.next().await {
//!     println!("Device count: {}", devices.len());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Discovery and Connecting
//!
//! Actions return immediately; watch state for the outcome.
//!
//! ```rust,no_run
//! # use wayle_bluetooth::BluetoothService;
//! # use std::time::Duration;
//! use futures::StreamExt;
//!
//! # async fn example() -> Result<(), wayle_bluetooth::Error> {
//! # let bt = BluetoothService::new().await?;
//! // Scan for 30 seconds
//! bt.start_timed_discovery(Duration::from_secs(30));
//!
//! // Connect to a device, then wait for the outcome
//! let headphones = bt
//!     .devices
//!     .get()
//!     .into_iter()
//!     .find(|device| device.info.get().name.as_deref() == Some("My Headphones"));
//!
//! if let Some(device) = headphones {
//!     // An earlier failure may still be recorded; only a new one is this
//!     // connect's (failures are equal only to themselves).
//!     let earlier_error = device.info.get().last_error.clone();
//!     device.connect();
//!
//!     let mut info = device.info.watch();
//!     while let Some(info) = info.next().await {
//!         if info.connected {
//!             println!("connected");
//!             break;
//!         }
//!         if let Some(error) = &info.last_error
//!             && Some(error) != earlier_error.as_ref()
//!         {
//!             println!("cannot connect: {}", error.error);
//!             break;
//!         }
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Reactive Properties
//!
//! All fields are [`Property<T>`](wayle_core::Property):
//! - `.get()` - Current value snapshot
//! - `.watch()` - Stream yielding on changes
//!
//! # Service Fields
//!
//! | Field | Type | Description |
//! |-------|------|-------------|
//! | `adapters` | `Vec<Arc<Adapter>>` | All Bluetooth adapters |
//! | `primary_adapter` | `Option<Arc<Adapter>>` | Active adapter for operations |
//! | `devices` | `Vec<Arc<Device>>` | All known devices |
//! | `available` | `bool` | Whether an adapter is present, or rfkill blocks a Bluetooth radio |
//! | `enabled` | `bool` | Whether no radio is blocked and an adapter is on or turning on |
//! | `powered` | `bool` | Whether no radio is blocked and an adapter is on |
//! | `discovering` | `bool` | Whether the primary adapter is discovering |
//! | `connected` | `Vec<Arc<Device>>` | Currently connected devices |
//! | `pairing_request` | `Option<PairingRequest>` | Pending pairing request |
//! | `radio_block` | [`RadioBlock`](types::RadioBlock) | Whether rfkill blocks a Bluetooth radio, and whether software can lift that |
//!
//! Each adapter and device is a single live instance; look one up by object
//! path with [`device()`](BluetoothService::device) /
//! [`adapter()`](BluetoothService::adapter). A device's state is
//! [`info`](core::device::Device::info) (including the operation this service
//! is performing on it, and the most recent failure) and
//! [`signal`](core::device::Device::signal) (RSSI and other advertisement
//! data, which changes constantly while scanning).
//!
//! # Actions
//!
//! - [`enable()`](BluetoothService::enable) / [`disable()`](BluetoothService::disable) - Turn Bluetooth on and off
//!   (through rfkill where possible)
//! - [`start_discovery()`](BluetoothService::start_discovery) / [`stop_discovery()`](BluetoothService::stop_discovery) - Scan
//!   (stopping ends every scan this service started)
//! - [`start_timed_discovery()`](BluetoothService::start_timed_discovery) - Scan with timeout
//!   (repeated calls extend it)
//!
//! Device-level: `connect()`, `disconnect()`, `pair()`, `forget()`

mod agent;
/// Bluetooth domain models for adapters and devices.
pub mod core;
mod dispatcher;
mod error;
mod props;
mod proxy;
mod service;
#[cfg(test)]
mod test_support;
/// BlueZ type definitions for adapter/device properties.
pub mod types;

pub use error::Error;
pub use service::BluetoothService;

#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDocTests;
