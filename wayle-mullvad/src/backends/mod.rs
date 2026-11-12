//! Concrete, version-specific [`MullvadBackend`](crate::backend::MullvadBackend)
//! implementations.
//!
//! Each backend lives in its own subdirectory named after the *earliest* daemon
//! version it is compatible with, holding both the minimal protobuf schema and
//! the client implementation. Backends are registered for version ranges in
//! [`crate::backend`].

// Each directory is named after the earliest compatible daemon version, but the
// module gets a `v` prefix because a Rust identifier cannot start with a digit.
#[path = "2025_14/mod.rs"]
pub(crate) mod v2025_14;
#[path = "2025_9/mod.rs"]
pub(crate) mod v2025_9;
