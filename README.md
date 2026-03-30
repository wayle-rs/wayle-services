<p align="center">
  <img src="assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-services

[![CI](https://img.shields.io/github/actions/workflow/status/wayle-rs/wayle-services/ci.yml?branch=master)](https://github.com/wayle-rs/wayle-services/actions)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/wayle-rs/wayle-services/blob/master/LICENSE)

Reactive system service crates for Linux desktops. Each service exposes its state as [`Property<T>`](https://docs.rs/wayle-core/latest/wayle_core/struct.Property.html) fields you can `.get()` or `.watch()` for changes.

## Services

| Crate | Description |
|-------|-------------|
| [wayle-audio](https://docs.rs/wayle-audio) | PulseAudio devices and streams |
| [wayle-battery](https://docs.rs/wayle-battery) | Battery monitoring via UPower |
| [wayle-bluetooth](https://docs.rs/wayle-bluetooth) | Bluetooth device management via BlueZ |
| [wayle-brightness](https://docs.rs/wayle-brightness) | Backlight control for internal displays |
| [wayle-cava](https://docs.rs/wayle-cava) | Real-time audio frequency visualization |
| [wayle-hyprland](https://docs.rs/wayle-hyprland) | Hyprland compositor state and events |
| [wayle-media](https://docs.rs/wayle-media) | MPRIS media player control |
| [wayle-network](https://docs.rs/wayle-network) | WiFi and wired network management |
| [wayle-notification](https://docs.rs/wayle-notification) | Desktop notification daemon |
| [wayle-power-profiles](https://docs.rs/wayle-power-profiles) | Power profile switching |
| [wayle-sysinfo](https://docs.rs/wayle-sysinfo) | CPU, memory, disk, and network metrics |
| [wayle-systray](https://docs.rs/wayle-systray) | System tray via StatusNotifier |
| [wayle-wallpaper](https://docs.rs/wayle-wallpaper) | Wallpaper management with color extraction |
| [wayle-weather](https://docs.rs/wayle-weather) | Weather data with multi-provider support |

## Internals

| Crate | Description |
|-------|-------------|
| [wayle-core](https://docs.rs/wayle-core) | `Property<T>` reactive primitive and D-Bus macros |
| [wayle-traits](https://docs.rs/wayle-traits) | Shared service monitoring traits |

## Quick start

```rust,no_run
use futures::StreamExt;
use wayle_audio::AudioService;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let audio = AudioService::new().await?;

    // snapshot
    if let Some(device) = audio.default_output.get() {
        println!("default output: {:?}", device);
    }

    // react to changes
    let mut stream = audio.default_output.watch();
    while let Some(device) = stream.next().await {
        println!("default output changed: {:?}", device);
    }

    Ok(())
}
```

## Credits

Logo by [@M70v](https://www.instagram.com/m70v.art/).

## License

MIT
