//! Central registrar: the single owner of tray-item state.
//!
//! All tray-item mutations flow through one actor task ([`run_registrar`]) that owns
//! the authoritative ordered list of items. The reactive [`SystemTrayService::items`]
//! property and the `org.kde.StatusNotifierWatcher` D-Bus surface are pure derivations
//! of that list, so the two-sources-of-truth divergence of the old design is gone.
//!
//! Removal is fully reactive: each item runs a lifecycle task that watches an
//! `arg0`-filtered `NameOwnerChanged` for exactly its owner connection and tears the
//! item down when that owner disappears. There is no periodic reconciliation and no
//! polling — the only non-listener query is a single liveness check per item, armed
//! *after* the death watch so the "owner already gone" gap is closed by construction.
//!
//! [`SystemTrayService::items`]: crate::service::SystemTrayService::items

use std::sync::Arc;

use futures::{Stream, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use wayle_core::Property;
use wayle_traits::Reactive;
use zbus::{
    Connection,
    fdo::{DBusProxy, NameOwnerChanged},
    names::{BusName, OwnedUniqueName},
};

use crate::{
    core::item::{LiveTrayItemParams, TrayItem},
    types::{ITEM_OBJECT_PATH, WATCHER_INTERFACE, WATCHER_OBJECT_PATH},
};

/// Stable identity of a tray item: the unique owner connection plus the object path
/// its `StatusNotifierItem` lives at.
///
/// The same connection can register under three service-string shapes — a well-known
/// name (`org.kde.StatusNotifierItem-PID-N`), the object-path form
/// (`{unique}/StatusNotifierItem`), or a bare unique name (`:1.42`, produced by startup
/// discovery). All three resolve to the same `(owner, path)`, so one app is exactly one
/// entry and duplicate icons cannot occur regardless of which shape is used.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct ItemKey {
    owner: OwnedUniqueName,
    path: String,
}

struct Entry {
    key: ItemKey,
    /// The callable service string (becomes `TrayItem::bus_name`) and the value exposed
    /// via the watcher's `RegisteredStatusNotifierItems` property / signals.
    service: String,
    item: Arc<TrayItem>,
    /// Cancels this item's lifecycle task and its property monitor.
    cancel: CancellationToken,
}

/// Messages accepted by the registrar actor.
pub(crate) enum RegistrarMsg {
    /// A registration request. `sender` is `Some` for a live method call or startup
    /// discovery (the owner is known); `None` for a host-mode external `Registered`
    /// signal (the owner must be resolved from the service string).
    Register {
        service: String,
        sender: Option<OwnedUniqueName>,
    },
    /// Host-mode external `StatusNotifierItemUnregistered`: correlate by exact string,
    /// since the external watcher emits the raw string it stored (after owner death, so
    /// the owner is no longer resolvable).
    RemoveByService { service: String },
    /// A prepared item is ready to publish (sent by a lifecycle task).
    Insert {
        key: ItemKey,
        service: String,
        item: Arc<TrayItem>,
        cancel: CancellationToken,
    },
    /// Tear down an item by identity (its owner disappeared).
    Remove { key: ItemKey },
    /// Reply with the current registered service strings (for the D-Bus property).
    Query {
        reply: oneshot::Sender<Vec<String>>,
    },
}

/// Cloneable handle used by producers (the watcher interface, discovery, host-mode
/// signal listeners) to talk to the registrar actor.
#[derive(Clone)]
pub(crate) struct RegistrarHandle {
    tx: mpsc::UnboundedSender<RegistrarMsg>,
}

impl RegistrarHandle {
    /// Requests registration of `service`. `sender` is the resolved owner when known.
    pub(crate) fn register(&self, service: String, sender: Option<OwnedUniqueName>) {
        let _ = self.tx.send(RegistrarMsg::Register { service, sender });
    }

    /// Removes an item correlated by its exact registered string (host mode).
    pub(crate) fn remove_by_service(&self, service: String) {
        let _ = self.tx.send(RegistrarMsg::RemoveByService { service });
    }

    /// Returns the current registered service strings.
    pub(crate) async fn registered_services(&self) -> Vec<String> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(RegistrarMsg::Query { reply }).is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    fn send(&self, msg: RegistrarMsg) {
        let _ = self.tx.send(msg);
    }
}

/// Spawns the registrar actor and returns a handle to it.
///
/// `items` is the reactive property kept in sync (single source of truth). `watcher_conn`
/// is `Some` in watcher mode — the connection owning `org.kde.StatusNotifierWatcher`, used
/// to emit `StatusNotifierItem(Un)Registered`; `None` in host mode.
pub(crate) fn spawn_registrar(
    conn: Connection,
    watcher_conn: Option<Connection>,
    items: Property<Vec<Arc<TrayItem>>>,
    cancellation_token: CancellationToken,
) -> RegistrarHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = RegistrarHandle { tx };
    tokio::spawn(run_registrar(
        rx,
        handle.clone(),
        conn,
        watcher_conn,
        items,
        cancellation_token,
    ));
    handle
}

