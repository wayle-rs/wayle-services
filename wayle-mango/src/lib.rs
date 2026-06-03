//! Reactive bindings to the MangoWM compositor via IPC.
//!
//! [`MangoService::new`] connects to `$MANGO_INSTANCE_SIGNATURE`, subscribes to
//! Mango's `all-monitors` and `all-clients` watch streams, and exposes
//! compositor state through [`Property<T>`] fields that stay in sync
//! automatically.
//!
//! ```no_run
//! use wayle_mango::MangoService;
//! use futures::StreamExt;
//!
//! # async fn example() -> wayle_mango::Result<()> {
//! let service = MangoService::new().await?;
//!
//! for monitor in service.monitors.get() {
//!     for tag in &monitor.tags {
//!         println!("tag {} active={}", tag.index, tag.is_active);
//!     }
//! }
//!
//! let mut focused = service.focused_client.watch();
//! while let Some(client) = focused.next().await {
//!     println!("focused: {client:?}");
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Reactive properties
//!
//! Every [`Property<T>`] supports `.get()` (cloned snapshot) and `.watch()`
//! (stream that yields the current value, then each subsequent change).
//!
//! - [`MangoService::monitors`] - every monitor. Each [`Monitor`] is a plain
//!   value carrying its [`Tag`]s, active tag indices, and focused client.
//! - [`MangoService::clients`] - every open client (window) and the tags it
//!   occupies.
//! - [`MangoService::focused_client`] - the focused client on the active monitor.
//! - [`MangoService::keyboard_layout`] - the active XKB layout, global.
//! - [`MangoService::keymode`] - the active key mode, global.
//!
//! Mango pushes a full snapshot on every change, so the entire `monitors` list
//! is rebuilt each frame; watching it fires whenever any monitor or tag changes.
//!
//! # Tags, not workspaces
//!
//! Mango is dwm-derived: each monitor has a fixed set of [`Tag`]s rather than a
//! growable workspace list. Several tags can be visible at once, and a client
//! can belong to more than one, so each [`Tag`] reports `is_active`
//! individually.
//!
//! # Commands
//!
//! [`MangoService::dispatch`] sends a raw `dispatch` command. The wrappers
//! [`MangoService::view_tag`], [`MangoService::view_left`],
//! [`MangoService::view_right`], and [`MangoService::focus_window`] cover the
//! common cases.
//!
//! [`Property<T>`]: wayle_core::Property

mod constants;
mod error;
mod ipc;
mod monitoring;
mod service;

pub mod core;
pub mod types;

pub use core::{Client, Monitor};

pub use error::{Error, Result, SocketKind};
pub use service::MangoService;
pub use types::{ClientId, FocusedClient, Tag, TagId};

#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDocTests;
