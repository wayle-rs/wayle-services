//! Desktop notification service implementing the freedesktop.org Desktop Notifications spec.
//!
//! # Overview
//!
//! Registers as `org.freedesktop.Notifications` on D-Bus to receive notifications from
//! applications. Notifications are stored, displayed as popups, and can be dismissed
//! or have actions invoked.
//!
//! # Scope: Wayland only
//!
//! This crate targets Wayland compositors/shells; X11 is out of scope. That guides which
//! wire features are modeled: X11-era artifacts with no valid Wayland semantics (e.g. the
//! spec's `x`/`y` "point-to-screen-location" hints, which a Wayland client cannot fill
//! because it has no access to a global screen coordinate) are intentionally not supported.
//! Everything a Wayland shell can actually display is exposed as a typed facet rather than an
//! untyped hint bag.
//!
//! # Reactive Properties
//!
//! Service state is exposed through [`Property`](wayle_core::Property) fields:
//! - `.get()` returns a snapshot of the current value
//! - `.watch()` returns a stream that yields on changes
//!
//! # Service Fields
//!
//! | Field | Type | Description |
//! |-------|------|-------------|
//! | `notifications` | `Vec<Arc<Notification>>` | All received notifications |
//! | `popups` | `Vec<Arc<Notification>>` | Currently visible popups |
//! | `popup_duration` | `u32` | Popup display time in ms |
//! | `dnd` | `bool` | Do Not Disturb mode (suppresses popups) |
//! | `remove_expired` | `bool` | Auto-remove expired notifications |
//!
//! # Example
//!
//! ```no_run
//! use wayle_notification::NotificationService;
//! use futures::StreamExt;
//!
//! # async fn example() -> Result<(), wayle_notification::Error> {
//! let service = NotificationService::new().await?;
//!
//! // Snapshot access
//! let count = service.notifications.get().len();
//!
//! // Reactive stream
//! let mut stream = service.notifications.watch();
//! while let Some(notifications) = stream.next().await {
//!     println!("{} notifications", notifications.len());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Configuration
//!
//! | Method | Effect |
//! |--------|--------|
//! | `with_daemon()` | Control notifications from scripts or other processes |
//!
//! ```no_run
//! use wayle_notification::NotificationService;
//!
//! # async fn example() -> Result<(), wayle_notification::Error> {
//! let service = NotificationService::builder()
//!     .with_daemon()
//!     .build()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! # D-Bus Interface
//!
//! When `with_daemon()` is enabled, the service registers on the session bus.
//!
//! - **Service:** `com.wayle.Notifications1`
//! - **Path:** `/com/wayle/Notifications`
//! - **Interface:** `com.wayle.Notifications1`
//!
//! See [`dbus.md`](https://github.com/wayle-rs/wayle-services/blob/master/wayle-notification/dbus.md) for the full interface specification.

/// Notification ingest backends: the three protocol adapters (freedesktop, GTK, portal) plus the
/// helpers they share (wire-format parsing, `.desktop` metadata, blocklist glob matching).
pub(crate) mod backends;
mod builder;
/// Notification data structures and operations.
pub mod core;
/// Error types.
pub mod error;
pub(crate) mod events;
pub(crate) mod image_cache;
pub(crate) mod monitoring;
pub(crate) mod persistence;
pub(crate) mod popup_timer;
/// D-Bus client proxies (the wayle-native extension + the freedesktop interface).
pub(crate) mod proxy;
/// Service implementation.
pub mod service;
/// freedesktop notification types (Urgency, ClosedReason, Capabilities, etc.).
pub mod types;
pub(crate) mod wayle_daemon;

pub use builder::NotificationServiceBuilder;
pub use error::Error;
pub use proxy::wayle::WayleNotificationsProxy;
pub use service::NotificationService;

#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDocTests;
