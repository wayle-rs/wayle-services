//! Shared `org.freedesktop.Application` dispatch for the [`gtk`](super::gtk) and
//! [`portal`](super::portal) backends.
//!
//! Both protocols identify the source app by an `app_id` that doubles as a D-Bus-activatable
//! bus name, and both mirror GNOME Shell's rule for a body click with no explicit
//! `default-action`: raise (or cold-launch) that app. GNOME Shell does it in-process via
//! `Shell.App.activate()`; a D-Bus notification server does the equivalent by calling
//! `org.freedesktop.Application.Activate`. That one call — plus the focus-token plumbing it
//! carries — is the only `org.freedesktop.Application` machinery the two backends share, so it
//! lives here rather than being duplicated (GTK also dispatches `app.`-prefixed *button*
//! actions via `ActivateAction`, which stays in the GTK backend).

use std::collections::HashMap;

use zbus::{
    Connection,
    zvariant::{OwnedValue, Value},
};

use crate::{core::types::gtk_object_path, error::Error};

/// The interface a `DBusActivatable` GApplication exposes; the session bus cold-launches the
/// app via service activation if it is not already running.
pub(crate) const APPLICATION_INTERFACE: &str = "org.freedesktop.Application";

/// The `"app."` prefix on `GNotification` detailed action names, stripped before dispatch via
/// `org.freedesktop.Application.ActivateAction`.
pub(crate) const APP_ACTION_PREFIX: &str = "app.";

/// Builds the `org.freedesktop.Application` platform-data `a{sv}` carrying the shell's focus
/// token, so a (possibly cold-launched) app can raise its window past focus-stealing
/// prevention. GLib reads `activation-token` (with `desktop-startup-id` as the legacy alias)
/// out of it in its `before_emit`. Empty when the shell couldn't mint a token.
pub(crate) fn platform_data(token: Option<&str>) -> HashMap<String, OwnedValue> {
    let mut platform_data = HashMap::new();
    if let Some(token) = token
        && let Ok(value) = OwnedValue::try_from(Value::new(token))
    {
        if let Ok(legacy) = value.try_clone() {
            platform_data.insert(String::from("desktop-startup-id"), legacy);
        }
        platform_data.insert(String::from("activation-token"), value);
    }
    platform_data
}

/// Raises — or cold-launches, for a `DBusActivatable` app — the GApplication `app_id` by
/// calling `org.freedesktop.Application.Activate(platform_data)`. This is the shared "body
/// click with no explicit default action" behavior both the GTK and portal backends route to,
/// mirroring GNOME Shell's `Shell.App.activate()` fallback.
pub(crate) async fn activate(
    connection: &Connection,
    app_id: &str,
    token: Option<&str>,
) -> Result<(), Error> {
    connection
        .call_method(
            Some(app_id),
            gtk_object_path(app_id).as_str(),
            Some(APPLICATION_INTERFACE),
            "Activate",
            &(platform_data(token),),
        )
        .await?;
    Ok(())
}