async fn run_registrar(
    mut rx: mpsc::UnboundedReceiver<RegistrarMsg>,
    handle: RegistrarHandle,
    conn: Connection,
    watcher_conn: Option<Connection>,
    items: Property<Vec<Arc<TrayItem>>>,
    cancellation_token: CancellationToken,
) {
    // Authoritative, insertion-ordered item list (small N; linear lookup is fine, and
    // the stable order keeps the tray from reshuffling on every change).
    let mut entries: Vec<Entry> = Vec::new();

    loop {
        tokio::select! {
            () = cancellation_token.cancelled() => {
                for entry in entries.drain(..) {
                    entry.cancel.cancel();
                }
                return;
            }
            msg = rx.recv() => {
                let Some(msg) = msg else { return };
                match msg {
                    RegistrarMsg::Register { service, sender } => {
                        handle_register(&handle, &conn, &cancellation_token, &entries, service, sender);
                    }
                    RegistrarMsg::Insert { key, service, item, cancel } => {
                        if entries.iter().any(|e| e.key == key) {
                            // Same connection already registered under another shape, or a
                            // concurrent register for the same key won the race.
                            cancel.cancel();
                        } else {
                            entries.push(Entry { key, service: service.clone(), item, cancel });
                            publish(&items, &entries);
                            emit_signal(&watcher_conn, "StatusNotifierItemRegistered", &service).await;
                        }
                    }
                    RegistrarMsg::Remove { key } => {
                        if let Some(pos) = entries.iter().position(|e| e.key == key) {
                            let entry = entries.remove(pos);
                            entry.cancel.cancel();
                            publish(&items, &entries);
                            emit_signal(&watcher_conn, "StatusNotifierItemUnregistered", &entry.service).await;
                        }
                    }
                    RegistrarMsg::RemoveByService { service } => {
                        if let Some(pos) = entries.iter().position(|e| e.service == service) {
                            let entry = entries.remove(pos);
                            entry.cancel.cancel();
                            publish(&items, &entries);
                            // Host mode: the external watcher owns the signals; we don't emit.
                        }
                    }
                    RegistrarMsg::Query { reply } => {
                        let _ = reply.send(entries.iter().map(|e| e.service.clone()).collect());
                    }
                }
            }
        }
    }
}

fn publish(items: &Property<Vec<Arc<TrayItem>>>, entries: &[Entry]) {
    items.set(entries.iter().map(|e| e.item.clone()).collect());
}

async fn emit_signal(watcher_conn: &Option<Connection>, member: &str, service: &str) {
    let Some(conn) = watcher_conn else { return };
    conn.emit_signal(
        None::<()>,
        WATCHER_OBJECT_PATH,
        WATCHER_INTERFACE,
        member,
        &service,
    )
    .await
    .unwrap_or_else(|error| {
        warn!(error = %error, member = %member, service = %service, "cannot emit watcher signal");
    });
}

/// Dispatches a registration request onto a per-item lifecycle task.
///
/// Dedup is intentionally done only against live `entries` here and again at `Insert`
/// time (a duplicate loses the race and is cancelled). There is deliberately no
/// "pending" reservation: a lifecycle that aborts (e.g. a transient `get_live` failure)
/// simply leaves nothing behind, so a re-registration always spawns a fresh, self-healing
/// lifecycle rather than being swallowed by a slot that is about to be released.
fn handle_register(
    handle: &RegistrarHandle,
    conn: &Connection,
    cancellation_token: &CancellationToken,
    entries: &[Entry],
    service: String,
    sender: Option<OwnedUniqueName>,
) {
    match sender {
        // Owner known synchronously (live method call or startup discovery).
        Some(owner) => {
            let (callable, path) = split_registered(&service, &owner);
            let key = ItemKey {
                owner: owner.clone(),
                path,
            };
            if entries.iter().any(|e| e.key == key) {
                return;
            }
            tokio::spawn(item_lifecycle(
                handle.clone(),
                conn.clone(),
                cancellation_token.child_token(),
                callable,
                owner,
                key,
            ));
        }
        // Owner must be resolved from the raw string (host-mode external registration).
        None => {
            tokio::spawn(resolve_then_run(
                handle.clone(),
                conn.clone(),
                cancellation_token.child_token(),
                service,
            ));
        }
    }
}

