<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-traits

Shared traits for Wayle service monitoring and lifecycle.

[![Crates.io](https://img.shields.io/crates/v/wayle-traits)](https://crates.io/crates/wayle-traits)
[![docs.rs](https://img.shields.io/docsrs/wayle-traits)](https://docs.rs/wayle-traits)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```sh
cargo add wayle-traits
```

## Usage

Implement `ServiceMonitoring` for background state watchers, `Static` for one-shot fetches, or `Reactive` for services that support both snapshot and live-updating access.

```rust,no_run
use wayle_traits::{Reactive, Static};
use std::sync::Arc;

struct MyService;

impl Static for MyService {
    type Error = String;
    type Context<'a> = &'a str;

    async fn get(context: Self::Context<'_>) -> Result<Self, Self::Error> {
        Ok(MyService)
    }
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
