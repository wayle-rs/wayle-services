//! XDG Desktop Portal backend hosting for wayle.
//!
//! A portal backend is a single D-Bus name — `org.freedesktop.impl.portal.desktop.<name>`
//! — that exposes one or more `org.freedesktop.impl.portal.*` interfaces at the object path
//! `/org/freedesktop/portal/desktop`. `xdg-desktop-portal` (the app-facing frontend) reads
//! the installed `.portal` files, picks exactly one backend per interface (via
//! `portals.conf`, matched against `XDG_CURRENT_DESKTOP`), and forwards requests to it.
//!
//! This crate provides the two pieces a shell needs to *be* such a backend, independent of
//! any specific portal interface:
//!
//! - [`PortalHost`] — owns the shared backend connection and the well-known name, with the
//!   register-then-serve ordering that keeps xdg-desktop-portal from racing a
//!   not-yet-registered interface. Each portal-providing service (e.g. `wayle-notification`)
//!   registers its interface object on [`PortalHost::connection`], then the shell calls
//!   [`PortalHost::serve`].
//! - [`PortalManifest`] — renders and validates the static `.portal` file, so the advertised
//!   interface list can be generated from, and checked against, what is actually registered.
//!
//! The individual `org.freedesktop.impl.portal.*` interface implementations live with their
//! owning services, not here — this crate is deliberately interface-agnostic infrastructure.

mod error;
mod host;
mod manifest;

pub use error::Error;
pub use host::PortalHost;
pub use manifest::PortalManifest;
