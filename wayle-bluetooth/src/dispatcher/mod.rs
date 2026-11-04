//! The dispatcher: the one task that changes the service's state.
//!
//! Everything the service knows or does passes through it, one event at a
//! time, in the order the events happened:
//!
//! - BlueZ's messages, on a single subscription bound to bluetoothd's unique
//!   bus name: `InterfacesAdded`/`InterfacesRemoved` and `PropertiesChanged`
//!   (the state), the replies to our requests, and BlueZ's calls to the
//!   pairing agent, all in the order BlueZ sent them;
//! - commands queued by the public API (actions and queries), whose requests
//!   it sends to BlueZ in the order they were queued;
//! - bluetoothd appearing, restarting or going away (`NameOwnerChanged`);
//! - errors the bus itself answers our requests with (e.g. `NoReply`);
//! - method calls from anyone else, which it turns away;
//! - the timers ending timed discoveries.
//!
//! State is taken from BlueZ wherever it publishes it; the only exceptions,
//! a device's activity and most recent failure, the pairing request, and this
//! service's discovery sessions, follow BlueZ's events as far as they can
//! (see [`crate::core::device::activity`], [`pairing`] and [`discovery`]).
//! Because a single task owns all of it, nothing is locked and nothing can
//! race.
//!
//! A session starts by subscribing and then calling `GetManagedObjects` once,
//! which returns every object with all of its properties. bluetoothd restarts
//! are handled by watching its bus-name ownership (the same approach as
//! `wayle-iwd`): the state is cleared, the subscription is rebound to the new
//! owner, the new daemon is enumerated, and the pairing agent is registered
//! with it again.

pub(crate) mod command;
mod discovery;
mod pairing;
mod registry;
mod request;
mod rfkill;
mod session;

