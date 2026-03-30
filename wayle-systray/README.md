<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-systray

System tray management via the StatusNotifier (SNI) and DBusMenu protocols.

[![Crates.io](https://img.shields.io/crates/v/wayle-systray)](https://crates.io/crates/wayle-systray)
[![docs.rs](https://img.shields.io/docsrs/wayle-systray)](https://docs.rs/wayle-systray)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## Installation

```sh
cargo add wayle-systray
```

## Usage

```rust,no_run
use wayle_systray::SystemTrayService;
use futures::StreamExt;

async fn example() -> Result<(), wayle_systray::error::Error> {
    let service = SystemTrayService::new().await?;

    for item in service.items.get().iter() {
        println!("{}: {}", item.id.get(), item.title.get());
    }

    let mut stream = service.items.watch();
    while let Some(items) = stream.next().await {
        println!("{} tray items", items.len());
    }

    Ok(())
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
