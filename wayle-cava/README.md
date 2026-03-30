<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-cava

Real-time audio frequency visualization via libcava.

[![Crates.io](https://img.shields.io/crates/v/wayle-cava)](https://crates.io/crates/wayle-cava)
[![docs.rs](https://img.shields.io/docsrs/wayle-cava)](https://docs.rs/wayle-cava)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```sh
cargo add wayle-cava
```

## Usage

`CavaService` captures system audio and produces frequency bar amplitudes. The `values` field updates at the configured framerate (default 60fps).

```rust,no_run
use wayle_cava::{CavaService, InputMethod};
use futures::StreamExt;

async fn example() -> Result<(), wayle_cava::Error> {
    let cava = CavaService::builder()
        .bars(32)
        .framerate(30)
        .input(InputMethod::PipeWire)
        .build()
        .await?;

    let mut stream = cava.values.watch();
    while let Some(values) = stream.next().await {
        println!("Bars: {:?}", &values[..4]);
    }
    Ok(())
}
```

Runtime changes like `set_bars()` and `set_noise_reduction()` restart the capture automatically. The `vendored` feature (on by default) compiles libcava from source.

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
