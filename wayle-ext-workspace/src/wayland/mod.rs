//! Low-level wayland protocol plumbing: the `Dispatch` state machine that
//! backs [`ExtWorkspaceService`](crate::ExtWorkspaceService).

mod dispatch;
pub(crate) mod state;
