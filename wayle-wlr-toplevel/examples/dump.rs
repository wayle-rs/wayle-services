//! Connects to the compositor and dumps the current toplevel (window) state.

use std::time::Duration;

use wayle_wlr_toplevel::WlrToplevelService;

fn main() -> wayle_wlr_toplevel::Result<()> {
    let service = WlrToplevelService::new()?;

    std::thread::sleep(Duration::from_millis(200));

    for toplevel in service.toplevels.get().values() {
        println!(
            "toplevel {} title={:?} app_id={:?} outputs={:?} parent={:?} maximized={} minimized={} activated={} fullscreen={}",
            toplevel.key,
            toplevel.title.get(),
            toplevel.app_id.get(),
            toplevel.outputs.get(),
            toplevel.parent.get(),
            toplevel.is_maximized(),
            toplevel.is_minimized(),
            toplevel.is_activated(),
            toplevel.is_fullscreen(),
        );
    }

    Ok(())
}
