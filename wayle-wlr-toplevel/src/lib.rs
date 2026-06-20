//! Reactive bindings to the `wlr-foreign-toplevel-management-unstable-v1`
//! Wayland protocol.
//!
//! This is a wlroots-specific protocol, chosen over the newer, standard
//! `ext-foreign-toplevel-list-v1` because that one is deliberately
//! read-only - it cannot activate or close a window. This crate works on
//! any wlroots-based compositor (e.g. Sway), which have no dedicated
//! `wayle-*` service crate for window management.
//!
//! ```no_run
//! use wayle_wlr_toplevel::WlrToplevelService;
//!
//! # fn example() -> wayle_wlr_toplevel::Result<()> {
//! let service = WlrToplevelService::new()?;
//!
//! for toplevel in service.toplevels.get().values() {
//!     println!("{:?}", toplevel.title.get());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Reactive properties
//!
//! [`WlrToplevelService::toplevels`] is a [`Property`](wayle_core::Property)
//! value that refreshes once per `done` event - per the protocol, `done` is
//! emitted per-toplevel (not as a single global batch marker like
//! `ext-workspace-v1`'s manager-level `Done`), so the map republishes once
//! per affected window rather than once for every window at once.
//!
//! # Limitations
//!
//! - Outputs are exposed as wayland object ids (see
//!   [`Toplevel::outputs`](core::Toplevel::outputs)); resolving them to
//!   connector names would require also binding every `wl_output` global
//!   and tracking `wl_output.name`, which is left for a follow-up.
//! - The protocol carries no focus history, so there is no way to order
//!   windows by "most recently used" - only the current `activated` state
//!   is available.
//! - The background dispatch thread runs for the lifetime of the process;
//!   there is no graceful per-instance shutdown yet.

mod error;
mod service;
mod wayland;

pub mod core;

pub use error::{Error, Result};
pub use service::WlrToplevelService;
