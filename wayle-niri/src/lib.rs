//! Reactive bindings to the niri compositor via IPC.
//!
//! [`NiriService::new`] connects to `$NIRI_SOCKET`, subscribes to niri's event
//! stream, and exposes compositor state through [`Property<T>`] fields that
//! stay in sync automatically.
//!
//! ```no_run
//! use wayle_niri::NiriService;
//! use futures::StreamExt;
//!
//! # async fn example() -> wayle_niri::Result<()> {
//! let service = NiriService::new().await?;
//!
//! for workspace in service.workspaces.get().values() {
//!     println!("{} on {:?}", workspace.idx.get(), workspace.output.get());
//! }
//!
//! let mut focused = service.focused_window_id.watch();
//! while let Some(window_id) = focused.next().await {
//!     println!("focused: {window_id:?}");
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
//! - [`NiriService::workspaces`] - all workspaces, in niri's order.
//! - [`NiriService::windows`] - every open toplevel.
//! - [`NiriService::focused_window_id`] - the id of the currently focused window.
//! - [`NiriService::overview_open`] - whether the overview is visible.
//! - [`NiriService::config_failed`] - whether the most recent reload failed.
//! - [`NiriService::keyboard_layouts`] - the configured layouts plus the current index.
//!
//! Fields on each [`Window`](core::Window) and [`Workspace`](core::Workspace)
//! are themselves [`Property<T>`] values, so watching `window.title` or
//! `workspace.is_focused` only fires when that specific field changes.
//!
//! # Events
//!
//! [`NiriService::events`] returns a `Stream<Item = Event>` mirroring the
//! event stream. Every event niri sends is forwarded, even those the internal
//! state machine chose to skip because of a transient desync.
//!
//! # Actions
//!
//! [`NiriService::dispatch_action`] dispatches a typed [`Action`]. The wrappers
//! [`NiriService::focus_window`], [`NiriService::close_window`],
//! [`NiriService::focus_workspace`], [`NiriService::focus_column`],
//! [`NiriService::spawn`], and [`NiriService::switch_layout`] cover the common
//! cases; anything beyond that goes through `dispatch_action()` directly.
//!
//! # Version pinning
//!
//! niri's IPC surface adds fields and variants in patch releases, so this
//! crate tracks a single pinned `niri-ipc` version. Upgrading `wayle-niri`
//! pulls in the matching `niri-ipc`.
//!
//! [`Property<T>`]: wayle_core::Property

mod constants;
mod error;
mod ipc;
mod monitoring;
mod service;

pub mod core;

pub use error::{Error, Result, SocketKind};
pub use niri_ipc::{
    Action, Event, KeyboardLayouts, LayoutSwitchTarget, Timestamp, WindowLayout,
    WorkspaceReferenceArg,
};
pub use service::NiriService;

#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDocTests;
