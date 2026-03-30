<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-battery

Battery monitoring via UPower D-Bus with reactive state updates.

[![Crates.io](https://img.shields.io/crates/v/wayle-battery)](https://crates.io/crates/wayle-battery)
[![docs.rs](https://img.shields.io/docsrs/wayle-battery)](https://docs.rs/wayle-battery)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```sh
cargo add wayle-battery
```

## Usage

`BatteryService` monitors UPower's composite DisplayDevice by default. All device properties are reactive `Property<T>` types.

```rust,no_run
use wayle_battery::BatteryService;
use futures::StreamExt;

async fn example() -> Result<(), wayle_battery::Error> {
    let service = BatteryService::new().await?;

    let percentage = service.device.percentage.get();
    let state = service.device.state.get();
    println!("Battery: {percentage}% ({state})");

    let mut stream = service.device.state.watch();
    while let Some(new_state) = stream.next().await {
        println!("State changed: {new_state}");
    }
    Ok(())
}
```

For a specific battery, use `BatteryService::builder().device_path(path).build().await?`.

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
