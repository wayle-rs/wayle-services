//! Notification data and internal types.

/// The per-notification backend dispatch interface.
pub(crate) mod backend;
/// The [`Notification`](notification::Notification) struct.
pub mod notification;
/// Action and hint types.
pub mod types;
