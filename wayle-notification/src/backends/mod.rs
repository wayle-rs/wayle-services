//! Notification ingest backends.
//!
//! Each protocol wayle receives notifications from lives in its own self-contained module
//! and normalizes incoming notifications into the shared [`Notification`] domain type,
//! funneling them through the one [`NotificationEvent`] pipeline. Adding a new protocol is
//! a matter of adding a module here — the shared domain model, pipeline, persistence and
//! id allocation are written once elsewhere (`core`, `persistence`) and never duplicated.
//!
//! Protocol adapters:
//! - [`freedesktop`]: `org.freedesktop.Notifications` (the freedesktop.org spec)
//! - [`gtk`]: `org.gtk.Notifications` (`GNotification`)
//! - [`portal`]: `org.freedesktop.impl.portal.Notification` (XDG Desktop Portal backend)
//!
//! Shared helpers (each used only by the backends): [`gvariant`] (`a{sv}`/GIcon wire parsing),
//! [`desktop_entry`] (`.desktop` name/icon resolution), [`glob`] (blocklist glob matching), and
//! [`gapplication`] (`org.freedesktop.Application` raise/cold-launch).
//!
//! [`Notification`]: crate::core::notification::Notification
//! [`NotificationEvent`]: crate::events::NotificationEvent

pub(crate) mod freedesktop;
pub(crate) mod gtk;
pub(crate) mod portal;

/// `.desktop`-entry name/icon resolution, used by [`gtk`] and [`portal`].
mod desktop_entry;
/// `org.freedesktop.Application` activation (raise / cold-launch), used by [`gtk`] and [`portal`].
mod gapplication;
/// Glob matching for the app-name blocklist, used by all three backends.
mod glob;
/// Shared parsing for the `GNotification`-serialized `a{sv}` used by [`gtk`] and [`portal`].
mod gvariant;
