//! The single task that owns the service's state.
//!
//! The dispatcher consumes, in one loop: commands queued by the public API;
//! every message from IWD on one subscription bound to its unique bus name
//! (its signals, its replies to our requests, and its calls to our agents, in
//! the order IWD sent them); IWD's bus name changing hands; errors the bus
//! returns in IWD's place; and calls from anyone else (answered as unknown).
//! It sends every request itself, in order, and applies each reply to the
//! instance it was sent for.

pub(crate) mod command;
mod passphrase;
mod registry;
mod request;
mod session;

use std::{collections::HashMap, sync::Arc, time::Duration};

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use zbus::{
    Connection, MatchRule, Message, MessageStream,
    fdo::{self, NameOwnerChanged, NameOwnerChangedStream},
    message::Type,
    names::BusName,
    zvariant::{OwnedObjectPath, OwnedValue},
};

pub(crate) use self::registry::Published;
use self::{
    command::{Command, Commands},
    passphrase::Passphrase,
    registry::{ADAPTER_INTERFACE, Changes, DEVICE_INTERFACE, Registry, STATION_INTERFACE},
    request::{Call, Pending, Target},
    session::{Link, LinkEvent},
};
use crate::{
    agent::{self, AGENT_PATH, AgentCall, Answer, SIGNAL_LEVEL_AGENT_PATH},
    error::{Error, IWD_ABORTED, IWD_ALREADY_EXISTS, IWD_BUSY, IWD_NOT_CONNECTED},
    network::Network,
    station::Station,
    types::{SIGNAL_STRENGTH_THRESHOLDS, SignalStrength, StationAction, StationError},
};

/// IWD's well-known bus name.
pub(crate) const IWD_SERVICE: &str = "net.connman.iwd";

/// The bus driver, which answers requests itself when it can't deliver them.
const BUS_DRIVER: &str = "org.freedesktop.DBus";

const NETWORK_INTERFACE: &str = "net.connman.iwd.Network";
const KNOWN_NETWORK_INTERFACE: &str = "net.connman.iwd.KnownNetwork";
const DIAGNOSTIC_INTERFACE: &str = "net.connman.iwd.StationDiagnostic";
const AGENT_MANAGER_INTERFACE: &str = "net.connman.iwd.AgentManager";
const AGENT_MANAGER_PATH: &str = "/net/connman/iwd";

/// What [`start`] hands the service.
pub(crate) struct Handle {
    pub published: Published,
    pub commands: Commands,
    pub cancellation_token: CancellationToken,
}

/// Connects to the system bus, enumerates IWD if it is running, and spawns the
/// dispatcher on the current Tokio runtime. IWD isn't D-Bus-activated: it may
/// be installed alongside NetworkManager without being the WiFi daemon in use.
///
/// # Errors
/// Returns error if the system bus connection fails.
pub(crate) async fn start() -> Result<Handle, Error> {
    let connection = Connection::system()
        .await
        .map_err(|err| Error::ServiceInitialization(Box::new(err)))?;

    // Subscribed before any request is sent, so the dispatcher sees every
    // error the bus returns in IWD's place, and every call to this connection
    // (it answers them itself: there is no object server).
    let bus_errors = MessageStream::for_match_rule(
        MatchRule::builder()
            .msg_type(Type::Error)
            .sender(BUS_DRIVER)?
            .build(),
        &connection,
        None,
    )
    .await?;
    let calls = MessageStream::for_match_rule(
        MatchRule::builder().msg_type(Type::MethodCall).build(),
        &connection,
        None,
    )
    .await?;

    // Watch IWD's bus name before resolving its owner, so a restart in between
    // is still seen.
    let dbus = fdo::DBusProxy::new(&connection).await?;
    let owner_changed = dbus
        .receive_name_owner_changed_with_args(&[(0, IWD_SERVICE)])
        .await?;
    let owner = dbus
        .get_name_owner(BusName::from_static_str(IWD_SERVICE).map_err(zbus::Error::from)?)
        .await
        .ok()
        .map(|owner| owner.to_string());
    if owner.is_none() {
        info!("iwd is not running; waiting for it to start");
    }

    let (commands, command_rx) = Commands::channel();
    let published = Published::new();
    let cancellation_token = CancellationToken::new();
    let link = match owner {
        Some(owner) => Link::syncing(&connection, owner, Duration::ZERO),
        None => Link::Down,
    };
    let mut dispatcher = Dispatcher {
        registry: Registry::new(&commands, published.clone()),
        passphrase: Passphrase::new(&published.passphrase_request),
        link,
        connection,
        commands: command_rx,
        owner_changed,
        bus_errors,
        calls,
        cancellation_token: cancellation_token.clone(),
        pending: Pending::default(),
        ordering: Ordering::Idle,
    };

    // The first enumeration, so the service starts out populated.
    if matches!(dispatcher.link, Link::Syncing { .. }) {
        let event = dispatcher.link.next().await;
        dispatcher.handle_link(event).await;
    }
    tokio::spawn(dispatcher.run());

    Ok(Handle {
        published,
        commands,
        cancellation_token,
    })
}

