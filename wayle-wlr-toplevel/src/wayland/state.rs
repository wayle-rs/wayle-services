//! The wayland `Dispatch` target: owns the authoritative local map and
//! republishes it to the public [`Property`](wayle_core::Property) field.
//!
//! Unlike `ext-workspace-v1`, `done` here is emitted per-toplevel rather
//! than once for a whole batch, so [`State::publish`] is called once per
//! affected toplevel rather than at a single global checkpoint.

use std::collections::HashMap;
use std::sync::Arc;

use wayland_client::Connection;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1;
use wayle_core::Property;

use crate::core::Toplevel;

pub(crate) struct State {
    #[allow(dead_code)]
    pub(crate) connection: Connection,
    #[allow(dead_code)]
    pub(crate) manager: ZwlrForeignToplevelManagerV1,
    #[allow(dead_code)]
    pub(crate) seat: WlSeat,

    pub(crate) local_toplevels: HashMap<u32, Arc<Toplevel>>,
    pub(crate) toplevels: Property<HashMap<u32, Arc<Toplevel>>>,
}

impl State {
    pub(crate) fn publish(&self) {
        self.toplevels.set(self.local_toplevels.clone());
    }
}
