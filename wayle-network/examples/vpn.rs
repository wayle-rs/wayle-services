//! Manual VPN smoke test.
//!
//! Lists VPN profiles, prints the current state, and watches for changes.
//! Pass a profile id as the first argument to activate it on startup, and
//! Ctrl-C deactivates everything before exiting.
//!
//! Usage:
//!     cargo run -p wayle-network --example vpn
//!     cargo run -p wayle-network --example vpn -- <profile-id>

use std::env;

use futures::StreamExt;
use wayle_network::NetworkService;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let net = NetworkService::new().await?;
    let vpn = net.vpn.clone();

    println!("== VPN profiles ==");
    let profiles = vpn.connections.get();
    if profiles.is_empty() {
        println!("(none — import one with `nmcli connection import ...`)");
    }
    for c in &profiles {
        println!(
            "  {:<24} type={:?} path={}",
            c.id.get(),
            c.connection_type.get(),
            c.object_path
        );
    }

    println!(
        "\ninitial: connectivity={:?} active={} banner={:?}",
        vpn.connectivity.get(),
        vpn.active_connections.get().len(),
        vpn.banner.get()
    );

    if let Some(target) = env::args().nth(1) {
        match profiles.iter().find(|c| c.id.get() == target) {
            Some(profile) => {
                println!("\nactivating `{target}`...");
                let active = vpn.connect(profile.object_path.clone()).await?;
                println!("active connection: {active}");
            }
            None => {
                eprintln!("\nno profile with id `{target}` — skipping activation");
            }
        }
    }

    println!("\nwatching changes (Ctrl-C to exit)...");
    let mut stream = vpn.watch();
    loop {
        tokio::select! {
            Some(v) = stream.next() => {
                println!(
                    "[update] connectivity={:?} active={} banner={:?}",
                    v.connectivity.get(),
                    v.active_connections.get().len(),
                    v.banner.get()
                );
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    println!("\nshutting down — disconnecting all VPNs");
    if let Err(e) = vpn.disconnect_all().await {
        eprintln!("disconnect_all failed: {e}");
    }
    Ok(())
}