use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use tokio::{sync::mpsc, time::Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use zbus::{
    Connection, MatchRule, Message, MessageStream,
    fdo::{self, NameOwnerChanged, NameOwnerChangedStream},
    message::Type,
    names::{BusName, WellKnownName},
    zvariant::OwnedObjectPath,
};

use self::{
    command::{Command, Commands, Query},
    discovery::Discovery,
    pairing::Pairing,
    registry::{Registry, finish_adapter, update_adapter},
    request::{Pending, Target},
    rfkill::Rfkill,
    session::{Link, LinkEvent},
};
pub(crate) use self::{registry::Published, request::Call};
use crate::{
    agent,
    core::{
        adapter::{Adapter, AdapterInfo},
        device::Device,
    },
    error::{BLUEZ_IN_PROGRESS, Error},
    proxy,
    types::{
        ADAPTER_INTERFACE, BLUEZ_SERVICE,
        adapter::{AdapterAction, AdapterError, PowerState},
        device::DeviceAction,
    },
};

/// The bus driver, which answers requests itself when it can't deliver them.
const BUS_DRIVER: &str = "org.freedesktop.DBus";

/// The running dispatcher, as the service sees it.
pub(crate) struct Handle {
    pub published: Published,
    pub commands: Commands,
    pub cancellation_token: CancellationToken,
}

/// Connects to the system bus, D-Bus-activates bluetoothd if it isn't
/// running, enumerates it, and spawns the dispatcher on the current Tokio
/// runtime.
///
/// # Errors
/// Returns error if the system bus connection fails.
pub(crate) async fn start() -> Result<Handle, Error> {
    let connection = Connection::system()
        .await
        .map_err(|err| Error::ServiceInitialization(Box::new(err)))?;

    // Subscribed before any request is sent, so the dispatcher sees every
    // error the bus returns in BlueZ's place, and every call to this
    // connection (it answers them itself: there is no object server).
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

    // Watch bluetoothd's bus name before resolving its owner, so a restart
    // in between is still seen.
    let dbus = fdo::DBusProxy::new(&connection).await?;
    let owner_changed = dbus
        .receive_name_owner_changed_with_args(&[(0, BLUEZ_SERVICE)])
        .await?;

    // Start bluetoothd through D-Bus activation if it isn't running, so it has
    // an owner to enumerate. (Returns at once if it is running.)
    let bluez = WellKnownName::from_static_str(BLUEZ_SERVICE).map_err(zbus::Error::from)?;
    match dbus.start_service_by_name(bluez, 0).await {
        Ok(_) => {}
        Err(fdo::Error::ServiceUnknown(_)) => {
            info!("bluetoothd is not available; waiting for it to start");
        }
        Err(err) => warn!(error = %err, "cannot start bluetoothd; waiting for it to start"),
    }
    let owner = dbus
        .get_name_owner(BusName::from_static_str(BLUEZ_SERVICE).map_err(zbus::Error::from)?)
        .await
        .ok()
        .map(|owner| owner.to_string());

    let (commands, command_rx) = Commands::channel();
    let published = Published::new();
    let cancellation_token = CancellationToken::new();
    let link = match owner {
        Some(owner) => Link::syncing(&connection, owner, Duration::ZERO),
        None => Link::Down,
    };
    let mut dispatcher = Dispatcher {
        registry: Registry::new(&commands, published.clone()),
        pairing: Pairing::new(&published.pairing_request),
        link,
        connection,
        commands: command_rx,
        owner_changed,
        bus_errors,
        calls,
        cancellation_token: cancellation_token.clone(),
        pending: Pending::default(),
        discovery: Discovery::default(),
        rfkill: Rfkill::open(&cancellation_token),
    };
    dispatcher
        .registry
        .set_radio_block(dispatcher.rfkill.radio_block());

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

struct Dispatcher {
    connection: Connection,
    registry: Registry,
    /// The link to the current bluetoothd, which also knows its bus name.
    link: Link,
    commands: mpsc::UnboundedReceiver<Command>,
    owner_changed: NameOwnerChangedStream,
    /// Errors from the bus driver, for requests it answered in BlueZ's place.
    bus_errors: MessageStream,
    /// Every method call to this connection.
    calls: MessageStream,
    cancellation_token: CancellationToken,
    pending: Pending,
    pairing: Pairing,
    discovery: Discovery,
    /// The kernel's Bluetooth radio switches, which turn Bluetooth on and off.
    rfkill: Rfkill,
}

impl Dispatcher {
    async fn run(mut self) {
        loop {
            let deadline = self.discovery.next_deadline();

            tokio::select! {
                () = self.cancellation_token.cancelled() => {
                    debug!("bluetooth dispatcher stopped");
                    return;
                }
                Some(command) = self.commands.recv() => self.handle_command(command).await,
                Some(signal) = self.owner_changed.next() => self.handle_owner_changed(&signal),
                event = self.link.next() => self.handle_link(event).await,
                Some(Ok(error)) = self.bus_errors.next() => {
                    // The bus answered one of our requests in BlueZ's place
                    // (e.g. bluetoothd went away without replying).
                    self.handle_reply(&error).await;
                }
                Some(Ok(call)) = self.calls.next() => self.handle_foreign_call(&call).await,
                update = self.rfkill.next() => self.handle_rfkill(update),
                () = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)),
                    if deadline.is_some() => self.end_timed_discoveries().await,
            }
        }
    }

    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::Device {
                device,
                action,
                call,
            } => self.device_request(&device, action, call).await,
            Command::DismissDeviceError(device) => {
                if let Some(entry) = self.registry.entry_of(&device) {
                    entry.dismiss_error();
                }
            }
            Command::Adapter {
                adapter,
                action,
                call,
            } => {
                if self.registry.is_live(&adapter) {
                    self.send(call, Target::Adapter { adapter, action }).await;
                } else {
                    debug!(adapter = %adapter.object_path, ?action, "bluetooth adapter no longer exists");
                }
            }
            Command::DismissAdapterError(adapter) => {
                if self.registry.is_live(&adapter) {
                    update_adapter(&adapter, |info| info.last_error = None);
                }
            }
            Command::Discover { adapter, start } => {
                if self.registry.is_live(&adapter) {
                    self.discover(&adapter, start, None).await;
                }
            }
            Command::SetEnabled(on) => self.set_enabled(on).await,
            Command::StartDiscovery { timeout } => {
                let Some(adapter) = self.registry.primary_adapter() else {
                    debug!("no bluetooth adapter to discover with");
                    return;
                };
                let deadline = timeout.and_then(discovery::deadline);
                self.discover(&adapter, true, deadline).await;
            }
            Command::StopDiscovery => {
                for path in self.discovery.wanted() {
                    if let Some(adapter) = self.registry.adapter(&path).cloned() {
                        self.discover(&adapter, false, None).await;
                    }
                }
            }
            Command::Pairing(response) => {
                let answered = self.pairing.respond(response);
                // Before the answer, so a connect the disconnect interrupts
                // counts as superseded, not failed.
                if let Some(path) = answered.turned_down {
                    self.abort_pairing(&path).await;
                }
                if let Some(reply) = answered.reply {
                    self.send_message(&reply).await;
                }
            }
            Command::Query(query) => self.query(query),
        }
    }

    /// Sends `call`, the request for `action` on `device`, starting its
    /// activity.
    async fn device_request(&mut self, device: &Arc<Device>, action: DeviceAction, call: Call) {
        let Some(entry) = self.registry.entry_of(device) else {
            debug!(device = %device.object_path, ?action, "bluetooth device no longer exists");
            return;
        };

        let ticket = entry.begin(action.activity());
        self.send(
            call,
            Target::Device {
                device: Arc::clone(device),
                action,
                ticket,
            },
        )
        .await;
    }

    /// Turns Bluetooth on (`on`) or off as KDE's Bluetooth switch does: sets
    /// the rfkill soft block on every Bluetooth radio to `!on`, and powers
    /// every adapter to `on`.
    ///
    /// An adapter is powered through BlueZ only where the block doesn't do it
    /// already (see [`powered_through_bluez`]), which leaves the outcome
    /// unchanged. Blocking powers every adapter off (the kernel does even
    /// when the firmware fails to power off in an orderly way, which a
    /// power-off through BlueZ racing the block could fail at first), and
    /// BlueZ powers a blocked adapter back on itself once unblocked (and turns
    /// a request then down as busy).
    async fn set_enabled(&mut self, on: bool) {
        let blockable = self.rfkill.can_block();
        // Even if no switch is reported blocked: a block may still be on its
        // way, and blocks are written in order, so the latest request wins.
        if blockable {
            self.rfkill.set_blocked(!on);
        }

        let adapters: Vec<Arc<Adapter>> = self
            .registry
            .adapters()
            .filter(|adapter| powered_through_bluez(on, blockable, &adapter.info.get()))
            .cloned()
            .collect();
        for adapter in adapters {
            let (action, call) = adapter.power_request(on);
            self.send(call, Target::Adapter { adapter, action }).await;
        }
    }

    /// Takes an update from rfkill: a switch changed, or setting a block
    /// failed.
    fn handle_rfkill(&mut self, update: rfkill::Update) {
        match update {
            rfkill::Update::Event(event) => {
                self.rfkill.apply(event);
                self.registry.set_radio_block(self.rfkill.radio_block());
            }
            rfkill::Update::WriteFailed { blocked, error } => {
                warn!(blocked, error = %error, "cannot set the rfkill block on bluetooth");
                // Every adapter was to be powered on or off.
                let error = Arc::new(Error::Rfkill(error));
                for adapter in self.registry.adapters() {
                    update_adapter(adapter, |info| {
                        info.last_error = Some(AdapterError {
                            action: AdapterAction::SetPowered,
                            error: Arc::clone(&error),
                        });
                    });
                }
            }
        }
    }

    /// Disconnects the device at `path` if a connect or pair this service
    /// sent for it still awaits BlueZ's answer: a pairing with it was turned
    /// down, and disconnecting is how BlueZ aborts the pairing of a connect.
    ///
    /// Decided by the request rather than the device's activity: BlueZ
    /// reports a device connected (ending a connect's activity) as soon as
    /// the link is up, before it asks to pair.
    async fn abort_pairing(&mut self, path: &OwnedObjectPath) {
        let Some(device) = self.pending.pairing(path.as_str()) else {
            return;
        };

        let (action, call) = device.disconnect_request();
        self.device_request(&device, action, call).await;
    }

    /// Asks to start (with an optional `deadline`) or stop discovering on
    /// `adapter`, sending whatever request that needs now (see
    /// [`discovery`]).
    async fn discover(&mut self, adapter: &Arc<Adapter>, start: bool, deadline: Option<Instant>) {
        if let Some(start) = self
            .discovery
            .request(adapter.object_path.as_str(), start, deadline)
        {
            let (call, target) = discovery_request(adapter, start);
            self.send(call, target).await;
        }
    }

    /// Sends a request; one that can't be sent counts as failing. Sends the
    /// requests its completion leads to as well.
    async fn send(&mut self, call: Call, target: Target) {
        let mut next = Some((call, target));
        while let Some((call, target)) = next.take() {
            if let Err((target, err)) = self.pending.send(&self.connection, call, target).await {
                next = self.complete(target, Err(err));
            }
        }
    }

    /// Applies the result of a request to the object it was for, if that is
    /// still the live instance. Returns the request to send next, if the
    /// result leads to one.
    fn complete(&mut self, target: Target, result: Result<(), Error>) -> Option<(Call, Target)> {
        match target {
            Target::Device {
                device,
                action,
                ticket,
            } => {
                let entry = self.registry.entry_of(&device)?;
                let pairing_over = ends_pairing(action, &result);
                entry.finish(action, ticket, result);
                if pairing_over {
                    self.pairing.pairing_ended(device.object_path.as_str());
                }
                None
            }
            Target::Adapter { adapter, action } => {
                if self.registry.is_live(&adapter) {
                    finish_adapter(&adapter, action, result);
                }
                None
            }
            Target::Discovery { adapter, start } => {
                if !self.registry.is_live(&adapter) {
                    return None;
                }
                let next = self
                    .discovery
                    .answered(adapter.object_path.as_str(), &result);
                if start {
                    finish_adapter(&adapter, AdapterAction::StartDiscovery, result);
                } else if let Err(err) = result {
                    // BlueZ ends sessions itself (e.g. when the adapter
                    // powers off): a refused stop has nothing to stop.
                    debug!(adapter = %adapter.object_path, error = %err, "bluetooth discovery stop refused");
                }
                next.map(|start| discovery_request(&adapter, start))
            }
        }
    }

    /// Completes the request `message` answers, if it answers one of ours.
    /// Returns whether it did.
    async fn handle_reply(&mut self, message: &Message) -> bool {
        let Some((target, result)) = self.pending.take_reply(message) else {
            return false;
        };
        if let Some((call, target)) = self.complete(target, result) {
            self.send(call, target).await;
        }
        true
    }

    async fn handle_link(&mut self, event: LinkEvent) {
        match event {
            LinkEvent::Synced(Ok(session)) => {
                // BlueZ calls the agent only once it is registered, which
                // happens after this, so the buffer holds signals alone.
                self.registry.load(session.objects, &session.buffered);
                self.link = Link::Up {
                    owner: session.owner,
                    stream: session.stream,
                };
                // BlueZ forgets agents when it restarts. Registered from its own
                // task: awaiting the reply here would stop this loop draining the
                // subscription, and zbus stalls the connection (reply included)
                // when that subscription's queue fills.
                self.spawn(register_agent(self.connection.clone()));
            }
            LinkEvent::Synced(Err(err)) => {
                if let Some(retry) = self.link.retry(&self.connection) {
                    warn!(error = %err, "cannot enumerate bluetooth objects; retrying");
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
                // BlueZ ended the discovery sessions of adapters that powered
                // off or went away, without answering every request with them.
                for adapter in changes.powered_off.iter().chain(&changes.removed_adapters) {
                    self.discovery.forget(adapter);
                    self.pending.forget_discovery(adapter);
                }
                if let Some(reply) = self.pairing.devices_changed(&changes) {
                    self.send_message(&reply).await;
                }
            }
            // BlueZ calling the pairing agent.
            Type::MethodCall => {
                for reply in self.pairing.on_call(message) {
                    self.send_message(&reply).await;
                }
            }
            // Replies to requests that aren't the dispatcher's.
            Type::MethodReturn | Type::Error => {}
        }
    }

    /// Turns away a method call from anyone but bluetoothd, whose calls arrive
    /// (in order) on its subscription instead.
    async fn handle_foreign_call(&mut self, call: &Message) {
        let from_bluez = call
            .header()
            .sender()
            .is_some_and(|sender| Some(sender.as_str()) == self.link.owner());
        if from_bluez {
            return;
        }

        if let Some(reply) = agent::reply_error(call, agent::UNKNOWN_METHOD) {
            self.send_message(&reply).await;
        }
    }

    async fn send_message(&mut self, message: &Message) {
        if let Err(err) = self.connection.send(message).await {
            debug!(error = %err, "cannot send bluetooth reply");
        }
    }

    fn handle_owner_changed(&mut self, signal: &NameOwnerChanged) {
        let Ok(args) = signal.args() else {
            return;
        };
        let previous = args.old_owner().as_ref().map(ToString::to_string);
        let owner = args.new_owner().as_ref().map(ToString::to_string);

        // Only a change from the owner followed counts. That leaves out the
        // changes queued before startup looked the owner up: the startup
        // owner appearing (D-Bus-activated by starting the service), and an
        // earlier instance leaving.
        if previous.as_deref() != self.link.owner() || owner.as_deref() == self.link.owner() {
            return;
        }

        // Everything belonged to the previous instance.
        self.registry.clear();
        self.pending.clear();
        self.pairing.clear();
        self.discovery.clear();

        self.link = match owner {
            Some(owner) => {
                debug!(%owner, "bluetoothd appeared");
                Link::syncing(&self.connection, owner, Duration::ZERO)
            }
            None => {
                debug!("bluetoothd left the bus");
                Link::Down
            }
        };
    }

    async fn end_timed_discoveries(&mut self) {
        for path in self.discovery.expired(Instant::now()) {
            match self.registry.adapter(&path).cloned() {
                Some(adapter) => self.discover(&adapter, false, None).await,
                None => self.discovery.forget(&path),
            }
        }
    }

    /// Runs `query` against BlueZ from its own task, so a slow answer doesn't
    /// hold up the dispatcher.
    fn query(&self, query: Query) {
        let connection = self.connection.clone();

        self.spawn(async move {
            match query {
                Query::ServiceRecords { device, reply } => {
                    let result = async {
                        let proxy = proxy::device1(&connection, &device).await?;
                        Ok(proxy.get_service_records().await?)
                    };
                    let _ = reply.send(result.await);
                }
                Query::DiscoveryFilters { adapter, reply } => {
                    let result = async {
                        let proxy = proxy::adapter1(&connection, &adapter).await?;
                        Ok(proxy.get_discovery_filters().await?)
                    };
                    let _ = reply.send(result.await);
                }
                Query::ConnectDevice {
                    adapter,
                    properties,
                    reply,
                } => {
                    let result = async {
                        let proxy = proxy::adapter1(&connection, &adapter).await?;
                        let properties = properties
                            .into_iter()
                            .map(|(key, value)| (key, value.into()))
                            .collect();
                        Ok(proxy.connect_device(properties).await?)
                    };
                    let _ = reply.send(result.await);
                }
            }
        });
    }

    /// Runs `task` on its own, until it's done or the service is gone: a task
    /// must not keep the connection open (and with it this client's agent and
    /// discovery sessions) once the service is dropped.
    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(
            self.cancellation_token
                .clone()
                .run_until_cancelled_owned(task),
        );
    }
}

