<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-notification

Desktop notification service implementing the freedesktop.org Desktop Notifications spec.

[![Crates.io](https://img.shields.io/crates/v/wayle-notification)](https://crates.io/crates/wayle-notification)
[![docs.rs](https://img.shields.io/docsrs/wayle-notification)](https://docs.rs/wayle-notification)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## Installation

```sh
cargo add wayle-notification
```

## Usage

```rust,no_run
use wayle_notification::NotificationService;
use futures::StreamExt;

async fn example() -> Result<(), wayle_notification::Error> {
    let service = NotificationService::new().await?;

    let count = service.notifications.get().len();
    println!("{count} notifications");

    let mut stream = service.notifications.watch();
    while let Some(notifications) = stream.next().await {
        println!("{} notifications", notifications.len());
    }

    Ok(())
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
