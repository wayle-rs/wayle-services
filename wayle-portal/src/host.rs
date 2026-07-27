use tracing::{debug, instrument};
use zbus::{Connection, names::WellKnownName};

use crate::error::Error;

/// Owns the `org.freedesktop.impl.portal.desktop.<shortname>` connection that portal
/// backend interfaces are served on.
///
/// A portal backend is a *single* D-Bus name that may expose *many*
/// `org.freedesktop.impl.portal.*` interfaces at the object path
/// `/org/freedesktop/portal/desktop`. No single service can own that name, so the shell
/// owns one `PortalHost` and hands its [`connection`](PortalHost::connection) to each
/// service that contributes an interface. The two-phase lifecycle matters:
///
/// 1. [`PortalHost::new`] builds the connection **without** requesting the well-known name.
/// 2. Each portal-providing service registers its interface object(s) on
///    [`connection`](PortalHost::connection) (e.g. `wayle-notification`'s `attach_portal`).
/// 3. [`PortalHost::serve`] requests the well-known name — *after* every interface is
///    registered, so xdg-desktop-portal never sees the name before an interface exists.
///
/// The name is injected, never hardcoded: a shell "foo" passes
/// `org.freedesktop.impl.portal.desktop.foo`.
///
/// ```no_run
/// # use wayle_portal::PortalHost;
/// # async fn example() -> Result<(), wayle_portal::Error> {
/// let mut host = PortalHost::new("org.freedesktop.impl.portal.desktop.wayle").await?;
/// // notification.attach_portal(host.connection()).await?;   // register interfaces first
/// host.serve().await?;                                        // then claim the name
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct PortalHost {
    connection: Connection,
    name: String,
    served: bool,
}

impl PortalHost {
    /// Opens a session-bus connection for the backend, validating `name` but **not** yet
    /// requesting it. Register interfaces on [`connection`](Self::connection), then call
    /// [`serve`](Self::serve).
    ///
    /// # Errors
    /// [`Error::InvalidName`] if `name` is not a valid well-known name; [`Error::Connection`]
    /// if the session bus cannot be reached.
    #[instrument(err)]
    pub async fn new(name: impl Into<String> + std::fmt::Debug) -> Result<Self, Error> {
        let name = name.into();
        Self::validate(&name)?;
        let connection = Connection::session().await.map_err(Error::Connection)?;
        Ok(Self {
            connection,
            name,
            served: false,
        })
    }

    /// Wraps an existing connection instead of opening a new one, for consumers that
    /// already have a session-bus connection they want the portal name to live on.
    ///
    /// # Errors
    /// [`Error::InvalidName`] if `name` is not a valid well-known name.
    pub fn with_connection(
        connection: Connection,
        name: impl Into<String>,
    ) -> Result<Self, Error> {
        let name = name.into();
        Self::validate(&name)?;
        Ok(Self {
            connection,
            name,
            served: false,
        })
    }

    /// The shared connection that portal-providing services register their interface
    /// objects onto (at `/org/freedesktop/portal/desktop`).
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// The well-known name this host owns (or will own once [`serve`](Self::serve) runs).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the well-known name has been requested yet.
    #[must_use]
    pub fn is_serving(&self) -> bool {
        self.served
    }

    /// Requests the well-known name on the bus. Call once, **after** every interface has
    /// been registered on [`connection`](Self::connection). Idempotent.
    ///
    /// # Errors
    /// [`Error::RequestName`] if the name cannot be acquired.
    #[instrument(skip(self), fields(name = %self.name), err)]
    pub async fn serve(&mut self) -> Result<(), Error> {
        if self.served {
            return Ok(());
        }
        let well_known =
            WellKnownName::try_from(self.name.as_str()).map_err(|_| Error::InvalidName(self.name.clone()))?;
        self.connection
            .request_name(well_known)
            .await
            .map_err(|source| Error::RequestName {
                name: self.name.clone(),
                source,
            })?;
        self.served = true;
        debug!("portal backend name acquired");
        Ok(())
    }

    fn validate(name: &str) -> Result<(), Error> {
        WellKnownName::try_from(name)
            .map(|_| ())
            .map_err(|_| Error::InvalidName(name.to_owned()))
    }
}