/// The request starting (`start`) or stopping discovery on `adapter`.
fn discovery_request(adapter: &Arc<Adapter>, start: bool) -> (Call, Target) {
    let member = if start {
        "StartDiscovery"
    } else {
        "StopDiscovery"
    };

    (
        Call::method(&adapter.object_path, ADAPTER_INTERFACE, member),
        Target::Discovery {
            adapter: Arc::clone(adapter),
            start,
        },
    )
}

/// Whether turning Bluetooth on (`on`) or off powers `adapter` through BlueZ,
/// where KDE powers every adapter.
///
/// Without rfkill (`blockable` false), every adapter is. Through rfkill, a
/// block powers every adapter off, and BlueZ powers an adapter it reports
/// blocked back on itself once unblocked; only an adapter that is off for
/// another reason (its `Powered` set false) still needs powering on.
fn powered_through_bluez(on: bool, blockable: bool, adapter: &AdapterInfo) -> bool {
    !blockable || (on && !adapter.power_target() && adapter.power_state != PowerState::OffBlocked)
}

/// Whether the reply to a request means the pairing it may have involved is
/// over: a request that may pair finished, unless BlueZ turned it down as a
/// duplicate of one still under way.
fn ends_pairing(action: DeviceAction, result: &Result<(), Error>) -> bool {
    let duplicate = result
        .as_ref()
        .is_err_and(|err| err.is_bluez_error(BLUEZ_IN_PROGRESS));

    action.may_pair() && !duplicate
}

