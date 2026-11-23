//! Low-level wayland protocol plumbing: the `Dispatch` state machine that
//! backs [`WlrToplevelService`](crate::WlrToplevelService).

mod dispatch;
pub(crate) mod state;
