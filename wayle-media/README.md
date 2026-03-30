<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-media

MPRIS media player control and playback tracking via D-Bus.

[![Crates.io](https://img.shields.io/crates/v/wayle-media)](https://crates.io/crates/wayle-media)
[![docs.rs](https://img.shields.io/docsrs/wayle-media)](https://docs.rs/wayle-media)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## Installation

```sh
cargo add wayle-media
```

## Usage

```rust,no_run
use wayle_media::MediaService;
use futures::StreamExt;

async fn example() -> Result<(), wayle_media::Error> {
    let media = MediaService::new().await?;

    if let Some(player) = media.active_player.get() {
        println!("{}: {}", player.identity.get(), player.metadata.title.get());
    }

    let mut stream = media.active_player.watch();
    while let Some(player) = stream.next().await {
        match player {
            Some(p) => println!("{} playing: {}", p.identity.get(), p.metadata.title.get()),
            None => println!("No active player"),
        }
    }
    Ok(())
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
