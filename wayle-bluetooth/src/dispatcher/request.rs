//! Sending requests to BlueZ in order, and matching their replies.
//!
//! The dispatcher builds and sends each request itself, one at a time, so they
//! reach BlueZ in the order they were queued (zbus's own calls send when first
//! polled, which would let concurrent calls overtake each other). A reply is
//! matched to its request by serial number. Replies arrive on the dispatcher's
//! own subscription, interleaved with BlueZ's signals in the order BlueZ sent
//! them (e.g. a `Disconnect` reply before the `Connected=false` it caused).

use std::{collections::HashMap, num::NonZeroU32, sync::Arc};

use zbus::{
    Connection, Message,
    message::Type,
    zvariant::{OwnedObjectPath, Value},
};

use crate::{
    core::{
        adapter::Adapter,
        device::{Device, activity::Ticket},
    },
    error::Error,
    types::{
        BLUEZ_SERVICE, PROPERTIES_INTERFACE,
        adapter::{AdapterAction, DiscoveryFilter},
        device::DeviceAction,
    },
};

/// A method call to a BlueZ object.
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
    Str(String),
    Path(OwnedObjectPath),
    Dict(DiscoveryFilter<'static>),
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

    pub(crate) fn method_with_str(
        path: &OwnedObjectPath,
        interface: &'static str,
        member: &'static str,
        argument: String,
    ) -> Self {
        Self {
            body: Body::Str(argument),
            ..Self::method(path, interface, member)
        }
    }

    pub(crate) fn method_with_path(
        path: &OwnedObjectPath,
        interface: &'static str,
        member: &'static str,
        argument: &OwnedObjectPath,
    ) -> Self {
        Self {
            body: Body::Path(argument.clone()),
            ..Self::method(path, interface, member)
        }
    }

    pub(crate) fn method_with_dict(
        path: &OwnedObjectPath,
        interface: &'static str,
        member: &'static str,
        argument: DiscoveryFilter<'static>,
    ) -> Self {
        Self {
            body: Body::Dict(argument),
            ..Self::method(path, interface, member)
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

    fn message(&self) -> zbus::Result<Message> {
        let builder = Message::method_call(self.path.clone(), self.member)?
            .destination(BLUEZ_SERVICE)?
            .interface(self.interface)?;

        match &self.body {
            Body::Empty => builder.build(&()),
            Body::Str(argument) => builder.build(&(argument.as_str(),)),
            Body::Path(argument) => builder.build(&(argument,)),
            Body::Dict(argument) => builder.build(&(argument,)),
            Body::Property {
                interface,
                name,
                value,
            } => builder.build(&(interface, name, value)),
        }
    }
}

/// What a request was for, to apply its reply: the instance it was sent for,
/// so a reply can't reach a replacement at the same path.
#[derive(Debug)]
pub(crate) enum Target {
    Device {
        device: Arc<Device>,
        action: DeviceAction,
        ticket: Ticket,
    },
    Adapter {
        adapter: Arc<Adapter>,
        action: AdapterAction,
    },
    /// Starting (`start`) or stopping discovery.
    Discovery { adapter: Arc<Adapter>, start: bool },
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
    /// its result.
    pub(crate) fn take_reply(&mut self, message: &Message) -> Option<(Target, Result<(), Error>)> {
        let result = match message.message_type() {
            Type::MethodReturn => Ok(()),
            Type::Error => Err(Error::Dbus(zbus::Error::from(message.clone()))),
            Type::MethodCall | Type::Signal => return None,
        };
        let serial = message.header().reply_serial()?;
        let target = self.0.remove(&serial)?;

        Some((target, result))
    }

    /// The device a request that may pair it (see [`DeviceAction::may_pair`])
    /// was sent for, if one for the device at `path` awaits BlueZ's reply.
    pub(crate) fn pairing(&self, path: &str) -> Option<Arc<Device>> {
        self.0.values().find_map(|target| match target {
            Target::Device { device, action, .. }
                if device.object_path.as_str() == path && action.may_pair() =>
            {
                Some(Arc::clone(device))
            }
            _ => None,
        })
    }

    /// Forgets the discovery requests pending for `adapter`: BlueZ ended its
    /// sessions (the adapter powered off or went away), and doesn't answer
    /// every request it dropped with them.
    pub(crate) fn forget_discovery(&mut self, adapter: &str) {
        self.0.retain(|_, target| {
            !matches!(target, Target::Discovery { adapter: at, .. } if at.object_path.as_str() == adapter)
        });
    }

    /// Forgets every pending request: bluetoothd went away, so their replies
    /// will never come (or will come from a different instance).
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use zbus::MessageStream;

    use super::*;
    use crate::{
        dispatcher::command::Commands,
        props::PropertyMap,
        test_support::connection,
        types::{ADAPTER_INTERFACE, DEVICE_INTERFACE},
    };

    const ADAPTER: &str = "/org/bluez/hci0";
    const DEVICE: &str = "/org/bluez/hci0/dev_00_11_22_33_44_55";

    fn path(path: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(path).unwrap()
    }

    fn device_target(action: DeviceAction) -> Target {
        let (commands, _) = Commands::channel();
        let device = Device::new(&commands, path(DEVICE), PropertyMap::new(), None);
        Target::Device {
            device: Arc::new(device),
            action,
            ticket: Ticket::Untracked,
        }
    }

    fn action(target: &Target) -> DeviceAction {
        match target {
            Target::Device { action, .. } => *action,
            Target::Adapter { .. } | Target::Discovery { .. } => panic!("not a device request"),
        }
    }

    #[test]
    fn a_property_write_is_a_properties_set_call() {
        let call = Call::set_property(&path(ADAPTER), ADAPTER_INTERFACE, "Powered", true);
        let message = call.message().unwrap();
        let header = message.header();

        assert_eq!(header.destination().unwrap().as_str(), BLUEZ_SERVICE);
        assert_eq!(header.path().unwrap().as_str(), ADAPTER);
        assert_eq!(header.interface().unwrap().as_str(), PROPERTIES_INTERFACE);
        assert_eq!(header.member().unwrap().as_str(), "Set");
        let body = message.body();
        let (interface, name, value): (&str, &str, Value<'_>) = body.deserialize().unwrap();
        assert_eq!((interface, name), (ADAPTER_INTERFACE, "Powered"));
        assert_eq!(value, Value::from(true));
    }

    #[test]
    fn a_method_call_carries_its_argument() {
        let call = Call::method_with_path(
            &path(ADAPTER),
            ADAPTER_INTERFACE,
            "RemoveDevice",
            &path(DEVICE),
        );
        let message = call.message().unwrap();
        let header = message.header();

        assert_eq!(header.path().unwrap().as_str(), ADAPTER);
        assert_eq!(header.interface().unwrap().as_str(), ADAPTER_INTERFACE);
        assert_eq!(header.member().unwrap().as_str(), "RemoveDevice");
        let body = message.body();
        let (device,): (OwnedObjectPath,) = body.deserialize().unwrap();
        assert_eq!(device.as_str(), DEVICE);
    }

    #[tokio::test]
    async fn a_connect_awaiting_its_reply_may_be_pairing() {
        let (client, server) = connection().await;
        let mut received = MessageStream::from(&server);
        let mut pending = Pending::default();

        let call = Call::method(&path(DEVICE), DEVICE_INTERFACE, "SetTrusted");
        pending
            .send(&client, call, device_target(DeviceAction::SetTrusted))
            .await
            .unwrap();
        assert!(
            pending.pairing(DEVICE).is_none(),
            "not a request that pairs"
        );

        let call = Call::method(&path(DEVICE), DEVICE_INTERFACE, "Connect");
        pending
            .send(&client, call, device_target(DeviceAction::Connect))
            .await
            .unwrap();
        assert!(pending.pairing(DEVICE).is_some());
        assert!(pending.pairing(ADAPTER).is_none(), "another object");

        let _trusted = received.next().await.unwrap().unwrap();
        let connect = received.next().await.unwrap().unwrap();
        let reply = Message::method_return(&connect.header())
            .unwrap()
            .build(&())
            .unwrap();
        let _ = pending.take_reply(&reply).unwrap();
        assert!(pending.pairing(DEVICE).is_none(), "answered");
    }

    #[tokio::test]
    async fn each_reply_completes_the_request_it_answers() {
        let (client, server) = connection().await;
        let mut received = MessageStream::from(&server);
        let mut pending = Pending::default();

        for (member, action) in [
            ("Connect", DeviceAction::Connect),
            ("Disconnect", DeviceAction::Disconnect),
        ] {
            let call = Call::method(&path(DEVICE), DEVICE_INTERFACE, member);
            pending
                .send(&client, call, device_target(action))
                .await
                .unwrap();
        }
        let connect = received.next().await.unwrap().unwrap();
        let disconnect = received.next().await.unwrap().unwrap();
        assert_eq!(connect.header().member().unwrap().as_str(), "Connect");
        assert_eq!(disconnect.header().member().unwrap().as_str(), "Disconnect");

        let reply = Message::method_return(&disconnect.header())
            .unwrap()
            .build(&())
            .unwrap();
        let (target, result) = pending.take_reply(&reply).unwrap();
        assert_eq!(action(&target), DeviceAction::Disconnect);
        assert!(result.is_ok());
        assert!(
            pending.take_reply(&reply).is_none(),
            "a reply is taken once"
        );

        let error = Message::error(&connect.header(), "org.bluez.Error.Failed")
            .unwrap()
            .build(&("br-connection-page-timeout",))
            .unwrap();
        let (target, result) = pending.take_reply(&error).unwrap();
        assert_eq!(action(&target), DeviceAction::Connect);
        assert_eq!(
            result.unwrap_err().bluez_error(),
            Some(("org.bluez.Error.Failed", Some("br-connection-page-timeout")))
        );
    }

    #[tokio::test]
    async fn other_messages_are_not_replies() {
        let (client, server) = connection().await;
        let mut received = MessageStream::from(&server);
        let mut pending = Pending::default();
        let call = Call::method(&path(DEVICE), DEVICE_INTERFACE, "Connect");
        pending
            .send(&client, call, device_target(DeviceAction::Connect))
            .await
            .unwrap();
        let request = received.next().await.unwrap().unwrap();

        let unrelated_call = Call::method(&path(DEVICE), DEVICE_INTERFACE, "Pair")
            .message()
            .unwrap();
        let unrelated_reply = Message::method_return(&unrelated_call.header())
            .unwrap()
            .build(&())
            .unwrap();
        let signal = Message::signal(DEVICE, PROPERTIES_INTERFACE, "PropertiesChanged")
            .unwrap()
            .build(&())
            .unwrap();

        assert!(pending.take_reply(&unrelated_reply).is_none());
        assert!(pending.take_reply(&signal).is_none());
        assert!(pending.take_reply(&request).is_none(), "the request itself");
        let reply = Message::method_return(&request.header())
            .unwrap()
            .build(&())
            .unwrap();
        assert!(pending.take_reply(&reply).is_some(), "still pending");
    }
}
