<p align="center">
  <img src="https://raw.githubusercontent.com/wayle-rs/wayle-services/master/assets/wayle-services.svg" width="200" alt="Wayle">
</p>

# wayle-sysinfo

CPU, memory, disk, network, and NVIDIA GPU metrics via polling-based background tasks.

[![Crates.io](https://img.shields.io/crates/v/wayle-sysinfo)](https://crates.io/crates/wayle-sysinfo)
[![docs.rs](https://img.shields.io/docsrs/wayle-sysinfo)](https://docs.rs/wayle-sysinfo)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## Installation

```sh
cargo add wayle-sysinfo
```

## Usage

```rust,no_run
use wayle_sysinfo::SysinfoService;
use futures::StreamExt;

async fn example() {
    let service = SysinfoService::builder().build();

    let cpu = service.cpu.get();
    println!("CPU: {:.1}%", cpu.usage_percent);

    let memory = service.memory.get();
    println!("Memory: {:.1}%", memory.usage_percent);

    let gpu = service.gpu.get();
    println!(
        "GPUs: {} (avg util: {:.1}%)",
        gpu.total_count,
        gpu.average_utilization_percent
    );

    let mut stream = service.cpu.watch();
    while let Some(cpu) = stream.next().await {
        println!("CPU changed: {:.1}%", cpu.usage_percent);
    }
}
```

## License

MIT

Part of [wayle-services](https://github.com/wayle-rs/wayle-services).
