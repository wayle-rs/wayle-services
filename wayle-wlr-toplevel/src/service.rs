//! The [`WlrToplevelService`] type: connects to the compositor, runs the
//! protocol's event loop on a dedicated thread, and exposes the resulting
//! state through a [`Property`](wayle_core::Property) field.

use std::{collections::HashMap, sync::Arc};

use wayland_client::{Connection, globals::registry_queue_init, protocol::wl_seat::WlSeat};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1;
use wayle_core::Property;

use crate::{core::Toplevel, error::Result, wayland::state::State};

/// Reactive bindings to any compositor implementing
/// `wlr-foreign-toplevel-management-unstable-v1`.
///
/// Unlike compositor-specific services (`wayle-niri`, `wayle-hyprland`),
/// this works on any wlroots-based Wayland compositor that advertises the
/// `zwlr_foreign_toplevel_manager_v1` global, including Sway.
pub struct WlrToplevelService {
    #[allow(dead_code)]
    manager: ZwlrForeignToplevelManagerV1,
    connection: Connection,
    seat: WlSeat,

    /// All toplevels (windows), keyed by their wayland object id.
    pub toplevels: Property<HashMap<u32, Arc<Toplevel>>>,
}

impl WlrToplevelService {
    /// Connects to the compositor and binds `zwlr_foreign_toplevel_manager_v1`.
    ///
    /// # Errors
    ///
    /// - [`Error::ConnectionFailed`](crate::Error::ConnectionFailed) if no
    ///   Wayland display is reachable.
    /// - [`Error::NotSupported`](crate::Error::NotSupported) if the
    ///   compositor does not advertise `zwlr_foreign_toplevel_manager_v1`
    ///   (or `wl_seat`).
    /// - [`Error::Dispatch`](crate::Error::Dispatch) if the initial
    ///   round-trip fails.
    /// - [`Error::ThreadSpawnFailed`](crate::Error::ThreadSpawnFailed) if the
    ///   background event-loop thread cannot be spawned.
    pub fn new() -> Result<Arc<Self>> {
        let connection = Connection::connect_to_env()?;
        let (globals, mut event_queue) = registry_queue_init::<State>(&connection)?;
        let qh = event_queue.handle();

        let manager: ZwlrForeignToplevelManagerV1 = globals.bind(&qh, 1..=3, ())?;
        let seat: WlSeat = globals.bind(&qh, 1..=9, ())?;

        let toplevels = Property::new(HashMap::new());

        let mut state = State {
            connection: connection.clone(),
            manager: manager.clone(),
            seat: seat.clone(),
            local_toplevels: HashMap::new(),
            toplevels: toplevels.clone(),
        };

        // Drive the initial registry/manager round-trip synchronously so the
        // first snapshot is available as soon as `new()` returns.
        event_queue.roundtrip(&mut state)?;

        std::thread::Builder::new()
            .name("wayle-wlr-toplevel".into())
            .spawn(move || {
                loop {
                    if event_queue.blocking_dispatch(&mut state).is_err() {
                        break;
                    }
                }
            })?;

        Ok(Arc::new(Self {
            manager,
            connection,
            seat,
            toplevels,
        }))
    }

    /// Looks up a toplevel by its wayland object id.
    pub fn toplevel(&self, key: u32) -> Option<Arc<Toplevel>> {
        self.toplevels.get().get(&key).cloned()
    }

    /// Requests that the compositor activate the given toplevel on this
    /// service's seat, then flushes the connection.
    ///
    /// There is no guarantee the compositor honors the request; watch
    /// [`Toplevel::state`](crate::core::Toplevel::state) to observe the
    /// actual outcome.
    pub fn activate_toplevel(&self, key: u32) {
        let Some(toplevel) = self.toplevel(key) else {
            return;
        };
        toplevel.handle.activate(&self.seat);
        let _ = self.connection.flush();
    }

    /// Requests that the compositor close the given toplevel, then flushes
    /// the connection.
    pub fn close_toplevel(&self, key: u32) {
        let Some(toplevel) = self.toplevel(key) else {
            return;
        };
        toplevel.close();
        let _ = self.connection.flush();
    }
}
