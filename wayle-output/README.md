<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-output

Connected Wayland outputs with make, model and serial number.

[![Crates.io](https://img.shields.io/crates/v/wayle-output)](https://crates.io/crates/wayle-output)
[![docs.rs](https://img.shields.io/docsrs/wayle-output)](https://docs.rs/wayle-output)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Serials come from `zwlr_output_manager_v1` when the compositor offers it,
otherwise from the `wl_output` description.

## Installation

```sh
cargo add wayle-output
```

## Usage

```rust,no_run
use wayle_output::OutputService;

let Some(outputs) = OutputService::start() else {
    return;
};

for monitor in outputs.outputs.get() {
    println!("{}: {}", monitor.connector, monitor.description());
}
```
