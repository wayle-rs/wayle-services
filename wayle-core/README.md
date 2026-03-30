<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-core

Reactive state primitives and D-Bus utilities shared across Wayle services.

[![Crates.io](https://img.shields.io/crates/v/wayle-core)](https://crates.io/crates/wayle-core)
[![docs.rs](https://img.shields.io/docsrs/wayle-core)](https://docs.rs/wayle-core)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```sh
cargo add wayle-core
```

## Usage

Wrap any value in a `Property<T>` to get snapshot reads and change streams.

```rust,no_run
use wayle_core::Property;
use futures::stream::StreamExt;

async fn example() {
    let brightness = Property::new(75u32);
    brightness.set(100);

    let mut changes = brightness.watch();
    while let Some(level) = changes.next().await {
        println!("{level}");
    }
}
```


## Features

- `schema` enables `schemars::JsonSchema` support on `Property<T>`

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
