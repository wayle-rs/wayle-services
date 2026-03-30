<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-network

WiFi and wired network management via NetworkManager D-Bus.

[![Crates.io](https://img.shields.io/crates/v/wayle-network)](https://crates.io/crates/wayle-network)
[![docs.rs](https://img.shields.io/docsrs/wayle-network)](https://docs.rs/wayle-network)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## Installation

```sh
cargo add wayle-network
```

## Usage

```rust,no_run
use wayle_network::NetworkService;
use futures::StreamExt;

async fn example() -> Result<(), wayle_network::Error> {
    let net = NetworkService::new().await?;

    // wifi is None if no adapter is present
    let Some(wifi) = net.wifi.get() else {
        println!("no WiFi adapter");
        return Ok(());
    };

    println!("SSID: {:?}, signal: {:?}", wifi.ssid.get(), wifi.strength.get());

    // react to connectivity changes
    let mut stream = wifi.connectivity.watch();
    while let Some(status) = stream.next().await {
        println!("connectivity: {status:?}");
    }
    Ok(())
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