/// Reading IWD's order of the station's networks: one request at a time,
/// another queued if the networks changed meanwhile.
#[derive(Debug, PartialEq, Eq)]
enum Ordering {
    Idle,
    InFlight,
    InFlightAndStale,
}

struct Dispatcher {
    connection: Connection,
    registry: Registry,
    /// The link to the current IWD, which also knows its bus name.
    link: Link,
    commands: mpsc::UnboundedReceiver<Command>,
    owner_changed: NameOwnerChangedStream,
    /// Errors from the bus driver, for requests it answered in IWD's place.
    bus_errors: MessageStream,
    /// Every method call to this connection.
    calls: MessageStream,
    cancellation_token: CancellationToken,
    pending: Pending,
    passphrase: Passphrase,
    ordering: Ordering,
}

impl Dispatcher {
    async fn run(mut self) {
        loop {
            tokio::select! {
                () = self.cancellation_token.cancelled() => {
                    debug!("iwd dispatcher stopped");
                    return;
                }
                Some(command) = self.commands.recv() => self.handle_command(command).await,
                Some(signal) = self.owner_changed.next() => self.handle_owner_changed(&signal),
                event = self.link.next() => self.handle_link(event).await,
                Some(Ok(error)) = self.bus_errors.next() => {
                    // The bus answered one of our requests in IWD's place
                    // (e.g. IWD went away without replying).
                    self.handle_reply(&error).await;
                }
                Some(Ok(call)) = self.calls.next() => self.handle_foreign_call(&call).await,
            }
        }
    }

    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::Connect(network) => {
                let Some(station) = self.live_station_for(&network) else {
                    return;
                };
                station.last_error.set(None);
                self.connect(station, network).await;
            }
            Command::Forget(network) => {
                let Some(station) = self.live_station_for(&network) else {
                    return;
                };
                let Some(known) = self.registry.known_network(&network).cloned() else {
                    debug!(ssid = %network.ssid.get(), "nothing saved to forget");
                    return;
                };
                let action = StationAction::Forget {
                    network: Arc::clone(&network),
                };
                let call = Call::method(&known, KNOWN_NETWORK_INTERFACE, "Forget");
                self.station_request(station, action, call).await;
            }
            Command::Disconnect(station) => {
                let call = Call::method(station.object_path(), STATION_INTERFACE, "Disconnect");
                self.station_request(station, StationAction::Disconnect, call)
                    .await;
            }
            Command::Scan(station) => {
                let call = Call::method(station.object_path(), STATION_INTERFACE, "Scan");
                self.station_request(station, StationAction::Scan, call)
                    .await;
            }
            Command::SetPowered(station, powered) => self.set_powered(station, powered).await,
            Command::DismissError(station) => {
                if self.registry.is_live(&station) {
                    station.last_error.set(None);
                }
            }
            Command::ProvidePassphrase(passphrase) => {
                let ours = self
                    .passphrase
                    .asking_for()
                    .is_some_and(|network| self.pending.connecting(network));
                if let Some(reply) = self.passphrase.provide(passphrase, ours) {
                    self.send_message(&reply).await;
                }
            }
            Command::CancelPassphrase => {
                if let Some(reply) = self.passphrase.cancel() {
                    self.send_message(&reply).await;
                }
            }
        }
    }

    /// The live station, if `network` is still a live network.
    fn live_station_for(&self, network: &Arc<Network>) -> Option<Arc<Station>> {
        if !self.registry.live_network(network) {
            debug!(path = %network.object_path(), "iwd network no longer exists");
            return None;
        }
        self.registry.station().cloned()
    }

    async fn connect(&mut self, station: Arc<Station>, network: Arc<Network>) {
        let (call, target) = connect_request(station, network);
        self.send(call, target).await;
    }

    /// Turns WiFi on (`on`) or off as desktops do: through rfkill, by powering
    /// every adapter (`Adapter.Powered` is IWD's rfkill soft block), and, to
    /// turn it on, bringing the device up if it is down. Only what needs
    /// changing is sent. Without adapters to block, the device is powered
    /// instead.
    ///
    /// A device whose radio is blocked is down, whether or not it was also
    /// taken down on its own; IWD brings it back up once the radio is
    /// unblocked only if the last request it had for the device was to be up.
    /// So the device is asked to be up first: IWD records that, then fails
    /// to bring it up while the radio is still blocked (an expected failure,
    /// not recorded), and brings it up itself once the adapter is unblocked.
    async fn set_powered(&mut self, station: Arc<Station>, on: bool) {
        let Some((device_up, adapter_powered)) = self
            .registry
            .is_live(&station)
            .then(|| self.registry.station_power())
            .flatten()
        else {
            return;
        };
        let blockable = self.registry.adapters().next().is_some();
        let adapters: Vec<Call> = self
            .registry
            .adapters()
            .filter(|(_, powered)| *powered != on)
            .map(|(adapter, _)| {
                Call::set_property(&adapter.clone().into(), ADAPTER_INTERFACE, "Powered", on)
            })
            .collect();
        let path = station.object_path().clone();
        let device = |up: bool| Call::set_property(&path, DEVICE_INTERFACE, "Powered", up);
        let recorded = || Target::Action {
            station: Arc::clone(&station),
            action: StationAction::SetPowered,
        };
        let mut requests = Vec::new();

        if on && !device_up {
            let target = if adapter_powered {
                recorded()
            } else {
                Target::DeviceWantedUp
            };
            requests.push((device(true), target));
        }
        requests.extend(adapters.into_iter().map(|call| (call, recorded())));
        if !on && !blockable && device_up {
            requests.push((device(false), recorded()));
        }

        if requests.is_empty() {
            return;
        }
        // Cleared once, not per request: a request failing as it is sent is
        // recorded at once, and mustn't be wiped by the next one.
        station.last_error.set(None);
        for (call, target) in requests {
            self.send(call, target).await;
        }
    }

    /// Sends a request whose failure is recorded on `station`, which must be
    /// the live one.
    async fn station_request(&mut self, station: Arc<Station>, action: StationAction, call: Call) {
        if !self.registry.is_live(&station) {
            debug!(?action, "iwd station no longer exists");
            return;
        }
        station.last_error.set(None);
        self.send(call, Target::Action { station, action }).await;
    }

    /// Sends `call`, and whatever its failure to send leads to.
    async fn send(&mut self, call: Call, target: Target) {
        let mut next = Some((call, target));
        while let Some((call, target)) = next.take() {
            if let Err((target, err)) = self.pending.send(&self.connection, call, target).await {
                next = self.complete(target, Err(err));
            }
        }
    }

    async fn handle_reply(&mut self, message: &Message) -> bool {
        let Some((target, result)) = self.pending.take_reply(message) else {
            return false;
        };
        if let Some((call, target)) = self.complete(target, result) {
            self.send(call, target).await;
        }
        true
    }

    /// Applies the reply to a request to the instance it was sent for.
    /// Returns the request to send next, if the reply leads to one.
    fn complete(
        &mut self,
        target: Target,
        result: Result<Message, Error>,
    ) -> Option<(Call, Target)> {
        match target {
            Target::Connect { station, network } => {
                let rejected = self
                    .passphrase
                    .connect_finished(network.object_path(), &result);
                if !self.registry.is_live(&station) {
                    return None;
                }
                if rejected {
                    debug!(ssid = %network.ssid.get(), "iwd rejected the passphrase; asking again");
                    return Some(connect_request(station, network));
                }
                record_failure(&station, StationAction::Connect { network }, result);
            }
            Target::Action { station, action } => {
                if self.registry.is_live(&station) {
                    record_failure(&station, action, result);
                }
            }
            Target::OrderedNetworks(station) => {
                let stale = self.ordering == Ordering::InFlightAndStale;
                self.ordering = Ordering::Idle;
                match result.and_then(|reply| {
                    reply
                        .body()
                        .deserialize::<Vec<(OwnedObjectPath, i16)>>()
                        .map_err(|err| Error::Dbus(err.into()))
                }) {
                    Ok(ordered) => self.registry.set_ordered(&station, ordered),
                    Err(err) => debug!(error = %err, "cannot read iwd's ordered networks"),
                }
                if stale {
                    return self.ordering_request();
                }
            }
            Target::Diagnostics(station) => {
                if !self.registry.is_live(&station) {
                    return None;
                }
                match result.and_then(|reply| {
                    reply
                        .body()
                        .deserialize::<HashMap<String, OwnedValue>>()
                        .map_err(|err| Error::Dbus(err.into()))
                }) {
                    Ok(diagnostics) => apply_diagnostics(&station, &diagnostics),
                    // Diagnostics may need privileges this user lacks.
                    Err(err) => debug!(error = %err, "cannot read iwd diagnostics"),
                }
            }
            Target::DeviceWantedUp => {
                if let Err(err) = result {
                    debug!(error = %err, "iwd brings the device up once its radio is unblocked");
                }
            }
            Target::SignalLevelAgent => {
                if let Err(err) = result
                    && !err.is_iwd_error(IWD_ALREADY_EXISTS)
                {
                    debug!(error = %err, "cannot register iwd signal-level agent; strength updates on connect only");
                }
            }
            Target::Agent => {
                if let Err(err) = result
                    && !err.is_iwd_error(IWD_ALREADY_EXISTS)
                {
                    warn!(error = %err, "cannot register iwd passphrase agent");
                }
            }
        }
        None
    }

    async fn handle_link(&mut self, event: LinkEvent) {
        match event {
            LinkEvent::Synced(Ok(session)) => {
                let changes = self.registry.load(session.objects, &session.buffered);
                self.link = Link::Up {
                    owner: session.owner,
                    stream: session.stream,
                };
                // IWD forgets agents when it restarts.
                let call = Call::method_with_path(
                    &object_path(AGENT_MANAGER_PATH),
                    AGENT_MANAGER_INTERFACE,
                    "RegisterAgent",
                    object_path(AGENT_PATH),
                );
                self.send(call, Target::Agent).await;
                self.react(changes).await;
            }
            LinkEvent::Synced(Err(err)) => {
                if let Some(retry) = self.link.retry(&self.connection) {
                    warn!(error = %err, "cannot enumerate iwd objects; retrying");
                    self.link = retry;
                }
            }
            LinkEvent::Message(message) => self.handle_message(&message).await,
        }
    }

    async fn handle_message(&mut self, message: &Message) {
        if self.handle_reply(message).await {
            return;
        }

        match message.message_type() {
            Type::Signal => {
                let changes = self.registry.handle(message);
                self.react(changes).await;
            }
            // IWD calling an agent.
            Type::MethodCall => self.handle_agent_call(message).await,
            // Replies to requests that aren't the dispatcher's.
            Type::MethodReturn | Type::Error => {}
        }
    }

    /// Follows up on what a signal changed.
    async fn react(&mut self, changes: Changes) {
        for network in &changes.removed_networks {
            if let Some(reply) = self.passphrase.network_removed(network) {
                self.send_message(&reply).await;
            }
        }
        let Some(station) = self.registry.station().cloned() else {
            return;
        };
        if changes.station_up {
            let call = Call::register_signal_levels(
                station.object_path(),
                STATION_INTERFACE,
                object_path(SIGNAL_LEVEL_AGENT_PATH),
                SIGNAL_STRENGTH_THRESHOLDS.to_vec(),
            );
            self.send(call, Target::SignalLevelAgent).await;
        }
        if changes.link_up {
            let call = Call::method(
                station.object_path(),
                DIAGNOSTIC_INTERFACE,
                "GetDiagnostics",
            );
            self.send(call, Target::Diagnostics(station)).await;
        }
        if changes.networks {
            self.refresh_networks().await;
        }
    }

    /// Reads IWD's order of the station's networks, unless a read is already
    /// under way (then it is read again once that one is answered).
    async fn refresh_networks(&mut self) {
        if self.ordering != Ordering::Idle {
            self.ordering = Ordering::InFlightAndStale;
            return;
        }
        if let Some((call, target)) = self.ordering_request() {
            self.send(call, target).await;
        }
    }

    /// The request reading IWD's order of the station's networks, if it has a
    /// `Station` interface to ask.
    fn ordering_request(&mut self) -> Option<(Call, Target)> {
        let station = self.registry.station()?.clone();
        if !station.powered.get() {
            return None;
        }
        self.ordering = Ordering::InFlight;
        let call = Call::method(
            station.object_path(),
            STATION_INTERFACE,
            "GetOrderedNetworks",
        );
        Some((call, Target::OrderedNetworks(station)))
    }

    async fn handle_agent_call(&mut self, call: &Message) {
        let replies: Vec<Message> = match AgentCall::parse(call) {
            Ok(AgentCall::RequestPassphrase(path)) => match self.registry.network(&path).cloned() {
                Some(network) => self
                    .passphrase
                    .ask(call.clone(), network)
                    .into_iter()
                    .collect(),
                None => {
                    debug!(%path, "iwd asked for the passphrase of an unknown network");
                    agent::reply(call, Answer::Canceled).into_iter().collect()
                }
            },
            // Enterprise credentials come from IWD's provisioning files.
            Ok(AgentCall::Unsupported) => {
                agent::reply(call, Answer::Canceled).into_iter().collect()
            }
            Ok(AgentCall::Cancel) => {
                let mut replies: Vec<Message> = self.passphrase.cancelled().into_iter().collect();
                replies.extend(agent::reply(call, Answer::Done));
                replies
            }
            Ok(AgentCall::Release) => {
                self.passphrase.released();
                agent::reply(call, Answer::Done).into_iter().collect()
            }
            Ok(AgentCall::SignalLevel { device, level }) => {
                if let Some(station) = self.registry.station()
                    && station.object_path() == &device
                {
                    station
                        .strength
                        .set(Some(SignalStrength::from_level(level)));
                }
                agent::reply(call, Answer::Done).into_iter().collect()
            }
            Ok(AgentCall::SignalLevelReleased) => {
                agent::reply(call, Answer::Done).into_iter().collect()
            }
            Err(error) => agent::reply_error(call, error).into_iter().collect(),
        };
        for reply in replies {
            self.send_message(&reply).await;
        }
    }

    /// Answers a method call from anyone but IWD (IWD's arrive on its own
    /// subscription too, and are answered there).
    async fn handle_foreign_call(&mut self, call: &Message) {
        let from_iwd = call
            .header()
            .sender()
            .is_some_and(|sender| Some(sender.as_str()) == self.link.owner());
        if from_iwd {
            return;
        }

        if let Some(reply) = agent::reply_error(call, agent::UNKNOWN_METHOD) {
            self.send_message(&reply).await;
        }
    }

    async fn send_message(&mut self, message: &Message) {
        if let Err(err) = self.connection.send(message).await {
            debug!(error = %err, "cannot send iwd reply");
        }
    }

    fn handle_owner_changed(&mut self, signal: &NameOwnerChanged) {
        let Ok(args) = signal.args() else {
            return;
        };
        let previous = args.old_owner().as_ref().map(ToString::to_string);
        let owner = args.new_owner().as_ref().map(ToString::to_string);

        // Only a change from the owner followed counts (not one queued before
        // startup looked the owner up).
        if previous.as_deref() != self.link.owner() || owner.as_deref() == self.link.owner() {
            return;
        }

        // Everything belonged to the previous instance.
        self.registry.clear();
        self.pending.clear();
        self.passphrase.clear();
        self.ordering = Ordering::Idle;

        self.link = match owner {
            Some(owner) => {
                debug!(%owner, "iwd appeared");
                Link::syncing(&self.connection, owner, Duration::ZERO)
            }
            None => {
                debug!("iwd left the bus");
                Link::Down
            }
        };
    }
}

