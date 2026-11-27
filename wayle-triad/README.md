<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-triad

Reactive bindings to Triad compositor state and events via IPC.

[![Crates.io](https://img.shields.io/crates/v/wayle-triad)](https://crates.io/crates/wayle-triad)
[![docs.rs](https://img.shields.io/docsrs/wayle-triad)](https://docs.rs/wayle-triad)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```sh
cargo add wayle-triad
```

## Usage

`TriadService` connects to Triad's IPC socket, subscribes to compositor
events, and exposes workspaces, windows, outputs, and keyboard layouts as
reactive `Property<T>` fields.

```rust,no_run
use futures::StreamExt;
use wayle_triad::TriadService;

async fn example() -> wayle_triad::Result<()> {
    let service = TriadService::new().await?;

    for workspace in service.workspaces.get().values() {
        println!(
            "workspace {} on {:?}",
            workspace.idx.get(),
            workspace.output.get()
        );
    }

    let mut focused = service.focused_window_id.watch();
    while let Some(window_id) = focused.next().await {
        println!("focused window id: {window_id:?}");
    }

    Ok(())
}
```

## Actions

Send compositor actions through the same service instance. Convenience
wrappers cover workspace focus, window focus, window close, command spawn,
keyboard layout switching, overview toggle, and layout switching.

```rust,no_run
use wayle_triad::TriadService;

async fn switch_and_spawn(service: &TriadService) -> wayle_triad::Result<()> {
    service.focus_workspace(2).await?;
    service.spawn(vec!["alacritty".into()]).await?;
    Ok(())
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
