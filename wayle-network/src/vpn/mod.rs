mod controls;
mod monitoring;
mod types;

use std::sync::Arc;

use controls::VpnControls;
use derive_more::Debug;
use futures::stream::Stream;
use tokio_util::sync::CancellationToken;
pub(crate) use types::{LiveVpnParams, VpnParams};
use wayle_core::{Property, watch_all};
use wayle_traits::{ModelMonitoring, Reactive};
use zbus::{Connection, zvariant::OwnedObjectPath};

use super::{
    core::{
        connection::ActiveConnection, settings::Settings, settings_connection::ConnectionSettings,
    },
    error::Error,
    proxy::active_connection::vpn::VPNConnectionProxy,
    types::{
        connectivity::ConnectionType,
        states::{NMActiveConnectionState, NetworkStatus},
    },
};

/// VPN connections with profile and activation control. See [crate-level
/// docs](crate) for usage.
#[derive(Clone, Debug)]
pub struct Vpn {
    /// Aggregate connectivity status across all active VPN connections.
    pub connectivity: Property<NetworkStatus>,
    /// Currently active VPN connections.
    pub active_connections: Property<Vec<Arc<ActiveConnection>>>,
    /// Saved VPN connection profiles.
    pub connections: Property<Vec<ConnectionSettings>>,
    /// Banner from the first active VPN connection, if any.
    pub banner: Property<Option<String>>,
    #[debug(skip)]
    pub(crate) zbus_connection: Connection,
    #[debug(skip)]
    pub(crate) cancellation_token: Option<CancellationToken>,
    #[debug(skip)]
    pub(crate) settings: Arc<Settings>,
}

impl Reactive for Vpn {
    type Context<'a> = VpnParams<'a>;
    type LiveContext<'a> = LiveVpnParams<'a>;
    type Error = Error;

    async fn get(params: Self::Context<'_>) -> Result<Self, Self::Error> {
        Self::from_connection(params.connection, params.settings, None).await
    }

    async fn get_live(params: Self::LiveContext<'_>) -> Result<Arc<Self>, Self::Error> {
        let vpn = Self::from_connection(
            params.connection,
            params.settings,
            Some(params.cancellation_token.child_token()),
        )
        .await?;
        let vpn = Arc::new(vpn);

        vpn.clone().start_monitoring().await?;

        Ok(vpn)
    }
}

impl Vpn {
    /// Watch for any VPN property changes.
    ///
    /// Emits whenever any VPN property changes (connectivity, active
    /// connections, profiles, or banner).
    pub fn watch(&self) -> impl Stream<Item = Vpn> + Send {
        watch_all!(self, connectivity, active_connections, connections, banner)
    }

    /// Activate a saved VPN connection profile.
    ///
    /// # Errors
    ///
    /// Returns `NetworkError::OperationFailed` if activation fails.
    pub async fn connect(
        &self,
        connection_path: OwnedObjectPath,
    ) -> Result<OwnedObjectPath, Error> {
        VpnControls::connect(&self.zbus_connection, connection_path).await
    }

    /// Deactivate a specific active VPN connection.
    ///
    /// # Errors
    ///
    /// Returns `NetworkError::OperationFailed` if deactivation fails.
    pub async fn disconnect(&self, active_connection_path: OwnedObjectPath) -> Result<(), Error> {
        VpnControls::disconnect(&self.zbus_connection, active_connection_path).await
    }

    /// Deactivate every currently active VPN connection.
    ///
    /// Errors deactivating individual connections are returned from the first
    /// failure encountered; remaining connections are not attempted.
    ///
    /// # Errors
    ///
    /// Returns `NetworkError::OperationFailed` if any deactivation fails.
    pub async fn disconnect_all(&self) -> Result<(), Error> {
        for active in self.active_connections.get() {
            VpnControls::disconnect(&self.zbus_connection, active.object_path.clone()).await?;
        }
        Ok(())
    }

    pub(crate) fn is_vpn_settings(connection: &ConnectionSettings) -> bool {
        matches!(
            connection.connection_type.get(),
            ConnectionType::Vpn | ConnectionType::WireGuard
        )
    }

    pub(crate) fn is_vpn_active(active: &ActiveConnection) -> bool {
        active.vpn.get()
            || matches!(
                ConnectionType::from_nm_type(&active.type_.get()),
                ConnectionType::Vpn | ConnectionType::WireGuard
            )
    }

    pub(crate) fn connectivity_for(actives: &[Arc<ActiveConnection>]) -> NetworkStatus {
        if actives.is_empty() {
            return NetworkStatus::Disconnected;
        }

        let mut any_connecting = false;
        for active in actives {
            match active.state.get() {
                NMActiveConnectionState::Activated => return NetworkStatus::Connected,
                NMActiveConnectionState::Activating => any_connecting = true,
                _ => {}
            }
        }

        if any_connecting {
            NetworkStatus::Connecting
        } else {
            NetworkStatus::Disconnected
        }
    }

    pub(crate) async fn banner_of_first(
        connection: &Connection,
        actives: &[Arc<ActiveConnection>],
    ) -> Option<String> {
        let first = actives.first()?;
        let proxy = VPNConnectionProxy::new(connection, &first.object_path)
            .await
            .ok()?;
        let banner = proxy.banner().await.ok()?;
        if banner.is_empty() {
            None
        } else {
            Some(banner)
        }
    }

    fn vpn_profiles_from(settings: &Settings) -> Vec<ConnectionSettings> {
        settings
            .connections
            .get()
            .into_iter()
            .filter(Self::is_vpn_settings)
            .collect()
    }

    async fn from_connection(
        connection: &Connection,
        settings: Arc<Settings>,
        cancellation_token: Option<CancellationToken>,
    ) -> Result<Self, Error> {
        let connections = Self::vpn_profiles_from(&settings);

        Ok(Self {
            connectivity: Property::new(NetworkStatus::Disconnected),
            active_connections: Property::new(Vec::new()),
            connections: Property::new(connections),
            banner: Property::new(None),
            zbus_connection: connection.clone(),
            cancellation_token,
            settings,
        })
    }
}
