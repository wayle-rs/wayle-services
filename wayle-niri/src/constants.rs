//! Tuning values used across the crate.

/// Capacity of both the internal and the public broadcast event channels.
///
/// When a subscriber lags past this many events, tokio's broadcast channel
/// reports `RecvError::Lagged(n)` so the receiver can decide what to do.
pub(crate) const EVENT_CHANNEL_CAPACITY: usize = 100;
