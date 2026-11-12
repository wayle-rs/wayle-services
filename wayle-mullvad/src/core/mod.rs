//! The reactive Mullvad model: observable state plus connect/disconnect controls.

mod monitoring;

use std::sync::Arc;

use derive_more::Debug;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use wayle_core::Property;
use wayle_traits::ModelMonitoring;

use crate::{
    backend::MullvadBackend,
    error::Error,
    types::{ConnectionStatus, LoginState, NetworkCountry, NetworkTarget, RelayLocation},
};

/// A control request queued for serialized execution against the daemon.
#[derive(Debug)]
enum ControlAction {
    Select(NetworkTarget),
    Connect,
    Disconnect,
}

/// Reactive Mullvad VPN state and controls.
///
/// State is exposed as [`Property`] fields — read a snapshot with
/// [`Property::get`], or observe changes with [`Property::watch`]. The tunnel is
/// driven with [`select`](Self::select) / [`connect`](Self::connect) /
/// [`disconnect`](Self::disconnect), which are non-blocking: they queue a
/// request that a single worker runs to completion in order, so daemon calls
/// from different callers never interleave. Observe the outcome via the
/// reactive state.
#[derive(Debug, Clone)]
pub struct Mullvad {
    #[debug(skip)]
    backend: Arc<dyn MullvadBackend>,
    #[debug(skip)]
    cancellation_token: Option<CancellationToken>,
    #[debug(skip)]
    action_tx: UnboundedSender<ControlAction>,

    /// The available networks, as a country → city → network tree.
    pub networks: Property<Vec<NetworkCountry>>,

    /// The overall VPN status to display: the tunnel state plus active relay,
    /// with the account login state overlaid taking precedence (`LoggedOut` /
    /// `Revoked` override any tunnel state, since the tunnel cannot be up then).
    pub status: Property<ConnectionStatus>,

    /// The relay location the daemon has selected — the one a
    /// [`connect`](Self::connect) would use (display names). Sourced
    /// authoritatively from the daemon's persisted relay settings (initial
    /// snapshot + settings events), so it reflects what connect will do even
    /// before this client calls [`select`](Self::select). `None` when the daemon
    /// has no single geographic selection (a custom list or an unconstrained
    /// "any" location), or until it resolves against the relay tree.
    pub selected: Property<Option<RelayLocation>>,

    /// Tunnel-only status (from tunnel-state events), combined with [`login`] into
    /// the public [`status`](Self::status).
    tunnel_status: Property<ConnectionStatus>,
    /// Account login state (from device events), combined with [`tunnel_status`]
    /// into the public [`status`](Self::status).
    login: Property<LoginState>,
    /// Raw code-only selection from the daemon, kept so [`selected`](Self::selected)
    /// can be re-resolved to display names when the relay tree changes.
    selected_target: Property<Option<NetworkTarget>>,
}

impl Mullvad {
    /// Builds a live model from `backend`, fetching an initial snapshot and
    /// starting background monitoring + the control-action worker under
    /// `cancellation_token`.
    ///
    /// # Errors
    /// Returns an error if any initial state fetch fails, or if monitoring
    /// cannot start.
    pub(crate) async fn connect_live(
        backend: Arc<dyn MullvadBackend>,
        cancellation_token: CancellationToken,
    ) -> Result<Arc<Self>, Error> {
        let tunnel = backend.connection_status().await?;
        let login = backend.login_state().await?;
        let networks = backend.networks().await?;
        let selected_target = backend.selected().await?;
        let tunnel = fix_relay_code(tunnel, &networks);
        let status = combine_status(login, &tunnel);
        let selected = selected_target
            .as_ref()
            .and_then(|target| resolve_selection(target, &networks));

        // Serialize all control actions through a single worker so daemon calls
        // from different UI handlers can never interleave.
        let (action_tx, action_rx) = mpsc::unbounded_channel();
        spawn_action_worker(Arc::clone(&backend), action_rx, cancellation_token.clone());

        let model = Arc::new(Self {
            backend,
            cancellation_token: Some(cancellation_token),
            action_tx,
            networks: Property::new(networks),
            status: Property::new(status),
            selected: Property::new(selected),
            tunnel_status: Property::new(tunnel),
            login: Property::new(login),
            selected_target: Property::new(selected_target),
        });

        Arc::clone(&model).start_monitoring().await?;
        Ok(model)
    }

    /// Selects `target` as the relay location: queues a daemon write that changes
    /// only the exit location (preserving the other relay constraints).
    /// Non-blocking, and daemon-authoritative — [`selected`](Self::selected)
    /// updates when the daemon emits the resulting settings event, not
    /// optimistically here.
    pub fn select(&self, target: &NetworkTarget) {
        let _ = self.action_tx.send(ControlAction::Select(target.clone()));
    }

    /// Queues a connect. The daemon connects to its currently selected relay —
    /// i.e. the location published as [`selected`](Self::selected). Non-blocking.
    pub fn connect(&self) {
        let _ = self.action_tx.send(ControlAction::Connect);
    }

    /// Queues a disconnect. Non-blocking.
    pub fn disconnect(&self) {
        let _ = self.action_tx.send(ControlAction::Disconnect);
    }