/// Records a failed request on `station`, except outcomes that aren't
/// failures: a request cancelled (`Aborted`), a scan IWD is too busy for (it
/// already scans), and a disconnect with nothing to disconnect.
fn record_failure(station: &Station, action: StationAction, result: Result<Message, Error>) {
    let Err(err) = result else {
        return;
    };
    let benign = err.is_iwd_error(IWD_ABORTED)
        || (action == StationAction::Scan && err.is_iwd_error(IWD_BUSY))
        || (action == StationAction::Disconnect && err.is_iwd_error(IWD_NOT_CONNECTED));
    if benign {
        debug!(?action, error = %err, "iwd request didn't happen");
        return;
    }

    warn!(?action, error = %err, "iwd request failed");
    station.last_error.set(Some(StationError::new(action, err)));
}

/// Takes the connected link's RSSI and frequency from IWD's diagnostics.
fn apply_diagnostics(station: &Station, diagnostics: &HashMap<String, OwnedValue>) {
    if let Some(rssi) = diagnostics
        .get("RSSI")
        .and_then(|value| i16::try_from(value).ok())
    {
        station.strength.set(Some(SignalStrength::from_dbm(rssi)));
    }
    if let Some(frequency) = diagnostics
        .get("Frequency")
        .and_then(|value| u32::try_from(value).ok())
    {
        station.frequency.set(Some(frequency));
    }
}

/// The request connecting `station` to `network`.
fn connect_request(station: Arc<Station>, network: Arc<Network>) -> (Call, Target) {
    let call = Call::method(network.object_path(), NETWORK_INTERFACE, "Connect");
    (call, Target::Connect { station, network })
}

fn object_path(path: &'static str) -> OwnedObjectPath {
    OwnedObjectPath::from(zbus::zvariant::ObjectPath::from_static_str_unchecked(path))
}
