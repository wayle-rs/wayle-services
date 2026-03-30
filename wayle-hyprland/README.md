<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-hyprland

Reactive bindings to Hyprland compositor state and events via IPC.

[![Crates.io](https://img.shields.io/crates/v/wayle-hyprland)](https://crates.io/crates/wayle-hyprland)
[![docs.rs](https://img.shields.io/docsrs/wayle-hyprland)](https://docs.rs/wayle-hyprland)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```sh
cargo add wayle-hyprland
```

## Usage

`HyprlandService` connects to Hyprland's Unix sockets and exposes `workspaces`, `clients`, `monitors`, and `layers` as reactive `Property<T>` fields.

```rust,no_run
use wayle_hyprland::HyprlandService;
use futures::StreamExt;

async fn example() -> wayle_hyprland::Result<()> {
    let service = HyprlandService::new().await?;

    // Snapshot: print current workspace names
    for ws in service.workspaces.get().iter() {
        println!("Workspace {} on {}", ws.name.get(), ws.id.get());
    }

    // Watch: react to workspace layout changes
    let mut stream = service.workspaces.watch();
    while let Some(workspaces) = stream.next().await {
        let names: Vec<_> = workspaces.iter().map(|ws| ws.name.get()).collect();
        println!("Workspaces changed: {names:?}");
    }
    Ok(())
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
