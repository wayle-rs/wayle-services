//! Backend for the Mullvad 2025.9–2025.13 daemon line — 2025.9 is the earliest
//! release whose `management_interface.proto` is wire-compatible with this
//! subset (2025.9 moved `Relay.location` to field 10), and 2025.13 is the last
//! before OpenVPN removal renumbered `NormalRelaySettings` in 2025.14.

mod convert;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use tonic::transport::Channel;

use crate::{
    backend::{BackendEvent, MullvadBackend},
    error::Error,
    types::{ConnectionStatus, LoginState, NetworkCountry, NetworkTarget},
};

/// Generated client and messages for the 2025.9-line management interface.
#[allow(
    missing_docs,
    unsafe_code,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cognitive_complexity,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::too_many_lines,
    clippy::large_types_passed_by_value,
    clippy::inefficient_to_string,
    clippy::manual_ok_or,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
mod proto {
    include!(concat!(
        env!("OUT_DIR"),
        "/v2025_9/mullvad_daemon.management_interface.rs"
    ));
}

use proto::management_service_client::ManagementServiceClient;

/// Mullvad management client speaking the 2025.9-line schema.
pub(crate) struct Backend {
    client: ManagementServiceClient<Channel>,
}

impl Backend {
    /// Wraps an already-connected gRPC channel in a 2025.9-line backend.
    pub(crate) fn new(channel: Channel) -> Self {
        Self {
            client: ManagementServiceClient::new(channel),
        }
    }
}

#[async_trait]
impl MullvadBackend for Backend {
    async fn connection_status(&self) -> Result<ConnectionStatus, Error> {
        let mut client = self.client.clone();
        let state = client.get_tunnel_state(proto::Empty {}).await?.into_inner();
        Ok(convert::connection_status(state))
    }

    async fn login_state(&self) -> Result<LoginState, Error> {
        let mut client = self.client.clone();
        let device = client.get_device(proto::Empty {}).await?.into_inner();
        Ok(convert::login_state(&device))
    }

    async fn networks(&self) -> Result<Vec<NetworkCountry>, Error> {
        let mut client = self.client.clone();
        let list = client
            .get_relay_locations(proto::Empty {})
            .await?
            .into_inner();
        Ok(convert::networks(list))
    }

    async fn selected(&self) -> Result<Option<NetworkTarget>, Error> {
        let mut client = self.client.clone();
        let settings = client.get_settings(proto::Empty {}).await?.into_inner();
        Ok(settings.relay_settings.and_then(convert::selected_target))
    }

    async fn select(&self, target: &NetworkTarget) -> Result<(), Error> {
        let mut client = self.client.clone();
        // Read-modify-write: preserve the user's existing relay constraints and
        // change only the exit location (SetRelaySettings is a full replace).
        let current = client
            .get_settings(proto::Empty {})
            .await?
            .into_inner()
            .relay_settings;
        let settings = convert::relay_settings_with_location(current, target);
        client.set_relay_settings(settings).await?;
        Ok(())
    }

    async fn connect(&self) -> Result<(), Error> {
        let mut client = self.client.clone();
        client.connect_tunnel(proto::Empty {}).await?;
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), Error> {
        let mut client = self.client.clone();
        // Pre-2025.14 DisconnectTunnel takes an Empty request.
        client.disconnect_tunnel(proto::Empty {}).await?;
        Ok(())
    }

    async fn events(&self) -> Result<BoxStream<'static, BackendEvent>, Error> {
        let mut client = self.client.clone();
        let stream = client.events_listen(proto::Empty {}).await?.into_inner();
        let mapped = stream.filter_map(|message| async move {
            match message {
                Ok(event) => convert::backend_event(event),
                Err(status) => {
                    tracing::warn!(%status, "Mullvad event stream error");
                    None
                }
            }
        });
        Ok(mapped.boxed())
    }
}
