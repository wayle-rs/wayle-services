# wayle-ext-workspace

Reactive bindings to the [`ext-workspace-v1`](https://wayland.app/protocols/ext-workspace-v1)
Wayland staging protocol.

Unlike `wayle-niri` or `wayle-hyprland`, this crate talks directly to the
compositor over Wayland rather than a compositor-specific IPC socket, so it
works on any compositor that advertises the `ext_workspace_manager_v1`
global - including wlroots-based compositors such as Sway, which have no
dedicated `wayle-*` service crate.

```rust,no_run
use wayle_ext_workspace::ExtWorkspaceService;

let service = ExtWorkspaceService::new()?;

for workspace in service.workspaces.get().values() {
    println!("{:?}", workspace.name.get());
}
# Ok::<(), wayle_ext_workspace::Error>(())
```
