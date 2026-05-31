<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-niri

Reactive bindings to the niri compositor via IPC.

[![Crates.io](https://img.shields.io/crates/v/wayle-niri)](https://crates.io/crates/wayle-niri)
[![docs.rs](https://img.shields.io/docsrs/wayle-niri)](https://docs.rs/wayle-niri)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```sh
cargo add wayle-niri
```

## Usage

[`NiriService`] connects to niri's IPC socket (from `$NIRI_SOCKET`), subscribes
to the event stream, and exposes the current window / workspace state through
`Property<T>` fields.

```rust,no_run
use wayle_niri::NiriService;
use futures::StreamExt;

async fn example() -> wayle_niri::Result<()> {
    let service = NiriService::new().await?;

    for workspace in service.workspaces.get().values() {
        println!("workspace {} on {:?}", workspace.id.get(), workspace.output.get());
    }

    let mut focused = service.focused_window_id.watch();
    while let Some(window_id) = focused.next().await {
        println!("focused window id: {window_id:?}");
    }
    Ok(())
}
```

## Actions

Send typed actions through the same service instance. `action(Action)` is the
generic entry point; convenience wrappers cover the common cases.

```rust,no_run
use wayle_niri::{NiriService, WorkspaceReferenceArg};

async fn switch_and_spawn(service: &NiriService) -> wayle_niri::Result<()> {
    service.focus_workspace(WorkspaceReferenceArg::Index(2)).await?;
    service.spawn(vec!["alacritty".into()]).await?;
    Ok(())
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