/// Resolves the owner of a stored service string, then runs its lifecycle.
async fn resolve_then_run(
    handle: RegistrarHandle,
    conn: Connection,
    cancel: CancellationToken,
    service: String,
) {
    let (name_part, path) = split_stored(&service);

    let owner = {
        let Ok(dbus) = DBusProxy::new(&conn).await else {
            return;
        };
        let Ok(bus_name) = BusName::try_from(name_part) else {
            return;
        };
        // Owner-less / unresolvable: drop (fail-closed) — these paths have no protocol
        // ordering guarantee, so a dead name must not be registered.
        match dbus.get_name_owner(bus_name).await {
            Ok(owner) => owner,
            Err(_) => return,
        }
        // `dbus` is dropped here so we don't hold an idle proxy for the item's life.
    };

    let key = ItemKey {
        owner: owner.clone(),
        path: path.to_string(),
    };
    item_lifecycle(handle, conn, cancel, service, owner, key).await;
}

/// Owns one tray item for its whole life: arm the owner-death watch, verify liveness,
/// build the live item, publish it, then tear it down when the owner disappears.
async fn item_lifecycle(
    handle: RegistrarHandle,
    conn: Connection,
    cancel: CancellationToken,
    service: String,
    owner: OwnedUniqueName,
    key: ItemKey,
) {
    let Ok(dbus) = DBusProxy::new(&conn).await else {
        return;
    };

    // Arm the death watch BEFORE the liveness check: any owner loss between the check
    // and arming would otherwise be missed. A unique name only ever loses ownership by
    // disconnecting, so this stream firing == this item is gone.
    let mut death = match dbus
        .receive_name_owner_changed_with_args(&[(0, owner.as_str())])
        .await
    {
        Ok(stream) => stream,
        Err(error) => {
            warn!(error = %error, owner = %owner, "cannot watch owner liveness");
            return;
        }
    };

    // One-shot liveness check (not polling): closes the "owner already dead before the
    // watch was armed" register-after-death race by construction.
    let alive = match BusName::try_from(owner.as_str()) {
        Ok(bus_name) => matches!(dbus.name_has_owner(bus_name).await, Ok(true)),
        Err(_) => true,
    };
    if !alive {
        return;
    }

    let params = LiveTrayItemParams {
        connection: &conn,
        service: service.clone(),
        cancellation_token: &cancel,
    };
    let item = tokio::select! {
        result = TrayItem::get_live(params) => match result {
            Ok(item) => item,
            Err(error) => {
                debug!(error = %error, service = %service, "cannot load tray item");
                return;
            }
        },
        () = wait_owner_gone(&mut death) => return,
        () = cancel.cancelled() => return,
    };

    handle.send(RegistrarMsg::Insert {
        key: key.clone(),
        service,
        item,
        cancel: cancel.clone(),
    });

    // Item is live: wait for its owner to vanish (or for a shutdown/dedup cancel), then
    // ask the registrar to remove it.
    tokio::select! {
        () = wait_owner_gone(&mut death) => {
            handle.send(RegistrarMsg::Remove { key });
        }
        () = cancel.cancelled() => {}
    }
}

/// Resolves once the watched owner loses its bus ownership.
///
/// The stream is `arg0`-filtered to a single unique name, which can only lose ownership
/// by disconnecting, so any `new_owner == None` signal means this item is gone.
async fn wait_owner_gone<S>(stream: &mut S)
where
    S: Stream<Item = NameOwnerChanged> + Unpin,
{
    while let Some(signal) = stream.next().await {
        if let Ok(args) = signal.args()
            && args.new_owner.is_none()
        {
            return;
        }
    }
}

/// Splits the string an app passes to `RegisterStatusNotifierItem` into the callable
/// service string (`TrayItem::bus_name`) and object path, given its resolved owner.
///
/// - `"/StatusNotifierItem"` (object-path form) -> callable `"{owner}/StatusNotifierItem"`.
/// - `"org.kde.StatusNotifierItem-1-1"` (bus-name form) -> callable unchanged, default path.
fn split_registered(service: &str, owner: &OwnedUniqueName) -> (String, String) {
    if service.starts_with('/') {
        (format!("{owner}{service}"), service.to_string())
    } else {
        (service.to_string(), ITEM_OBJECT_PATH.to_string())
    }
}

/// Splits an already-callable stored service string into its owning bus name and path,
/// i.e. the inverse of what is stored (`"{name}/path"` or a bare bus name).
fn split_stored(service: &str) -> (&str, &str) {
    match service.find('/') {
        Some(idx) => (&service[..idx], &service[idx..]),
        None => (service, ITEM_OBJECT_PATH),
    }
}
