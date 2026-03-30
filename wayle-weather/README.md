<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-weather

Weather data service with multi-provider support (Open-Meteo, Visual Crossing, WeatherAPI).

[![Crates.io](https://img.shields.io/crates/v/wayle-weather)](https://crates.io/crates/wayle-weather)
[![docs.rs](https://img.shields.io/docsrs/wayle-weather)](https://docs.rs/wayle-weather)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## Installation

```sh
cargo add wayle-weather
```

## Usage

```rust,no_run
use wayle_weather::{WeatherService, WeatherProviderKind, LocationQuery};
use tokio_stream::StreamExt;

async fn example() {
    let weather = WeatherService::builder()
        .provider(WeatherProviderKind::OpenMeteo)
        .location(LocationQuery::city("Tokyo"))
        .build();

    // Snapshot: read current conditions
    if let Some(data) = weather.weather.get().as_ref() {
        println!("{}°C, {:?}", data.current.temperature.celsius(), data.current.condition);
    }

    // Watch: update display when weather refreshes
    let mut stream = weather.weather.watch();
    while let Some(data) = stream.next().await {
        if let Some(w) = data.as_ref() {
            println!("Updated: {}°C, {:?}", w.current.temperature.celsius(), w.current.condition);
        }
    }
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
