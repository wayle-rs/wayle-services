//! Sending requests to IWD in order, and matching their replies.
//!
//! The dispatcher builds and sends each request itself, so they reach IWD in
//! the order they were queued. A reply is matched to its request by serial
//! number; replies arrive on the dispatcher's own subscription, interleaved
//! with IWD's signals in the order IWD sent them.

use std::{collections::HashMap, num::NonZeroU32, sync::Arc};

use zbus::{
    Connection, Message,
    message::Type,
    zvariant::{OwnedObjectPath, Value},
};

use super::IWD_SERVICE;
use crate::{error::Error, network::Network, station::Station, types::StationAction};

const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";

/// A method call to an IWD object.
#[derive(Debug)]
pub(crate) struct Call {
    path: OwnedObjectPath,
    interface: &'static str,
    member: &'static str,
    body: Body,
}

#[derive(Debug)]
enum Body {
    Empty,
    Path(OwnedObjectPath),
    /// `RegisterSignalLevelAgent(path, levels)`.
    SignalLevels(OwnedObjectPath, Vec<i16>),
    /// `org.freedesktop.DBus.Properties.Set(interface, name, value)`.
    Property {
        interface: &'static str,
        name: &'static str,
        value: Value<'static>,
    },
}

impl Call {
    pub(crate) fn method(
        path: &OwnedObjectPath,
        interface: &'static str,
        member: &'static str,
    ) -> Self {
        Self {
            path: path.clone(),
            interface,
            member,
            body: Body::Empty,
        }
    }

    pub(crate) fn method_with_path(
        path: &OwnedObjectPath,
        interface: &'static str,
        member: &'static str,
        argument: OwnedObjectPath,
    ) -> Self {
        Self {
            body: Body::Path(argument),
            ..Self::method(path, interface, member)
        }
    }

    pub(crate) fn register_signal_levels(
        path: &OwnedObjectPath,
        interface: &'static str,
        agent: OwnedObjectPath,
        levels: Vec<i16>,
    ) -> Self {
        Self {
            body: Body::SignalLevels(agent, levels),
            ..Self::method(path, interface, "RegisterSignalLevelAgent")
        }
    }

    /// Sets property `name` of `interface` on the object at `path`.
    pub(crate) fn set_property(
        path: &OwnedObjectPath,
        interface: &'static str,
        name: &'static str,
        value: impl Into<Value<'static>>,
    ) -> Self {
        Self {
            path: path.clone(),
            interface: PROPERTIES_INTERFACE,
            member: "Set",
            body: Body::Property {
                interface,
                name,
                value: value.into(),
            },
        }
    }

    pub(crate) fn message(&self) -> zbus::Result<Message> {
        let builder = Message::method_call(self.path.clone(), self.member)?
            .destination(IWD_SERVICE)?
            .interface(self.interface)?;

        match &self.body {
            Body::Empty => builder.build(&()),
            Body::Path(argument) => builder.build(&(argument,)),
            Body::SignalLevels(agent, levels) => builder.build(&(agent, levels)),
            Body::Property {
                interface,
                name,
                value,
            } => builder.build(&(interface, name, value)),
        }
    }
}

/// What a request was for, to apply its reply: the instance it was sent for,
/// so a reply can't reach a replacement.
#[derive(Debug)]
pub(crate) enum Target {
    /// Connecting `station` to `network`.
    Connect {
        station: Arc<Station>,
        network: Arc<Network>,
    },
    /// Any other request of the station's, whose failure is recorded.
    Action {
        station: Arc<Station>,
        action: StationAction,
    },
    /// Asking for the station's device to be up while its radio is blocked,
    /// which fails until it is unblocked (see `Dispatcher::set_powered`).
    DeviceWantedUp,
    /// Reading the station's networks in IWD's order.
    OrderedNetworks(Arc<Station>),
    /// Reading the connected link's diagnostics.
    Diagnostics(Arc<Station>),
    /// Registering the signal-level agent for the station.
    SignalLevelAgent,
    /// Registering the passphrase agent.
    Agent,
}

