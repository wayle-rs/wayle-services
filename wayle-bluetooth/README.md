<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-bluetooth

Bluetooth device management and discovery via BlueZ D-Bus.

[![Crates.io](https://img.shields.io/crates/v/wayle-bluetooth)](https://crates.io/crates/wayle-bluetooth)
[![docs.rs](https://img.shields.io/docsrs/wayle-bluetooth)](https://docs.rs/wayle-bluetooth)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```sh
cargo add wayle-bluetooth
```

## Usage

`BluetoothService` exposes adapter state, devices, and discovery controls. State comes from BlueZ (and the kernel's rfkill switches), published as reactive `Property<T>` types that follow changes made by any client; only what BlueZ doesn't publish (the operation this service is performing on a device, its latest failure, and the pairing request BlueZ asked this service) is the service's own. Actions return nothing; their outcome shows up as state.

```rust,no_run
use wayle_bluetooth::BluetoothService;
use futures::StreamExt;

async fn example() -> Result<(), wayle_bluetooth::Error> {
    let bt = BluetoothService::new().await?;

    // Snapshot: list known devices
    for device in bt.devices.get().iter() {
        let info = device.info.get();
        println!("{}: connected={}", info.alias, info.connected);
    }

    // Watch: log the connected devices whenever that changes
    let mut stream = bt.connected.watch();
    while let Some(devices) = stream.next().await {
        for device in devices.iter() {
            println!("{} connected", device.info.get().alias);
        }
    }
    Ok(())
}
```

`enable()` and `disable()` turn Bluetooth on and off the way desktops do: through rfkill, whose block holds until lifted and powers the controller down even when its firmware misbehaves, falling back to powering adapters through BlueZ when `/dev/rfkill` can't be written.

Devices support `connect()`, `disconnect()`, `pair()`, and `forget()`. Each runs in the background: its progress is `info.activity`, its outcome shows up as state (`info.connected`, `info.paired`, the device disappearing), and a failure is recorded in `info.last_error`.

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
