<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-power-profiles

Power profile switching and monitoring via power-profiles-daemon D-Bus.

[![Crates.io](https://img.shields.io/crates/v/wayle-power-profiles)](https://crates.io/crates/wayle-power-profiles)
[![docs.rs](https://img.shields.io/docsrs/wayle-power-profiles)](https://docs.rs/wayle-power-profiles)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## Installation

```sh
cargo add wayle-power-profiles
```

## Usage

```rust,no_run
use wayle_power_profiles::PowerProfilesService;
use futures::StreamExt;

async fn example() -> Result<(), wayle_power_profiles::Error> {
    let service = PowerProfilesService::new().await?;

    // Snapshot: check the current profile
    let profile = service.power_profiles.active_profile.get();
    println!("Current profile: {profile}");

    // Watch: log whenever the power profile switches
    let mut stream = service.power_profiles.active_profile.watch();
    while let Some(profile) = stream.next().await {
        println!("Profile switched to: {profile}");
    }
    Ok(())
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
