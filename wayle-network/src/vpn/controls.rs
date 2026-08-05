use tracing::instrument;
use wayle_core::NULL_PATH;
use zbus::{Connection, zvariant::OwnedObjectPath};

use crate::{error::Error, proxy::manager::NetworkManagerProxy};

pub(super) struct VpnControls;

impl VpnControls {
    #[instrument(skip(connection), fields(profile = %connection_path), err)]
    pub(super) async fn connect(
        connection: &Connection,
        connection_path: OwnedObjectPath,
    ) -> Result<OwnedObjectPath, Error> {
        let proxy = NetworkManagerProxy::new(connection).await?;

        let null_path =
            OwnedObjectPath::try_from(NULL_PATH).map_err(|err| Error::DbusError(err.into()))?;

        proxy
            .activate_connection(&connection_path, &null_path, &null_path)
            .await
            .map_err(|err| Error::OperationFailed {
                operation: "activate VPN connection",
                source: err.into(),
            })
    }

    #[instrument(skip(connection), fields(active = %active_connection_path), err)]
    pub(super) async fn disconnect(
        connection: &Connection,
        active_connection_path: OwnedObjectPath,
    ) -> Result<(), Error> {
        let proxy = NetworkManagerProxy::new(connection).await?;

        proxy
            .deactivate_connection(&active_connection_path)
            .await
            .map_err(|err| Error::OperationFailed {
                operation: "deactivate VPN connection",
                source: err.into(),
            })
    }
}
