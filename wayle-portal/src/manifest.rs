use std::collections::BTreeSet;

use crate::error::Error;

/// Builds the static `.portal` file that tells xdg-desktop-portal a backend exists, which
/// `org.freedesktop.impl.portal.*` interfaces it serves, and (legacy) which desktops it
/// applies to.
///
/// The `.portal` file is a packaging artifact installed to
/// `/usr/share/xdg-desktop-portal/portals/`. Keeping its interface list in sync with what
/// is actually registered at runtime is exactly the kind of drift this type guards against:
/// build the manifest from the same interface constants the services expose (e.g.
/// `NotificationServer::PORTAL_INTERFACES`), render it with [`to_file_contents`], and assert
/// consistency at startup with [`validate`].
///
/// ```
/// # use wayle_portal::PortalManifest;
/// let manifest = PortalManifest::new("org.freedesktop.impl.portal.desktop.wayle")
///     .interfaces(["org.freedesktop.impl.portal.Notification"])
///     .use_in("wayle");
/// assert!(manifest.to_file_contents().contains("DBusName=org.freedesktop.impl.portal.desktop.wayle"));
/// ```
///
/// [`to_file_contents`]: PortalManifest::to_file_contents
/// [`validate`]: PortalManifest::validate
#[derive(Debug, Clone)]
pub struct PortalManifest {
    dbus_name: String,
    interfaces: Vec<String>,
    use_in: Vec<String>,
}

impl PortalManifest {
    /// Starts a manifest for the backend owning `dbus_name` (e.g.
    /// `org.freedesktop.impl.portal.desktop.wayle`).
    pub fn new(dbus_name: impl Into<String>) -> Self {
        Self {
            dbus_name: dbus_name.into(),
            interfaces: Vec::new(),
            use_in: Vec::new(),
        }
    }

    /// Adds one implemented interface (e.g. `org.freedesktop.impl.portal.Notification`).
    #[must_use]
    pub fn interface(mut self, interface: impl Into<String>) -> Self {
        self.interfaces.push(interface.into());
        self
    }

    /// Adds several implemented interfaces at once.
    #[must_use]
    pub fn interfaces<I, S>(mut self, interfaces: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.interfaces
            .extend(interfaces.into_iter().map(Into::into));
        self
    }

    /// Adds a desktop name to the legacy `UseIn=` key (matched against
    /// `XDG_CURRENT_DESKTOP`). Deprecated by xdg-desktop-portal in favor of `portals.conf`,
    /// but still honored and useful for older versions.
    #[must_use]
    pub fn use_in(mut self, desktop: impl Into<String>) -> Self {
        self.use_in.push(desktop.into());
        self
    }

    /// The interfaces declared in this manifest.
    #[must_use]
    pub fn declared_interfaces(&self) -> &[String] {
        &self.interfaces
    }

    /// Renders the `[portal]` file contents. String lists use the freedesktop
    /// semicolon-terminated convention.
    #[must_use]
    pub fn to_file_contents(&self) -> String {
        let mut out = String::from("[portal]\n");
        out.push_str("DBusName=");
        out.push_str(&self.dbus_name);
        out.push('\n');

        out.push_str("Interfaces=");
        out.push_str(&Self::render_list(&self.interfaces));
        out.push('\n');

        if !self.use_in.is_empty() {
            out.push_str("UseIn=");
            out.push_str(&Self::render_list(&self.use_in));
            out.push('\n');
        }

        out
    }

    /// Checks that the interfaces registered at runtime match those declared in the
    /// manifest, so the `.portal` file cannot silently drift from reality.
    ///
    /// # Errors
    /// [`Error::InterfaceMismatch`] if the two sets differ.
    pub fn validate(&self, registered: &[&str]) -> Result<(), Error> {
        let declared: BTreeSet<&str> = self.interfaces.iter().map(String::as_str).collect();
        let registered_set: BTreeSet<&str> = registered.iter().copied().collect();
        if declared == registered_set {
            return Ok(());
        }
        Err(Error::InterfaceMismatch {
            expected: declared.into_iter().map(str::to_owned).collect(),
            registered: registered_set.into_iter().map(str::to_owned).collect(),
        })
    }

    fn render_list(items: &[String]) -> String {
        // freedesktop string lists are semicolon-separated AND semicolon-terminated.
        let mut out = String::new();
        for item in items {
            out.push_str(item);
            out.push(';');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_notification_only_manifest() {
        let manifest = PortalManifest::new("org.freedesktop.impl.portal.desktop.wayle")
            .interface("org.freedesktop.impl.portal.Notification")
            .use_in("wayle");

        let contents = manifest.to_file_contents();

        assert_eq!(
            contents,
            "[portal]\n\
             DBusName=org.freedesktop.impl.portal.desktop.wayle\n\
             Interfaces=org.freedesktop.impl.portal.Notification;\n\
             UseIn=wayle;\n"
        );
    }

    #[test]
    fn renders_multiple_interfaces_and_desktops() {
        let contents = PortalManifest::new("org.freedesktop.impl.portal.desktop.wayle")
            .interfaces([
                "org.freedesktop.impl.portal.Notification",
                "org.freedesktop.impl.portal.Settings",
            ])
            .use_in("wayle")
            .use_in("wlroots")
            .to_file_contents();

        assert!(contents.contains(
            "Interfaces=org.freedesktop.impl.portal.Notification;org.freedesktop.impl.portal.Settings;\n"
        ));
        assert!(contents.contains("UseIn=wayle;wlroots;\n"));
    }

    #[test]
    fn omits_use_in_when_unset() {
        let contents = PortalManifest::new("org.freedesktop.impl.portal.desktop.wayle")
            .interface("org.freedesktop.impl.portal.Notification")
            .to_file_contents();

        assert!(!contents.contains("UseIn="));
    }

    #[test]
    fn validate_accepts_matching_set_regardless_of_order() {
        let manifest = PortalManifest::new("org.freedesktop.impl.portal.desktop.wayle")
            .interfaces([
                "org.freedesktop.impl.portal.Notification",
                "org.freedesktop.impl.portal.Settings",
            ]);

        assert!(
            manifest
                .validate(&[
                    "org.freedesktop.impl.portal.Settings",
                    "org.freedesktop.impl.portal.Notification",
                ])
                .is_ok()
        );
    }

    #[test]
    fn validate_rejects_mismatched_set() {
        let manifest = PortalManifest::new("org.freedesktop.impl.portal.desktop.wayle")
            .interface("org.freedesktop.impl.portal.Notification");

        let error = manifest
            .validate(&[
                "org.freedesktop.impl.portal.Notification",
                "org.freedesktop.impl.portal.Settings",
            ])
            .unwrap_err();

        assert!(matches!(error, Error::InterfaceMismatch { .. }));
    }
}
