use std::time::Duration;

use futures::stream::{self, StreamExt};
use tracing::{debug, info, warn};
use zbus::{Connection, fdo::DBusProxy, names::OwnedUniqueName};

use crate::{proxy::status_notifier_item::StatusNotifierItemProxy, registrar::RegistrarHandle};

const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const PROBE_CONCURRENCY: usize = 16;

/// One-shot recovery, run once when we acquire the watcher name.
///
/// Some applications register with a *previous* watcher and never re-register when a new
/// one appears, so they would be invisible after a watcher restart. We enumerate the live
/// unique connections, probe each for a `StatusNotifierItem`, and hand any we find to the
/// registrar. This is reactive to startup, not a poll: it runs exactly once and never
/// repeats. Items it registers are keyed by owner just like live registrations, so an app
/// recovered here and the same app re-registering later collapse to one entry.
pub(crate) fn spawn_startup_discovery(
    conn: Connection,
    registrar: RegistrarHandle,
    own_names: Vec<String>,
) {
    tokio::spawn(async move {
        discover(&conn, &registrar, &own_names).await;
    });
}

async fn discover(conn: &Connection, registrar: &RegistrarHandle, own_names: &[String]) {
    let Ok(dbus) = DBusProxy::new(conn).await else {
        warn!("cannot create DBus proxy for startup discovery");
        return;
    };
    let Ok(names) = dbus.list_names().await else {
        warn!("cannot list bus names for startup discovery");
        return;
    };

    let candidates: Vec<String> = names
        .into_iter()
        .map(|name| name.as_str().to_string())
        .filter(|name| name.starts_with(':') && !own_names.iter().any(|own| own == name))
        .collect();

    debug!(count = candidates.len(), "probing bus for orphaned SNI items");

    // Probe concurrently: a single wedged peer must not stall recovery of the rest.
    let found: Vec<String> = stream::iter(candidates)
        .map(|name| async move {
            if probe_sni(conn, &name).await {
                Some(name)
            } else {
                None
            }
        })
        .buffer_unordered(PROBE_CONCURRENCY)
        .filter_map(|result| async move { result })
        .collect()
        .await;

    for name in &found {
        // A candidate is itself a unique connection name, so it is its own owner.
        if let Ok(owner) = OwnedUniqueName::try_from(name.as_str()) {
            registrar.register(name.clone(), Some(owner));
        }
    }

    if !found.is_empty() {
        info!(count = found.len(), "recovered orphaned SNI items");
    }
}

async fn probe_sni(conn: &Connection, bus_name: &str) -> bool {
    let probe = async {
        let proxy = StatusNotifierItemProxy::builder(conn)
            .destination(bus_name)?
            .build()
            .await?;
        proxy.id().await
    };

    matches!(tokio::time::timeout(PROBE_TIMEOUT, probe).await, Ok(Ok(_)))
}
