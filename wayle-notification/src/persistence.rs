use std::{
    collections::HashMap,
    env, fs,
    sync::{Arc, Mutex},
};

use chrono::Utc;
use derive_more::Debug;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};
use zbus::zvariant::{
    LE, OwnedValue, Value,
    serialized::{Context, Data},
    to_bytes,
};

use crate::{
    core::{
        notification::Notification,
        types::{
            Actions, Alert, Classification, Content, FreedesktopDispatch, GtkAction, GtkDispatch,
            Image, Lifecycle, NotificationId, NotificationSource, Origin, PortalAction,
            PortalDispatch, Presentation, Timeout,
        },
    },
    error::Error,
    types::{ButtonPurpose, ClosedReason},
};

/// On-disk form of a [`Notification`]. The display facets serialize verbatim (they are plain,
/// transport-free data); only the `dispatch` needs special handling, because its action
/// targets are arbitrary GVariants that don't survive a naive JSON round-trip — those go
/// through [`StoredSource`]. The whole struct is persisted as a single JSON blob, so adding a
/// facet field never needs a schema migration.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredNotification {
    pub id: NotificationId,
    pub received_ms: i64,
    #[serde(default)]
    pub close_reason: Option<ClosedReason>,
    pub origin: Origin,
    pub content: Content,
    #[serde(default)]
    pub image: Option<Image>,
    pub actions: Actions,
    #[serde(default)]
    pub alert: Alert,
    pub classification: Classification,
    pub lifecycle: Lifecycle,
    pub presentation: Presentation,
    pub dispatch: StoredSource,
}

impl From<&Notification> for StoredNotification {
    fn from(notification: &Notification) -> Self {
        let view = notification.view.get();
        Self {
            id: notification.id,
            received_ms: view.received.timestamp_millis(),
            close_reason: view.close_reason,
            origin: view.origin,
            content: view.content,
            image: view.image,
            actions: view.actions,
            alert: view.alert,
            classification: view.classification,
            lifecycle: view.lifecycle,
            presentation: view.presentation,
            dispatch: stored_source(&notification.dispatch.get()),
        }
    }
}

/// On-disk form of [`NotificationSource`]. Kept separate from the runtime type so action
/// targets serialize through the wire-format encoding proven to round-trip arbitrary
/// `OwnedValue`s (see [`encode_target`]).
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredSource {
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fdo: Option<StoredFdoDispatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gtk: Option<StoredGtkDispatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    portal: Option<StoredPortalDispatch>,
}

#[derive(Serialize, Deserialize, Default)]
struct StoredFdoDispatch {
    wire_id: u32,
    /// The session bus GUID this notification was created under, so a restart can tell whether
    /// its wire id + owner still belong to the live session.
    #[serde(default)]
    session_id: String,
    /// The owning connection's unique name (directed-signal target). Restored as-is; the daemon
    /// clears it at startup when `session_id` differs from the current bus (a prior session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredGtkDispatch {
    app_id: String,
    gtk_id: String,
    default_action: Option<StoredAction>,
    button_actions: HashMap<String, StoredAction>,
}

#[derive(Serialize, Deserialize)]
struct StoredPortalDispatch {
    app_id: String,
    portal_id: String,
    default_action: Option<StoredAction>,
    button_actions: HashMap<String, StoredAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply_action: Option<StoredAction>,
}

#[derive(Serialize, Deserialize)]
struct StoredAction {
    name: String,
    /// The action target, stored as its D-Bus wire encoding (a self-describing variant)
    /// held as a JSON byte array. See [`encode_target`] for why the encoding is bytes and
    /// not `serde_json` of the `OwnedValue` directly.
    target: Option<String>,
    /// The button purpose string (portal only). Defaulted for rows written before it
    /// existed, and for GTK actions which have no purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    purpose: Option<String>,
}

