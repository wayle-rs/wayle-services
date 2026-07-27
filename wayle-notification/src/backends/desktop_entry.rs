//! Minimal, dependency-free desktop-entry lookup for the headless daemon.
//!
//! The daemon has no gio/gtk dependency, so it can't use `gio::DesktopAppInfo`. GTK
//! notifications carry only an application id (e.g. `org.gnome.Calendar`); resolving it
//! to the human-readable `Name=` lets GTK notifications share the freedesktop blocklist
//! key space and show a friendly application name.

use std::{env, fs, path::PathBuf};

/// Resolves an application id to its desktop entry's `Name`, searching the standard XDG
/// applications directories in preference order. Returns `None` if no
/// `<app_id>.desktop` with a base `Name=` is found.
pub(crate) fn resolve_name(app_id: &str) -> Option<String> {
    let file_name = format!("{app_id}.desktop");
    applications_dirs()
        .into_iter()
        .find_map(|dir| fs::read_to_string(dir.join(&file_name)).ok())
        .as_deref()
        .and_then(parse_name)
}

/// XDG applications directories, most-preferred first: `$XDG_DATA_HOME/applications`,
/// then each `$XDG_DATA_DIRS/applications`.
fn applications_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    let data_home = env::var("XDG_DATA_HOME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::var("HOME")
                .ok()
                .map(|home| format!("{home}/.local/share"))
        });
    if let Some(data_home) = data_home {
        dirs.push(PathBuf::from(data_home).join("applications"));
    }

    let data_dirs = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| String::from("/usr/local/share:/usr/share"));
    for dir in data_dirs.split(':').filter(|value| !value.is_empty()) {
        dirs.push(PathBuf::from(dir).join("applications"));
    }

    dirs
}

/// Extracts the base `Name=` value from the `[Desktop Entry]` group, ignoring localized
/// `Name[xx]=` variants and keys in other groups.
fn parse_name(contents: &str) -> Option<String> {
    let mut in_desktop_entry = false;
    for line in contents.lines() {
        let line = line.trim();
        if let Some(group) = line.strip_prefix('[').and_then(|g| g.strip_suffix(']')) {
            in_desktop_entry = group == "Desktop Entry";
            continue;
        }
        if in_desktop_entry
            && let Some(value) = line.strip_prefix("Name=")
        {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_name_reads_base_name_from_desktop_entry_group() {
        let contents = "[Desktop Entry]\nType=Application\nName=Calendar\nName[de]=Kalender\n";
        assert_eq!(parse_name(contents).as_deref(), Some("Calendar"));
    }

    #[test]
    fn parse_name_ignores_name_in_other_groups() {
        let contents = "[Desktop Action New]\nName=New Window\n[Desktop Entry]\nName=Files\n";
        assert_eq!(parse_name(contents).as_deref(), Some("Files"));
    }

    #[test]
    fn parse_name_returns_none_without_name() {
        let contents = "[Desktop Entry]\nType=Application\n";
        assert_eq!(parse_name(contents), None);
    }
}
