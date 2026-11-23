//! Error types for the wlr-toplevel service.

use wayland_client::{
    ConnectError, DispatchError,
    globals::{BindError, GlobalError},
};

/// Errors produced by the wlr-toplevel service.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// No wayland display is reachable (e.g. running outside a Wayland session).
    #[error("cannot connect to the wayland display")]
    ConnectionFailed(#[from] ConnectError),

    /// The registry could not be enumerated.
    #[error("cannot enumerate wayland globals")]
    GlobalEnumerationFailed(#[from] GlobalError),

    /// The compositor does not advertise `zwlr_foreign_toplevel_manager_v1`.
    #[error("compositor does not support wlr-foreign-toplevel-management-v1")]
    NotSupported(#[from] BindError),

    /// Dispatching the wayland event queue failed.
    #[error("wayland event dispatch failed")]
    Dispatch(#[from] DispatchError),

    /// The background event-loop thread could not be spawned.
    #[error("cannot spawn the wlr-toplevel event loop thread")]
    ThreadSpawnFailed(#[from] std::io::Error),
}

/// Convenience alias for results produced by this crate.
pub type Result<T> = std::result::Result<T, Error>;