fn stored_source(source: &NotificationSource) -> StoredSource {
    match source {
        NotificationSource::Freedesktop(dispatch) => StoredSource {
            kind: String::from("fdo"),
            fdo: Some(StoredFdoDispatch {
                wire_id: dispatch.wire_id,
                session_id: dispatch.session_id.clone(),
                owner: dispatch.owner.clone(),
            }),
            gtk: None,
            portal: None,
        },
        NotificationSource::Gtk(dispatch) => StoredSource {
            kind: String::from("gtk"),
            fdo: None,
            gtk: Some(StoredGtkDispatch {
                app_id: dispatch.app_id.clone(),
                gtk_id: dispatch.gtk_id.clone(),
                default_action: dispatch.default_action.as_ref().map(stored_action),
                button_actions: dispatch
                    .button_actions
                    .iter()
                    .map(|(key, action)| (key.clone(), stored_action(action)))
                    .collect(),
            }),
            portal: None,
        },
        NotificationSource::Portal(dispatch) => StoredSource {
            kind: String::from("portal"),
            fdo: None,
            gtk: None,
            portal: Some(StoredPortalDispatch {
                app_id: dispatch.app_id.clone(),
                portal_id: dispatch.portal_id.clone(),
                default_action: dispatch.default_action.as_ref().map(stored_portal_action),
                button_actions: dispatch
                    .button_actions
                    .iter()
                    .map(|(key, action)| (key.clone(), stored_portal_action(action)))
                    .collect(),
                reply_action: dispatch.reply_action.as_ref().map(stored_portal_action),
            }),
        },
    }
}

pub(crate) fn source_from_stored(stored: StoredSource) -> NotificationSource {
    match (stored.kind.as_str(), stored.gtk, stored.portal) {
        ("gtk", Some(gtk), _) => NotificationSource::Gtk(GtkDispatch {
            app_id: gtk.app_id,
            gtk_id: gtk.gtk_id,
            default_action: gtk.default_action.map(gtk_action_from_stored),
            button_actions: gtk
                .button_actions
                .into_iter()
                .map(|(key, action)| (key, gtk_action_from_stored(action)))
                .collect(),
        }),
        ("portal", _, Some(portal)) => NotificationSource::Portal(PortalDispatch {
            app_id: portal.app_id,
            portal_id: portal.portal_id,
            default_action: portal.default_action.map(portal_action_from_stored),
            button_actions: portal
                .button_actions
                .into_iter()
                .map(|(key, action)| (key, portal_action_from_stored(action)))
                .collect(),
            reply_action: portal.reply_action.map(portal_action_from_stored),
        }),
        // Freedesktop, or any legacy/unknown row: restore the wire id + session if present,
        // else a default (inert) dispatch.
        _ => {
            let fdo = stored.fdo.unwrap_or_default();
            NotificationSource::Freedesktop(FreedesktopDispatch {
                wire_id: fdo.wire_id,
                session_id: fdo.session_id,
                owner: fdo.owner,
            })
        }
    }
}

fn stored_action(action: &GtkAction) -> StoredAction {
    StoredAction {
        name: action.name.clone(),
        target: action.target.as_ref().and_then(encode_target),
        // GTK actions have no button purpose.
        purpose: None,
    }
}

fn gtk_action_from_stored(stored: StoredAction) -> GtkAction {
    GtkAction {
        name: stored.name,
        target: stored.target.as_deref().and_then(decode_target),
    }
}

fn stored_portal_action(action: &PortalAction) -> StoredAction {
    StoredAction {
        name: action.name.clone(),
        target: action.target.as_ref().and_then(encode_target),
        purpose: action.purpose.as_ref().map(|purpose| purpose.as_str().to_owned()),
    }
}

fn portal_action_from_stored(stored: StoredAction) -> PortalAction {
    PortalAction {
        name: stored.name,
        target: stored.target.as_deref().and_then(decode_target),
        purpose: stored
            .purpose
            .as_deref()
            .and_then(|purpose| purpose.parse::<ButtonPurpose>().ok()),
    }
}

/// Serializes an action target to the D-Bus wire format (a self-describing variant that
/// embeds its own signature) held as a JSON byte array.
///
/// The obvious `serde_json::to_string(&owned_value)` is *not* reversible: `OwnedValue`'s
/// deserializer rebuilds borrowed scalars/strings from JSON but cannot reconstruct a
/// `Structure` or other composite variant — it needs the D-Bus type system, not JSON's.
/// So a structured target such as a `(yay)` serialized fine yet came back as `None` on
/// reload, and the action was then dispatched with an empty parameter, which the app
/// rejects ("expected type (yay) but got type ()"). The wire encoding carries the full
/// signature, so any GVariant an app attaches survives store→load intact.
fn encode_target(target: &OwnedValue) -> Option<String> {
    let value: &Value = target;
    let data = to_bytes(Context::new_dbus(LE, 0), value).ok()?;
    serde_json::to_string(&*data).ok()
}

