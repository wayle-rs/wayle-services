//! System information monitoring service.
//!
//! Provides reactive access to CPU, memory, disk, network, and GPU metrics
//! via polling-based background tasks.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use wayle_sysinfo::SysinfoService;
//!
//! let service = SysinfoService::builder().build();
//!
//! // Access current values
//! let cpu = service.cpu.get();
//! println!("CPU: {:.1}%", cpu.usage_percent);
//!
//! let memory = service.memory.get();
//! println!("Memory: {:.1}%", memory.usage_percent);
//!
//! let gpu = service.gpu.get();
//! println!(
//!     "GPUs: {}, avg util: {:.1}%",
//!     gpu.total_count, gpu.average_utilization_percent
//! );
//! ```
//!
//! # Reactive Streams
//!
//! All properties support `.watch()` for reactive updates:
//!
//! ```rust,no_run
//! use wayle_sysinfo::SysinfoService;
//! use futures::StreamExt;
//!
//! # async fn example() {
//! let service = SysinfoService::builder().build();
//!
//! let mut cpu_stream = service.cpu.watch();
//! while let Some(cpu) = cpu_stream.next().await {
//!     println!("CPU changed: {:.1}%", cpu.usage_percent);
//! }
//!
//! let mut gpu_stream = service.gpu.watch();
//! while let Some(gpu) = gpu_stream.next().await {
//!     println!("GPU devices changed: {}", gpu.total_count);
//! }
//! # }
//! ```

mod builder;
mod error;
mod polling;
mod service;
/// Data types for system metrics.
pub mod types;

pub use builder::SysinfoServiceBuilder;
pub use error::Error;
pub use service::SysinfoService;

#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDocTests;