    /// Stores the tunnel-only status (fixing the relay's country code against the
    /// tree) and republishes the combined [`status`](Self::status).
    pub(crate) fn set_tunnel_status(&self, tunnel: ConnectionStatus) {
        self.tunnel_status
            .set(fix_relay_code(tunnel, &self.networks.get()));
        self.recompute_status();
    }

    /// Stores the login state and republishes the combined [`status`](Self::status).
    pub(crate) fn set_login(&self, login: LoginState) {
        self.login.set(login);
        self.recompute_status();
    }

    /// Records the daemon's current relay selection and republishes the resolved
    /// [`selected`](Self::selected) (or `None` if the tree cannot name it yet).
    pub(crate) fn set_selected(&self, target: Option<NetworkTarget>) {
        let resolved = target
            .as_ref()
            .and_then(|t| resolve_selection(t, &self.networks.get()));
        self.selected_target.set(target);
        self.selected.set(resolved);
    }

    /// Re-resolves [`selected`](Self::selected)'s display names against the
    /// current relay tree, after the relay list changes.
    pub(crate) fn resync_selected_names(&self) {
        let target = self.selected_target.get();
        let resolved = target
            .as_ref()
            .and_then(|t| resolve_selection(t, &self.networks.get()));
        self.selected.set(resolved);
    }

    /// Recomputes the public [`status`](Self::status) by overlaying the login
    /// state onto the tunnel status.
    fn recompute_status(&self) {
        self.status
            .set(combine_status(self.login.get(), &self.tunnel_status.get()));
    }
}

/// Overlays the login state onto the tunnel status, login taking precedence:
/// while logged out or revoked the tunnel cannot be up, so those states override
/// any (stale) tunnel state.
fn combine_status(login: LoginState, tunnel: &ConnectionStatus) -> ConnectionStatus {
    match login {
        LoginState::LoggedOut => ConnectionStatus::LoggedOut,
        LoginState::Revoked => ConnectionStatus::Revoked,
        LoginState::LoggedIn => tunnel.clone(),
    }
}

/// Resolves a selection (codes) to a display-ready [`RelayLocation`] using the
/// country → city tree for names, or `None` when the tree cannot (yet) name the
/// country — so [`selected`](Mullvad::selected) is only published once it can be
/// constructed from the relay list.
fn resolve_selection(target: &NetworkTarget, networks: &[NetworkCountry]) -> Option<RelayLocation> {
    let (country_code, city_code, hostname) = match target {
        NetworkTarget::Country { code } => (code.as_str(), None, None),
        NetworkTarget::City { country, code } => (country.as_str(), Some(code.as_str()), None),
        NetworkTarget::Relay {
            country,
            city,
            hostname,
        } => (
            country.as_str(),
            Some(city.as_str()),
            Some(hostname.clone()),
        ),
    };
    let country = networks
        .iter()
        .find(|country| country.code == country_code)?;
    Some(RelayLocation {
        country_code: country_code.to_owned(),
        country: country.name.clone(),
        city: city_code.and_then(|code| {
            country
                .cities
                .iter()
                .find(|city| city.code == code)
                .map(|city| city.name.clone())
        }),
        hostname,
    })
}

/// Replaces a live relay's `country_code` with the authoritative code from the
/// relay tree (matched by hostname), keeping the backend's hostname-parsed code
/// when the tree does not contain the relay.
fn fix_relay_code(status: ConnectionStatus, networks: &[NetworkCountry]) -> ConnectionStatus {
    fn with_code(mut relay: RelayLocation, networks: &[NetworkCountry]) -> RelayLocation {
        if let Some(code) = relay
            .hostname
            .as_deref()
            .and_then(|hostname| code_for_hostname(hostname, networks))
        {
            relay.country_code = code;
        }
        relay
    }

    match status {
        ConnectionStatus::Connecting(Some(relay)) => {
            ConnectionStatus::Connecting(Some(with_code(relay, networks)))
        }
        ConnectionStatus::Connected(relay) => {
            ConnectionStatus::Connected(with_code(relay, networks))
        }
        other => other,
    }
}

/// The ISO country code of the relay with `hostname` in the tree, if present.
fn code_for_hostname(hostname: &str, networks: &[NetworkCountry]) -> Option<String> {
    networks.iter().find_map(|country| {
        country
            .cities
            .iter()
            .flat_map(|city| &city.networks)
            .any(|network| network.hostname == hostname)
            .then(|| country.code.clone())
    })
}

/// Drains queued [`ControlAction`]s, running each against the backend to
/// completion before the next so two callers' daemon calls never interleave.
/// Stops when the token is cancelled or the queue is closed (model dropped).
fn spawn_action_worker(
    backend: Arc<dyn MullvadBackend>,
    mut rx: UnboundedReceiver<ControlAction>,
    token: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let action = tokio::select! {
                () = token.cancelled() => return,
                action = rx.recv() => match action {
                    Some(action) => action,
                    None => return,
                },
            };

            let result = match action {
                ControlAction::Select(target) => backend.select(&target).await,
                ControlAction::Connect => backend.connect().await,
                ControlAction::Disconnect => backend.disconnect().await,
            };
            if let Err(error) = result {
                tracing::warn!(%error, "mullvad control action failed");
            }
        }
    });
}