/// Inverse of [`encode_target`]. Falls back to the legacy `serde_json`-of-`OwnedValue`
/// encoding so targets persisted before the wire-format switch (scalars/strings, which
/// that path could still read) keep working across the upgrade.
fn decode_target(stored: &str) -> Option<OwnedValue> {
    if let Ok(bytes) = serde_json::from_str::<Vec<u8>>(stored) {
        let data = Data::new(bytes, Context::new_dbus(LE, 0));
        if let Ok((value, _)) = data.deserialize::<Value<'_>>() {
            return OwnedValue::try_from(value).ok();
        }
    }
    serde_json::from_str::<OwnedValue>(stored).ok()
}

#[derive(Debug, Clone)]
pub(crate) struct NotificationStore {
    #[debug(skip)]
    connection: Arc<Mutex<Connection>>,
}

impl NotificationStore {
    #[instrument(err)]
    pub fn new() -> Result<Self, Error> {
        let home = env::var("HOME")
            .map_err(|_| Error::DatabaseError(String::from("HOME environment variable not set")))?;

        let data_dir = format!("{home}/.local/share/wayle");
        fs::create_dir_all(&data_dir)
            .map_err(|err| Error::DatabaseError(format!("cannot create data directory: {err}")))?;

        let db_path = format!("{data_dir}/notifications.db");
        debug!(path = %db_path, "notification store opened");
        let connection = Connection::open(db_path)
            .map_err(|err| Error::DatabaseError(format!("cannot open database: {err}")))?;

        Self::configure(&connection)?;

        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Migrates (if needed) and creates the schema on a freshly-opened connection. Split out from
    /// [`new`](Self::new) so tests can drive it against an in-memory database.
    fn configure(connection: &Connection) -> Result<(), Error> {
        // An older database is dropped and recreated when its columns don't match the current
        // schema — either the pre-facet wide layout (no `data` blob column) or a pre-`expires_at`
        // layout (before the deadline column that drives downtime-expiry reaping). Persisted
        // notifications are ephemeral, so losing a session's history on upgrade is acceptable.
        let columns: Vec<String> = {
            let mut stmt = connection
                .prepare("PRAGMA table_info(notifications)")
                .map_err(|err| Error::DatabaseError(format!("cannot inspect schema: {err}")))?;
            let columns = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|err| Error::DatabaseError(format!("cannot read schema: {err}")))?;
            columns.filter_map(Result::ok).collect()
        };
        let has_column = |name: &str| columns.iter().any(|column| column == name);
        if !columns.is_empty() && (!has_column("data") || !has_column("expires_at")) {
            connection
                .execute("DROP TABLE notifications", [])
                .map_err(|err| Error::DatabaseError(format!("cannot drop legacy table: {err}")))?;
        }

        // `expires_at` is the absolute wall-clock deadline (ms since epoch) for a finite-timeout
        // notification, or NULL when it never auto-expires (server-default / never-by-app /
        // backend-persistent). Storing the resolved deadline as a column lets the downtime reap
        // and the load filter be plain SQL comparisons rather than parsing every JSON blob.
        connection
            .execute(
                "CREATE TABLE IF NOT EXISTS notifications (
                    id INTEGER PRIMARY KEY,
                    timestamp INTEGER NOT NULL,
                    expires_at INTEGER,
                    data TEXT NOT NULL
                )",
                [],
            )
            .map_err(|err| Error::DatabaseError(format!("cannot create table: {err}")))?;

        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;",
            )
            .map_err(|err| Error::DatabaseError(format!("cannot set pragmas: {err}")))?;

        Ok(())
    }

    #[instrument(skip(self, notification), fields(id = %notification.id), err)]
    pub fn add(&self, notification: &Notification) -> Result<(), Error> {
        let stored = StoredNotification::from(notification);
        let data = serde_json::to_string(&stored)
            .map_err(|err| Error::DatabaseError(format!("cannot serialize notification: {err}")))?;

        // Resolve the absolute deadline once, at store time: only a finite `After` timeout expires
        // from history; every other policy stores NULL and is never reaped. Recomputed on each
        // upsert, so replacing a notification with a new timeout updates its deadline.
        let expires_at: Option<i64> = match stored.lifecycle.timeout {
            Timeout::After(ttl) => Some(stored.received_ms + ttl.as_millis() as i64),
            _ => None,
        };

        self.connection
            .lock()
            .map_err(|_| Error::DatabaseError("cannot acquire lock on database".to_string()))?
            .execute(
                "INSERT OR REPLACE INTO notifications (id, timestamp, expires_at, data)
                 VALUES (?1, ?2, ?3, ?4)",
                params![stored.id.get(), stored.received_ms, expires_at, data],
            )
            .map_err(|err| Error::DatabaseError(format!("cannot store notification: {err}")))?;

        Ok(())
    }

    #[instrument(skip(self), fields(notification_id = %id), err)]
    pub fn remove(&self, id: NotificationId) -> Result<(), Error> {
        self.connection
            .lock()
            .map_err(|_| Error::DatabaseError("cannot acquire lock on database".to_string()))?
            .execute("DELETE FROM notifications WHERE id = ?1", params![id.get()])
            .map_err(|err| Error::DatabaseError(format!("cannot remove notification: {err}")))?;

        Ok(())
    }

    /// Removes several notifications in a single atomic `DELETE` rather than one statement
    /// per id. The ids are `u32`, so inlining them in the `IN` clause is injection-safe.
    #[instrument(skip(self, ids), fields(count = ids.len()), err)]
    pub fn remove_many(&self, ids: &[NotificationId]) -> Result<(), Error> {
        if ids.is_empty() {
            return Ok(());
        }

        let placeholders = ids
            .iter()
            .map(|id| id.get().to_string())
            .collect::<Vec<_>>()
            .join(",");

        self.connection
            .lock()
            .map_err(|_| Error::DatabaseError("cannot acquire lock on database".to_string()))?
            .execute(
                &format!("DELETE FROM notifications WHERE id IN ({placeholders})"),
                [],
            )
            .map_err(|err| Error::DatabaseError(format!("cannot remove notifications: {err}")))?;

        Ok(())
    }

    #[instrument(skip(self), err)]
    pub fn load_all(&self, remove_expired: bool) -> Result<Vec<StoredNotification>, Error> {
        let conn = self
            .connection
            .lock()
            .map_err(|_| Error::DatabaseError("cannot acquire lock on database".to_string()))?;

        // A single `now` snapshot for both the reap and the survivor query, taken under the held
        // lock so they are one atomic view: a notification is never both reaped and loaded, nor
        // dropped by neither. A finite notification that crosses its deadline *after* this
        // snapshot is still loaded (kept in the store) and later removed through the normal
        // `Remove` path when monitoring arms its timer — so it is never orphaned in the store.
        let now_ms = Utc::now().timestamp_millis();

        // Reap notifications whose finite deadline elapsed while the daemon was down — the one
        // expiry no live `Remove` event can cover (no process ran to fire the timer). Every
        // runtime removal (dismiss / close / clear / expiry-while-running) already deletes at
        // removal time, so this is the sole reason the store could otherwise grow unbounded.
        // Plain SQL on the precomputed deadline; NULL `expires_at` never expires. Best-effort:
        // a failed reap must not fail the load.
        if remove_expired {
            match conn.execute(
                "DELETE FROM notifications WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                params![now_ms],
            ) {
                Ok(count) if count > 0 => debug!(count, "reaped expired notifications from store"),
                Ok(_) => {}
                Err(err) => warn!(error = %err, "cannot reap expired notifications from store"),
            }
        }

        // Load the survivors. When reaping, exclude anything already past its deadline in the
        // SAME `now_ms` snapshot as the DELETE, so an expired-during-downtime notification never
        // even briefly enters the live list (option (b): no stale flash on startup).
        let rows: Vec<String> = if remove_expired {
            let mut stmt = conn
                .prepare(
                    "SELECT data FROM notifications
                     WHERE expires_at IS NULL OR expires_at > ?1
                     ORDER BY timestamp DESC",
                )
                .map_err(|err| Error::DatabaseError(format!("cannot prepare query: {err}")))?;
            stmt.query_map(params![now_ms], |row| row.get::<_, String>(0))
                .map_err(|err| Error::DatabaseError(format!("cannot query notifications: {err}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| Error::DatabaseError(format!("cannot read notifications: {err}")))?
        } else {
            let mut stmt = conn
                .prepare("SELECT data FROM notifications ORDER BY timestamp DESC")
                .map_err(|err| Error::DatabaseError(format!("cannot prepare query: {err}")))?;
            stmt.query_map([], |row| row.get::<_, String>(0))
                .map_err(|err| Error::DatabaseError(format!("cannot query notifications: {err}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| Error::DatabaseError(format!("cannot read notifications: {err}")))?
        };

        let mut notifications = Vec::with_capacity(rows.len());
        for data in rows {
            match serde_json::from_str::<StoredNotification>(&data) {
                Ok(notif) => notifications.push(notif),
                Err(err) => warn!(error = %err, "cannot deserialize stored notification; skipping"),
            }
        }

        debug!(count = notifications.len(), "loaded stored notifications");
        Ok(notifications)
    }
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::{ObjectPath, OwnedValue, Str, Value};

    use super::*;

    /// Test helper mirroring how `add` encodes the dispatch, then how `load_all` decodes it.
    fn serialize_source(source: &NotificationSource) -> String {
        serde_json::to_string(&stored_source(source)).expect("serialize source")
    }

    fn deserialize_source(raw: Option<String>) -> NotificationSource {
        raw.and_then(|raw| serde_json::from_str::<StoredSource>(&raw).ok())
            .map(source_from_stored)
            .unwrap_or_else(|| {
                NotificationSource::Freedesktop(FreedesktopDispatch {
                    wire_id: 0,
                    session_id: String::new(),
                    owner: None,
                })
            })
    }

    fn owned(value: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(value).expect("value converts to OwnedValue")
    }

    /// An isolated store backed by an in-memory database with the real schema applied.
    fn in_memory_store() -> NotificationStore {
        let connection = rusqlite::Connection::open_in_memory().expect("open in-memory database");
        NotificationStore::configure(&connection).expect("configure schema");
        NotificationStore {
            connection: Arc::new(Mutex::new(connection)),
        }
    }

    /// Inserts a raw row with a chosen `expires_at`. The `data` blob is intentionally not a valid
    /// `StoredNotification` — these tests assert the DELETE/SELECT behavior on the `expires_at`
    /// column, which runs before (and independently of) blob deserialization.
    fn insert_row(store: &NotificationStore, id: i64, expires_at: Option<i64>) {
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO notifications (id, timestamp, expires_at, data) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, 0i64, expires_at, "{}"],
            )
            .expect("insert row");
    }

    fn remaining_ids(store: &NotificationStore) -> Vec<i64> {
        let conn = store.connection.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM notifications ORDER BY id")
            .expect("prepare");
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect")
    }

    #[test]
    fn load_all_reaps_only_elapsed_finite_deadlines() {
        let store = in_memory_store();
        let now = Utc::now().timestamp_millis();
        insert_row(&store, 1, Some(now - 1_000)); // finite, elapsed → reaped
        insert_row(&store, 2, Some(now + 60_000)); // finite, still live → kept
        insert_row(&store, 3, None); // never expires → kept

        store.load_all(true).expect("load");

        // The elapsed row is DELETED from the DB (bounded growth), not merely filtered from the
        // returned set; live and never-expire rows remain.
        assert_eq!(remaining_ids(&store), vec![2, 3]);
    }

    #[test]
    fn load_all_without_reaping_keeps_every_row() {
        let store = in_memory_store();
        let now = Utc::now().timestamp_millis();
        insert_row(&store, 1, Some(now - 1_000)); // elapsed, but reaping is off
        insert_row(&store, 2, None);

        store.load_all(false).expect("load");

        assert_eq!(remaining_ids(&store), vec![1, 2]);
    }

    #[test]
    fn source_round_trips_gtk_dispatch_with_target() {
        let mut button_actions = HashMap::new();
        button_actions.insert(
            String::from("app.open"),
            GtkAction {
                name: String::from("open"),
                target: Some(OwnedValue::from(Str::from("chat-42"))),
            },
        );
        let source = NotificationSource::Gtk(GtkDispatch {
            app_id: String::from("org.gnome.Fractal"),
            gtk_id: String::from("msg-1"),
            default_action: Some(GtkAction {
                name: String::from("show"),
                target: None,
            }),
            button_actions,
        });

        let restored = deserialize_source(Some(serialize_source(&source)));

        let NotificationSource::Gtk(dispatch) = restored else {
            panic!("expected a gtk source");
        };
        assert_eq!(dispatch.app_id, "org.gnome.Fractal");
        assert_eq!(dispatch.gtk_id, "msg-1");
        let default = dispatch.default_action.expect("default action preserved");
        assert_eq!(default.name, "show");
        assert!(default.target.is_none());
        let action = dispatch
            .button_actions
            .get("app.open")
            .expect("button action preserved");
        assert_eq!(action.name, "open");
        let target = action.target.as_ref().expect("target preserved");
        assert_eq!(target.downcast_ref::<String>().unwrap(), "chat-42");
    }

    /// Regression guard for a *structured* action target (e.g. a `(yay)`): it must
    /// survive store→load. `serde_json::from_str::<OwnedValue>` rebuilds borrowed
    /// scalars/strings from JSON but cannot reconstruct a `Structure`/composite variant,
    /// so the previous `serde_json`-of-`OwnedValue` encoding serialized such a target
    /// fine yet deserialized it back to `None`. The action was then dispatched with an
    /// empty parameter and the app rejected it ("expected type (yay) but got type ()").
    #[test]
    fn source_round_trips_structured_default_action_target() {
        let target = OwnedValue::try_from(Value::new((5u8, vec![1u8, 2, 3]))).unwrap();
        assert_eq!(target.value_signature().to_string(), "(yay)");

        let source = NotificationSource::Gtk(GtkDispatch {
            app_id: String::from("de.schmidhuberj.Flare"),
            gtk_id: String::from("msg-1"),
            default_action: Some(GtkAction {
                name: String::from("notification-clicked"),
                target: Some(target.try_clone().unwrap()),
            }),
            button_actions: HashMap::new(),
        });

        let restored = deserialize_source(Some(serialize_source(&source)));

        let NotificationSource::Gtk(dispatch) = restored else {
            panic!("expected a gtk source");
        };
        let default = dispatch.default_action.expect("default action preserved");
        assert_eq!(default.name, "notification-clicked");
        let restored_target = default.target.expect("structured target preserved");
        assert_eq!(restored_target.value_signature().to_string(), "(yay)");
        assert_eq!(restored_target, target);
    }

    /// The target encoding must survive store→load for every shape of GVariant an app
    /// might attach to an action — scalars, strings, object paths, arrays, tuples,
    /// nested tuples and dicts. Each case asserts (a) the signature is unchanged and
    /// (b) re-encoding the restored value yields byte-identical wire data, which proves
    /// value *and* type survived exactly without relying on `OwnedValue`'s equality for
    /// composite types.
    #[test]
    fn action_target_round_trips_across_gvariant_schemas() {
        let mut dict_ss: HashMap<String, String> = HashMap::new();
        dict_ss.insert(String::from("key"), String::from("value"));
        let mut dict_si: HashMap<String, i32> = HashMap::new();
        dict_si.insert(String::from("count"), 7);
        let mut dict_sv: HashMap<String, Value> = HashMap::new();
        dict_sv.insert(String::from("id"), Value::new(42i32));

        let cases: Vec<(&str, OwnedValue)> = vec![
            // Scalars, one per D-Bus basic type.
            ("y", owned(Value::U8(255))),
            ("b", owned(Value::Bool(true))),
            ("n", owned(Value::I16(-1234))),
            ("q", owned(Value::U16(60000))),
            ("i", owned(Value::I32(-42))),
            ("u", owned(Value::U32(42))),
            ("x", owned(Value::I64(-5_000_000_000))),
            ("t", owned(Value::U64(5_000_000_000))),
            ("d", owned(Value::F64(3.5))),
            ("s", owned(Value::new("conversation-42"))),
            (
                "o",
                owned(Value::new(
                    ObjectPath::try_from("/de/schmidhuberj/Flare/chat/7").unwrap(),
                )),
            ),
            // Arrays, including the byte array a naive JSON encoding could mangle.
            ("ay", owned(Value::new(vec![0u8, 1, 2, 255]))),
            ("ai", owned(Value::new(vec![1i32, -2, 3]))),
            (
                "as",
                owned(Value::new(vec![String::from("a"), String::from("b")])),
            ),
            // Tuples / structures, the shapes the old serde_json path silently dropped.
            ("(yay)", owned(Value::new((5u8, vec![1u8, 2, 3])))),
            ("(si)", owned(Value::new((String::from("chat"), 7i32)))),
            (
                "(ss)",
                owned(Value::new((String::from("a"), String::from("b")))),
            ),
            (
                "((si)s)",
                owned(Value::new((
                    (String::from("inner"), 7i32),
                    String::from("outer"),
                ))),
            ),
            // Dicts, including a vardict (a{sv}) whose values are themselves variants.
            ("a{ss}", owned(Value::new(dict_ss))),
            ("a{si}", owned(Value::new(dict_si))),
            ("a{sv}", owned(Value::new(dict_sv))),
        ];

        for (expected_sig, target) in &cases {
            let encoded =
                encode_target(target).unwrap_or_else(|| panic!("encode failed for {expected_sig}"));
            let decoded = decode_target(&encoded)
                .unwrap_or_else(|| panic!("decode failed for {expected_sig}"));
            assert_eq!(
                decoded.value_signature().to_string(),
                *expected_sig,
                "signature changed for {expected_sig}"
            );
            let reencoded = encode_target(&decoded)
                .unwrap_or_else(|| panic!("re-encode failed for {expected_sig}"));
            assert_eq!(reencoded, encoded, "wire bytes changed for {expected_sig}");
        }
    }

    /// End-to-end through `serialize_source`/`deserialize_source`: a dispatch with a
    /// structured default action plus several buttons carrying differently-typed targets
    /// (and one with none) must come back intact, keyed correctly.
    #[test]
    fn source_round_trips_dispatch_with_mixed_target_schemas() {
        let mut button_actions = HashMap::new();
        button_actions.insert(
            String::from("app.reply"),
            GtkAction {
                name: String::from("reply"),
                target: Some(owned(Value::new("thread-9"))),
            },
        );
        button_actions.insert(
            String::from("app.mark"),
            GtkAction {
                name: String::from("mark"),
                target: Some(owned(Value::new((1u8, vec![9u8, 8, 7])))),
            },
        );
        button_actions.insert(
            String::from("app.count"),
            GtkAction {
                name: String::from("count"),
                target: Some(owned(Value::U32(3))),
            },
        );
        button_actions.insert(
            String::from("app.dismiss"),
            GtkAction {
                name: String::from("dismiss"),
                target: None,
            },
        );

        let source = NotificationSource::Gtk(GtkDispatch {
            app_id: String::from("de.schmidhuberj.Flare"),
            gtk_id: String::from("msg-7"),
            default_action: Some(GtkAction {
                name: String::from("notification-clicked"),
                target: Some(owned(Value::new((5u8, vec![1u8, 2, 3])))),
            }),
            button_actions,
        });

        let NotificationSource::Gtk(dispatch) = deserialize_source(Some(serialize_source(&source)))
        else {
            panic!("expected a gtk source");
        };

        let default = dispatch.default_action.expect("default action preserved");
        assert_eq!(default.name, "notification-clicked");
        assert_eq!(
            default.target.unwrap().value_signature().to_string(),
            "(yay)"
        );

        let reply = dispatch
            .button_actions
            .get("app.reply")
            .expect("reply button preserved");
        assert_eq!(reply.name, "reply");
        assert_eq!(
            reply
                .target
                .as_ref()
                .unwrap()
                .downcast_ref::<String>()
                .unwrap(),
            "thread-9"
        );

        let mark = dispatch
            .button_actions
            .get("app.mark")
            .expect("mark button preserved");
        assert_eq!(
            mark.target.as_ref().unwrap().value_signature().to_string(),
            "(yay)"
        );

        let count = dispatch
            .button_actions
            .get("app.count")
            .expect("count button preserved");
        assert_eq!(
            count.target.as_ref().unwrap().value_signature().to_string(),
            "u"
        );

        let dismiss = dispatch
            .button_actions
            .get("app.dismiss")
            .expect("dismiss button preserved");
        assert!(dismiss.target.is_none());
    }

    /// Backward compatibility: targets persisted before the wire-format switch were
    /// stored as `serde_json::to_string(&OwnedValue)`. `decode_target` must still read
    /// those (for the scalar/string values that legacy path could round-trip) so an
    /// upgrade doesn't silently drop targets from already-stored notifications.
    #[test]
    fn decode_target_reads_legacy_json_string_encoding() {
        let legacy = serde_json::to_string(&OwnedValue::from(Str::from("chat-42")))
            .expect("legacy encode succeeds");
        // Sanity: the legacy form is the old object encoding, not the new byte array.
        assert!(serde_json::from_str::<Vec<u8>>(&legacy).is_err());

        let decoded = decode_target(&legacy).expect("legacy target still decodes");
        assert_eq!(decoded.downcast_ref::<String>().unwrap(), "chat-42");
    }

    /// A `None` target must serialize away and come back `None` (a body click with no
    /// default-action target dispatches a plain `Activate`, not `ActivateAction([])`).
    #[test]
    fn none_target_round_trips_as_none() {
        let stored = stored_action(&GtkAction {
            name: String::from("show"),
            target: None,
        });
        assert!(stored.target.is_none());
        let restored = gtk_action_from_stored(stored);
        assert_eq!(restored.name, "show");
        assert!(restored.target.is_none());
    }

    #[test]
    fn source_round_trips_portal_dispatch_with_target() {
        let mut button_actions = HashMap::new();
        button_actions.insert(
            String::from("app.reply"),
            PortalAction {
                // Portal keeps the raw (unstripped) action name.
                name: String::from("app.reply"),
                target: Some(OwnedValue::try_from(Value::new((5u8, vec![1u8, 2, 3]))).unwrap()),
                purpose: Some(ButtonPurpose::SystemCustomAlert),
            },
        );
        let source = NotificationSource::Portal(PortalDispatch {
            app_id: String::from("de.schmidhuberj.Flare"),
            portal_id: String::from("msg-1"),
            default_action: Some(PortalAction {
                name: String::from("app.show"),
                target: None,
                purpose: None,
            }),
            button_actions,
            reply_action: Some(PortalAction {
                name: String::from("app.inline-reply"),
                target: Some(OwnedValue::from(Str::from("thread-1"))),
                purpose: Some(ButtonPurpose::ImReplyWithText),
            }),
        });

        let restored = deserialize_source(Some(serialize_source(&source)));

        let NotificationSource::Portal(dispatch) = restored else {
            panic!("expected a portal source");
        };
        assert_eq!(dispatch.app_id, "de.schmidhuberj.Flare");
        assert_eq!(dispatch.portal_id, "msg-1");
        let default = dispatch.default_action.expect("default action preserved");
        assert_eq!(default.name, "app.show");
        assert!(default.target.is_none());
        let reply = dispatch
            .button_actions
            .get("app.reply")
            .expect("button action preserved");
        assert_eq!(reply.name, "app.reply");
        let target = reply.target.as_ref().expect("structured target preserved");
        assert_eq!(target.value_signature().to_string(), "(yay)");
        assert_eq!(reply.purpose, Some(ButtonPurpose::SystemCustomAlert));
        // The inline-reply action round-trips too (target + purpose).
        let reply_action = dispatch.reply_action.expect("reply action preserved");
        assert_eq!(reply_action.name, "app.inline-reply");
        assert_eq!(
            reply_action.target.unwrap().downcast_ref::<String>().unwrap(),
            "thread-1"
        );
        assert_eq!(reply_action.purpose, Some(ButtonPurpose::ImReplyWithText));
    }

    #[test]
    fn source_absent_or_unknown_defaults_to_freedesktop() {
        assert!(matches!(
            deserialize_source(None),
            NotificationSource::Freedesktop(_)
        ));
        assert!(matches!(
            deserialize_source(Some(String::from("garbage"))),
            NotificationSource::Freedesktop(_)
        ));
    }
}