/// Requests sent and awaiting their reply, by serial number.
#[derive(Debug, Default)]
pub(crate) struct Pending(HashMap<NonZeroU32, Target>);

impl Pending {
    /// Sends `call`. Returns the target with the error if it couldn't be sent,
    /// which counts as the request failing.
    pub(crate) async fn send(
        &mut self,
        connection: &Connection,
        call: Call,
        target: Target,
    ) -> Result<(), (Target, Error)> {
        let message = match call.message() {
            Ok(message) => message,
            Err(err) => return Err((target, err.into())),
        };
        let serial = message.primary_header().serial_num();

        match connection.send(&message).await {
            Ok(()) => {
                self.0.insert(serial, target);
                Ok(())
            }
            Err(err) => Err((target, err.into())),
        }
    }

    /// If `message` is the reply to a pending request, takes the request and
    /// the reply (or the error it carries).
    pub(crate) fn take_reply(
        &mut self,
        message: &Message,
    ) -> Option<(Target, Result<Message, Error>)> {
        let result = match message.message_type() {
            Type::MethodReturn => Ok(message.clone()),
            Type::Error => Err(Error::Dbus(zbus::Error::from(message.clone()))),
            Type::MethodCall | Type::Signal => return None,
        };
        let serial = message.header().reply_serial()?;
        let target = self.0.remove(&serial)?;

        Some((target, result))
    }

    /// Whether a connect to `network` this service sent is awaiting its reply.
    pub(crate) fn connecting(&self, network: &Arc<Network>) -> bool {
        self.0.values().any(|target| {
            matches!(target, Target::Connect { network: pending, .. } if Arc::ptr_eq(pending, network))
        })
    }

    /// Forgets every pending request: IWD went away, so their replies will
    /// never come (or will come from a different instance).
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use zbus::MessageStream;

    use super::*;
    use crate::{dispatcher::command::Commands, test_support::connection};

    const STATION: &str = "/net/connman/iwd/0/4";

    fn path(path: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(path).unwrap()
    }

    fn station() -> Arc<Station> {
        let (commands, _) = Commands::channel();
        Arc::new(Station::new(&commands, path(STATION)))
    }

    #[test]
    fn a_property_write_is_a_properties_set_call() {
        let call = Call::set_property(&path(STATION), "net.connman.iwd.Device", "Powered", true);
        let message = call.message().unwrap();
        let header = message.header();

        assert_eq!(header.destination().unwrap().as_str(), IWD_SERVICE);
        assert_eq!(header.interface().unwrap().as_str(), PROPERTIES_INTERFACE);
        assert_eq!(header.member().unwrap().as_str(), "Set");
        let body = message.body();
        let (interface, name, value): (&str, &str, Value<'_>) = body.deserialize().unwrap();
        assert_eq!((interface, name), ("net.connman.iwd.Device", "Powered"));
        assert_eq!(value, Value::from(true));
    }

    #[tokio::test]
    async fn each_reply_completes_the_request_it_answers() {
        let (client, server) = connection().await;
        let mut received = MessageStream::from(&server);
        let mut pending = Pending::default();

        for member in ["Scan", "Disconnect"] {
            let call = Call::method(&path(STATION), "net.connman.iwd.Station", member);
            let target = Target::Action {
                station: station(),
                action: StationAction::Scan,
            };
            pending.send(&client, call, target).await.unwrap();
        }
        let scan = received.next().await.unwrap().unwrap();
        let disconnect = received.next().await.unwrap().unwrap();

        let reply = Message::method_return(&disconnect.header())
            .unwrap()
            .build(&())
            .unwrap();
        assert!(pending.take_reply(&reply).unwrap().1.is_ok());
        assert!(
            pending.take_reply(&reply).is_none(),
            "a reply is taken once"
        );

        let error = Message::error(&scan.header(), "net.connman.iwd.Busy")
            .unwrap()
            .build(&("busy",))
            .unwrap();
        let (_, result) = pending.take_reply(&error).unwrap();
        assert!(result.unwrap_err().is_iwd_error("net.connman.iwd.Busy"));
    }
}
