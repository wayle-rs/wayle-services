<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-mango

Reactive MangoWM compositor state and event streaming.

[![Crates.io](https://img.shields.io/crates/v/wayle-mango)](https://crates.io/crates/wayle-mango)
[![docs.rs](https://img.shields.io/docsrs/wayle-mango)](https://docs.rs/wayle-mango)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```sh
cargo add wayle-mango
```

## Usage

```rust,no_run
use futures::StreamExt;
use wayle_mango::MangoService;

async fn example() -> wayle_mango::Result<()> {
    let service = MangoService::new().await?;

    let layout = service.keyboard_layout.get();
    println!("layout: {layout:?}");

    let mut focused = service.focused_client.watch();
    while let Some(client) = focused.next().await {
        println!("focused: {client:?}");
    }
    Ok(())
}
```

State is exposed as [`Property`](https://docs.rs/wayle-core) values:

- `.get()` returns the current value.
- `.watch()` yields a `Stream` of changes.

Mango is dwm-derived, so each monitor has a fixed set of tags rather than a
growable workspace list. Read them from `service.monitors`.

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
