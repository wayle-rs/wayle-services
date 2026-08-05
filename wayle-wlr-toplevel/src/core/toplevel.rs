//! Reactive wrapper for a `zwlr_foreign_toplevel_handle_v1` object.

use derive_more::Debug;
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1;
use wayle_core::Property;

/// Bit set on [`Toplevel::state`] when the toplevel is maximized.
pub const STATE_MAXIMIZED: u32 = 1;
/// Bit set on [`Toplevel::state`] when the toplevel is minimized.
pub const STATE_MINIMIZED: u32 = 2;
/// Bit set on [`Toplevel::state`] when the toplevel is the active window.
pub const STATE_ACTIVATED: u32 = 4;
/// Bit set on [`Toplevel::state`] when the toplevel is fullscreen.
pub const STATE_FULLSCREEN: u32 = 8;

/// A toplevel (window) advertised by `zwlr_foreign_toplevel_manager_v1`.
///
/// Fields update in place as the compositor sends events. Identity (the
/// [`Toplevel::key`]) is the wayland object id and is stable for the
/// lifetime of the handle, but is not preserved across reconnects.
#[derive(Debug)]
pub struct Toplevel {
    #[debug(skip)]
    pub(crate) handle: ZwlrForeignToplevelHandleV1,

    /// Local, process-lifetime-stable key (the wayland object id).
    pub key: u32,
    /// Window title.
    pub title: Property<Option<String>>,
    /// Application id (roughly: the app's identity, e.g. `firefox`).
    pub app_id: Property<Option<String>>,
    /// Wayland object ids of the outputs this toplevel is visible on.
    ///
    /// Exposed as raw object ids rather than connector names; see the
    /// crate-level limitations note.
    pub outputs: Property<Vec<u32>>,
    /// Bitmask of `STATE_*` flags.
    pub state: Property<u32>,
    /// Local key of the parent toplevel, if any (protocol version 3+).
    pub parent: Property<Option<u32>>,
}

impl Toplevel {
    pub(crate) fn new(handle: ZwlrForeignToplevelHandleV1, key: u32) -> Self {
        Self {
            handle,
            key,
            title: Property::new(None),
            app_id: Property::new(None),
            outputs: Property::new(Vec::new()),
            state: Property::new(0),
            parent: Property::new(None),
        }
    }

    /// Whether this is the currently active toplevel.
    pub fn is_activated(&self) -> bool {
        self.state.get() & STATE_ACTIVATED != 0
    }

    /// Whether the toplevel is minimized.
    pub fn is_minimized(&self) -> bool {
        self.state.get() & STATE_MINIMIZED != 0
    }

    /// Whether the toplevel is maximized.
    pub fn is_maximized(&self) -> bool {
        self.state.get() & STATE_MAXIMIZED != 0
    }

    /// Whether the toplevel is fullscreen.
    pub fn is_fullscreen(&self) -> bool {
        self.state.get() & STATE_FULLSCREEN != 0
    }

    /// Queues a request that the toplevel be closed.
    ///
    /// Requests are only applied once the connection is flushed - see
    /// [`WlrToplevelService::close_toplevel`](crate::WlrToplevelService::close_toplevel)
    /// for the usual entry point.
    pub fn close(&self) {
        self.handle.close();
    }
}

impl PartialEq for Toplevel {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
