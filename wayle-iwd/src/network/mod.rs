//! A visible IWD network (the analogue of a NetworkManager access point).
//!
//! Networks are lightweight snapshots rebuilt from
//! `Station.GetOrderedNetworks()` on each scan, so they carry no live
//! per-object monitoring; the [`Station`](crate::station::Station) re-fetches
//! the list when the connection or scan state changes.

use derive_more::Debug;
use wayle_core::Property;
use zbus::{Connection, zvariant::OwnedObjectPath};

use crate::{
    error::Error,
    proxy::{known_network::KnownNetworkProxy, network::NetworkProxy},
    station::is_real_path,
    types::{SecurityType, SignalStrength},
};

/// A network visible to a station.
#[derive(Clone, Debug)]
pub struct Network {
    #[debug(skip)]
    connection: Connection,
    object_path: OwnedObjectPath,
    /// Network name (SSID).
    pub ssid: Property<String>,
    /// Signal strength as a discrete [`SignalStrength`] bucket.
    pub strength: Property<SignalStrength>,
    /// Security classification derived from `Network.Type`.
    pub security: Property<SecurityType>,
    /// Whether credentials for this network are already saved.
    pub known: Property<bool>,
}

impl Network {
    /// D-Bus object path of this network.
    pub fn object_path(&self) -> &OwnedObjectPath {
        &self.object_path
    }

    /// Build a snapshot from a network object path and its bucketed signal
    /// strength.
    pub(crate) async fn from_path(
        connection: &Connection,
        path: OwnedObjectPath,
        strength: SignalStrength,
    ) -> Result<Self, Error> {
        let proxy = NetworkProxy::new(connection, path.clone())
            .await
            .map_err(Error::DbusError)?;

        let ssid = proxy
            .name()
            .await
            .map_err(|_| Error::ObjectNotFound(path.clone()))?;

        let security = SecurityType::from_iwd_type(&proxy.network_type().await.unwrap_or_default());
        let known = proxy
            .known_network()
            .await
            .map(|p| is_real_path(&p))
            .unwrap_or(false);

        Ok(Self {
            connection: connection.clone(),
            object_path: path,
            ssid: Property::new(ssid),
            strength: Property::new(strength),
            security: Property::new(security),
            known: Property::new(known),
        })
    }

    /// Forget the saved credentials for this network, if any.
    ///
    /// # Errors
    /// Returns [`Error::OperationFailed`] if the D-Bus call fails.
    pub async fn forget(&self) -> Result<(), Error> {
        let proxy = NetworkProxy::new(&self.connection, self.object_path.clone())
            .await
            .map_err(|e| Error::OperationFailed {
                operation: "create network proxy",
                source: e.into(),
            })?;

        let known_path = proxy
            .known_network()
            .await
            .map_err(|e| Error::OperationFailed {
                operation: "resolve known network",
                source: e.into(),
            })?;

        if !is_real_path(&known_path) {
            return Ok(());
        }

        let known = KnownNetworkProxy::new(&self.connection, known_path)
            .await
            .map_err(|e| Error::OperationFailed {
                operation: "create known network proxy",
                source: e.into(),
            })?;

        known.forget().await.map_err(|e| Error::OperationFailed {
            operation: "forget network",
            source: e.into(),
        })?;

        // Reflect the change immediately so the UI updates without a rescan.
        self.known.set(false);

        Ok(())
    }
}
