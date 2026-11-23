# wayle-wlr-toplevel

Reactive bindings to the [`wlr-foreign-toplevel-management-unstable-v1`](https://wayland.app/protocols/wlr-foreign-toplevel-management-unstable-v1)
Wayland protocol.

Lists open windows (title, app-id, output assignment, maximized/minimized/
activated/fullscreen state) and lets a client request activation or
closing them - the building blocks for a taskbar or an alt-tab style
window switcher.

This is a wlroots-specific protocol (not the newer, read-only
`ext-foreign-toplevel-list-v1`), chosen because it is the only one of the
two that actually supports activating a window. It works on any
wlroots-based compositor (Sway, etc).

```rust,no_run
use wayle_wlr_toplevel::WlrToplevelService;

let service = WlrToplevelService::new()?;

for toplevel in service.toplevels.get().values() {
    println!("{:?}", toplevel.title.get());
}
# Ok::<(), wayle_wlr_toplevel::Error>(())
```
