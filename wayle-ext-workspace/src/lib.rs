//! Reactive bindings to the `ext-workspace-v1` Wayland protocol.
//!
//! Unlike `wayle-niri` or `wayle-hyprland`, this crate talks directly to the
//! compositor over Wayland rather than a compositor-specific IPC socket, so
//! it works on any compositor implementing the `ext_workspace_manager_v1`
//! global - including wlroots-based compositors such as Sway, which have no
//! dedicated `wayle-*` service crate.
//!
//! ```no_run
//! use wayle_ext_workspace::ExtWorkspaceService;
//!
//! # fn example() -> wayle_ext_workspace::Result<()> {
//! let service = ExtWorkspaceService::new()?;
//!
//! for workspace in service.workspaces.get().values() {
//!     println!("{:?}", workspace.name.get());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Reactive properties
//!
//! [`ExtWorkspaceService::workspaces`] and [`ExtWorkspaceService::groups`]
//! are [`Property`](wayle_core::Property) values that refresh once per
//! protocol `done` event, matching the protocol's own atomic-update
//! boundary. Fields on each [`Workspace`](core::Workspace) and
//! [`WorkspaceGroup`](core::WorkspaceGroup) are themselves `Property`
//! values.
//!
//! # Limitations
//!
//! - Outputs are exposed as wayland object ids (see
//!   [`WorkspaceGroup::outputs`](core::WorkspaceGroup::outputs)); resolving
//!   them to connector names would require also binding every `wl_output`
//!   global and tracking `wl_output.name`, which is left for a follow-up.
//! - The background dispatch thread runs for the lifetime of the process;
//!   there is no graceful per-instance shutdown yet.

mod error;
mod service;
mod wayland;

pub mod core;

pub use error::{Error, Result};
pub use service::ExtWorkspaceService;
