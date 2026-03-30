<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-wallpaper

Wallpaper management with cycling and color extraction support.

[![Crates.io](https://img.shields.io/crates/v/wayle-wallpaper)](https://crates.io/crates/wayle-wallpaper)
[![docs.rs](https://img.shields.io/docsrs/wayle-wallpaper)](https://docs.rs/wayle-wallpaper)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## Installation

```sh
cargo add wayle-wallpaper
```

## Usage

```rust,no_run
use wayle_wallpaper::WallpaperService;
use futures::StreamExt;

async fn example() -> Result<(), wayle_wallpaper::Error> {
    let wp = WallpaperService::new().await?;

    // Snapshot: show wallpaper on each monitor
    for (monitor, state) in wp.monitors.get().iter() {
        println!("{monitor}: {:?}", state.wallpaper);
    }

    // Watch: react to wallpaper changes for theming or logging
    let mut stream = wp.monitors.watch();
    while let Some(monitors) = stream.next().await {
        for (monitor, state) in monitors.iter() {
            println!("{monitor} wallpaper changed to {:?}", state.wallpaper);
        }
    }
    Ok(())
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