/// Registers the pairing agent with a bluetoothd instance. It can only fail
/// for a bug on our side (BlueZ rejects only malformed registrations), so it
/// is not retried.
async fn register_agent(connection: Connection) {
    if let Err(err) = agent::register(&connection).await {
        error!(error = %err, "cannot register bluetooth pairing agent");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(powered: bool, power_state: PowerState) -> AdapterInfo {
        AdapterInfo {
            powered,
            power_state,
            ..AdapterInfo::empty()
        }
    }

    #[test]
    fn without_rfkill_every_adapter_is_powered_through_bluez() {
        for on in [true, false] {
            assert!(powered_through_bluez(
                on,
                false,
                &adapter(!on, PowerState::Off)
            ));
        }
    }

    #[test]
    fn a_block_powers_adapters_off_by_itself() {
        assert!(!powered_through_bluez(
            false,
            true,
            &adapter(true, PowerState::On)
        ));
    }

    #[test]
    fn turning_on_powers_only_adapters_off_for_another_reason() {
        // BlueZ powers a blocked adapter on itself once unblocked.
        assert!(!powered_through_bluez(
            true,
            true,
            &adapter(false, PowerState::OffBlocked)
        ));
        // Powered off through BlueZ: no block to lift.
        assert!(powered_through_bluez(
            true,
            true,
            &adapter(false, PowerState::Off)
        ));
        // Already on, or turning on.
        assert!(!powered_through_bluez(
            true,
            true,
            &adapter(true, PowerState::On)
        ));
        assert!(!powered_through_bluez(
            true,
            true,
            &adapter(false, PowerState::OffToOn)
        ));
    }
}
