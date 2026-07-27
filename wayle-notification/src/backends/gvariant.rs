//! Shared parsing for the `GNotification`-serialized `a{sv}` vardict.
//!
//! Both the `org.gtk.Notifications` and `org.freedesktop.impl.portal.Notification` backends
//! receive notifications in GLib's `g_notification_serialize()` format, so the icon, button
//! and value-extraction helpers are written once here and used by both. Protocol-specific
//! handling (GTK strips the `"app."` prefix and only keeps prefixed actions; the portal
//! echoes action names verbatim) stays in each backend.

use std::{collections::HashMap, os::fd::AsFd, path::PathBuf};

use zbus::zvariant::{Fd, OwnedValue, Value};

use crate::core::types::Image;

use crate::image_cache;

/// A button parsed from the `buttons` `aa{sv}` array.
pub(super) struct ParsedButton {
    pub(super) label: String,
    pub(super) action: String,
    pub(super) target: Option<OwnedValue>,
    /// The raw `purpose` string (portal v2), if present. GTK ignores it.
    pub(super) purpose: Option<String>,
}

/// Parses the `buttons` value (`aa{sv}`) into label/action/target triples. Malformed
/// entries (missing label or action) are skipped.
pub(super) fn parse_buttons(value: Option<&OwnedValue>) -> Vec<ParsedButton> {
    let mut buttons = Vec::new();
    let Some(value) = value else {
        return buttons;
    };
    let Value::Array(array) = &**value else {
        return buttons;
    };

    for element in array.iter() {
        let Ok(cloned) = element.try_clone() else {
            continue;
        };
        let Ok(row) = HashMap::<String, OwnedValue>::try_from(cloned) else {
            continue;
        };
        let (Some(label), Some(action)) =
            (owned_string(row.get("label")), owned_string(row.get("action")))
        else {
            continue;
        };
        buttons.push(ParsedButton {
            label,
            action,
            target: row.get("target").and_then(try_clone_owned),
            purpose: owned_string(row.get("purpose")),
        });
    }

    buttons
}

/// Parses a serialized `GIcon` / portal icon `(sv)` into a single [`Image`] — the sending app's
/// icon. A themed icon → [`Image::Named`]; a `file`/`bytes`/`file-descriptor` icon → a cached or
/// on-disk [`Image::Path`]; an `emblemed` icon → its base icon (emblems dropped). Unrecognized
/// shapes (`emblem`, `gvfs`, …) yield `None`, so the shell falls back to the `desktop-entry` icon.
///
/// This always maps to `origin.icon` (the app's icon), regardless of form — matching how the
/// freedesktop backend routes its `app_icon` — so an app's PNG icon is never mistaken for a large
/// content image just because it arrived as a file/bytes GIcon.
///
/// The `file-descriptor` form is what a v2 portal backend actually receives for inline icons:
/// xdg-desktop-portal rewrites the deprecated `bytes` form to a sealed-memfd fd before forwarding,
/// so handling it here is required, not optional.
pub(super) fn parse_icon(value: &OwnedValue) -> Option<Image> {
    parse_icon_value(value)
}

fn parse_icon_value(value: &Value<'_>) -> Option<Image> {
    let Value::Structure(structure) = value else {
        return None;
    };
    let [Value::Str(tag), Value::Value(payload)] = structure.fields() else {
        return None;
    };
    let payload = &**payload;

    match tag.as_str() {
        "themed" => {
            if let Value::Array(names) = payload
                && let Some(Value::Str(name)) = names.iter().next()
            {
                return Some(Image::Named(name.to_string()));
            }
            None
        }
        "file" => match payload {
            Value::Str(path) => Some(Image::Path(PathBuf::from(strip_file_uri(path.as_str())))),
            _ => None,
        },
        "bytes" => match payload {
            Value::Array(array) => {
                let bytes: Vec<u8> = array
                    .iter()
                    .filter_map(|value| match value {
                        Value::U8(byte) => Some(*byte),
                        _ => None,
                    })
                    .collect();
                image_cache::cache_encoded_image(&bytes).map(|path| Image::Path(PathBuf::from(path)))
            }
            _ => None,
        },
        "file-descriptor" => match payload {
            Value::Fd(fd) => read_fd(fd)
                .and_then(|bytes| image_cache::cache_encoded_image(&bytes))
                .map(|path| Image::Path(PathBuf::from(path))),
            _ => None,
        },
        // GEmblemedIcon serializes as `(base-icon: v, emblems: …)`; render the base icon and
        // drop the emblems (the notification's icon is the app icon, not the badge overlay).
        "emblemed" => match payload {
            Value::Structure(inner) => match inner.fields().first() {
                Some(Value::Value(base)) => parse_icon_value(base),
                _ => None,
            },
            _ => None,
        },
        // `emblem` (a bare GEmblem) and `gvfs` (a mount-backed icon) are rare and have no useful
        // notification rendering; fall back to the desktop-entry icon.
        _ => None,
    }
}

/// Reads every byte from a received file descriptor (a sealed memfd from the portal, or any
/// seekable fd). Dups first so reading doesn't disturb the sender's copy, and seeks to the
/// start because the shared open-file description's offset may have been left anywhere.
pub(super) fn read_fd(fd: &Fd<'_>) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    let owned = fd.as_fd().try_clone_to_owned().ok()?;
    let mut file = std::fs::File::from(owned);
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

pub(super) fn strip_file_uri(path: &str) -> String {
    path.strip_prefix("file://").unwrap_or(path).to_owned()
}

pub(super) fn owned_string(value: Option<&OwnedValue>) -> Option<String> {
    let value = value?;
    if let Ok(string) = value.downcast_ref::<String>() {
        return Some(string);
    }
    // Values inside a wire-deserialized *nested* dict (e.g. a button's `a{sv}` within the
    // `buttons` `aa{sv}`) retain an explicit `Value::Value` variant layer that `downcast_ref`
    // won't peel — unlike the top-level vardict, whose one variant layer zbus already unwrapped
    // into the `OwnedValue`. Peel that layer so nested string extraction works too.
    if let Value::Value(inner) = &**value
        && let Value::Str(string) = &**inner
    {
        return Some(string.to_string());
    }
    None
}

pub(super) fn try_clone_owned(value: &OwnedValue) -> Option<OwnedValue> {
    value.try_clone().ok()
}

#[cfg(test)]
mod tests {
    use std::{io::Write, os::fd::AsFd};

    use super::*;

    #[test]
    fn read_fd_reads_all_bytes_regardless_of_offset() {
        // A portal fd arrives as a sealed memfd; emulate one with a temp file whose write
        // left the offset at the end — `read_fd` must seek to the start before reading.
        let path = std::env::temp_dir().join(format!("wayle-read-fd-{}.bin", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .expect("open temp fd file");
        file.write_all(b"encoded-payload").expect("write payload");

        let fd = Fd::from(file.as_fd());
        let bytes = read_fd(&fd).expect("reads the whole fd from the start");
        assert_eq!(bytes, b"encoded-payload");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_fd_empty_is_none() {
        let path = std::env::temp_dir().join(format!("wayle-read-fd-empty-{}.bin", std::process::id()));
        let file = std::fs::File::create(&path).expect("create empty temp file");
        let fd = Fd::from(file.as_fd());
        assert!(read_fd(&fd).is_none(), "an empty fd yields no bytes");
        let _ = std::fs::remove_file(&path);
    }
}
