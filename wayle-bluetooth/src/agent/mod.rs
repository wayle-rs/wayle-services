//! The pairing agent's side of BlueZ's `org.bluez.Agent1` protocol.
//!
//! BlueZ calls the agent on the service's own connection. There is no zbus
//! object server: the calls arrive on the dispatcher's subscription to
//! bluetoothd, in order with BlueZ's signals, are decoded here, and are
//! answered by the dispatcher (see `dispatcher::pairing`).

use tracing::warn;
use zbus::{
    Connection, Message,
    message::{Flags, Type},
    proxy::CacheProperties,
    zvariant::{ObjectPath, OwnedObjectPath},
};

use crate::{
    Error,
    proxy::agent_manager::AgentManager1Proxy,
    types::agent::{AgentCapability, PairingRequest},
};

/// Object path of the pairing agent on the service's connection.
pub(crate) const AGENT_PATH: ObjectPath<'static> =
    ObjectPath::from_static_str_unchecked("/com/wayle/BluetoothAgent");

const AGENT_INTERFACE: &str = "org.bluez.Agent1";

/// BlueZ's error for registering a second agent from the same connection.
const BLUEZ_ALREADY_EXISTS: &str = "org.bluez.Error.AlreadyExists";

/// The agent's answer to a request the user turned down.
const BLUEZ_REJECTED: &str = "org.bluez.Error.Rejected";

/// The agent's answer to a request that no longer stands.
const BLUEZ_CANCELED: &str = "org.bluez.Error.Canceled";

/// The answer to a call this connection doesn't handle.
pub(crate) const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";

const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";

/// Registers the pairing agent with the current bluetoothd, as its default
/// agent: BlueZ sends the default agent the prompts of pairings no agent asked
/// for, such as a connect that needs pairing or one the remote device starts.
/// BlueZ restores the previous default once this agent goes away.
///
/// BlueZ forgets agents when it restarts, so this runs again for every new
/// instance.
pub(crate) async fn register(connection: &Connection) -> Result<(), Error> {
    let agent_manager = AgentManager1Proxy::builder(connection)
        .cache_properties(CacheProperties::No)
        .build()
        .await?;

    match agent_manager
        .register_agent(&AGENT_PATH, &AgentCapability::DisplayYesNo.to_string())
        .await
    {
        Ok(()) => {}
        // Already registered with this bluetoothd instance.
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == BLUEZ_ALREADY_EXISTS => {}
        Err(err) => return Err(Error::AgentRegistration(Box::new(err))),
    }

    agent_manager
        .request_default_agent(&AGENT_PATH)
        .await
        .map_err(|err| Error::AgentRegistration(Box::new(err)))
}

/// A call from BlueZ to the pairing agent.
#[derive(Debug)]
pub(crate) enum AgentCall {
    /// A request BlueZ awaits the user's answer to.
    Ask(PairingRequest),
    /// A passkey or PIN to display; BlueZ expects no answer.
    Show(PairingRequest),
    /// BlueZ no longer needs the answer it asked for.
    Cancel,
    /// BlueZ no longer uses the agent.
    Release,
}

impl AgentCall {
    /// Decodes a method call to this connection. Returns the D-Bus error to
    /// answer with if it isn't a well-formed call to the agent.
    pub(crate) fn parse(call: &Message) -> Result<Self, &'static str> {
        let header = call.header();
        let for_agent = header
            .path()
            .is_some_and(|path| path.as_str() == AGENT_PATH.as_str())
            && header
                .interface()
                .is_none_or(|interface| interface.as_str() == AGENT_INTERFACE);
        if call.message_type() != Type::MethodCall || !for_agent {
            return Err(UNKNOWN_METHOD);
        }

        let body = call.body();
        let decoded = match header.member().map(|member| member.as_str()) {
            Some("RequestPinCode") => body
                .deserialize::<(OwnedObjectPath,)>()
                .map(|(device_path,)| Self::Ask(PairingRequest::RequestPinCode { device_path })),
            Some("DisplayPinCode") => {
                body.deserialize::<(OwnedObjectPath, String)>()
                    .map(|(device_path, pincode)| {
                        Self::Show(PairingRequest::DisplayPinCode {
                            device_path,
                            pincode,
                        })
                    })
            }
            Some("RequestPasskey") => body
                .deserialize::<(OwnedObjectPath,)>()
                .map(|(device_path,)| Self::Ask(PairingRequest::RequestPasskey { device_path })),
            Some("DisplayPasskey") => body.deserialize::<(OwnedObjectPath, u32, u16)>().map(
                |(device_path, passkey, entered)| {
                    Self::Show(PairingRequest::DisplayPasskey {
                        device_path,
                        passkey,
                        entered,
                    })
                },
            ),
            Some("RequestConfirmation") => {
                body.deserialize::<(OwnedObjectPath, u32)>()
                    .map(|(device_path, passkey)| {
                        Self::Ask(PairingRequest::RequestConfirmation {
                            device_path,
                            passkey,
                        })
                    })
            }
            Some("RequestAuthorization") => {
                body.deserialize::<(OwnedObjectPath,)>()
                    .map(|(device_path,)| {
                        Self::Ask(PairingRequest::RequestAuthorization { device_path })
                    })
            }
            Some("AuthorizeService") => {
                body.deserialize::<(OwnedObjectPath, String)>()
                    .map(|(device_path, uuid)| {
                        Self::Ask(PairingRequest::RequestServiceAuthorization { device_path, uuid })
                    })
            }
            Some("Cancel") => Ok(Self::Cancel),
            Some("Release") => Ok(Self::Release),
            _ => return Err(UNKNOWN_METHOD),
        };

        decoded.map_err(|_| INVALID_ARGS)
    }
}

/// What the agent answers a call with.
#[derive(Debug)]
pub(crate) enum Answer {
    /// Success, for methods that return nothing.
    Done,
    Pin(String),
    Passkey(u32),
    /// The user turned the request down.
    Rejected,
    /// The request no longer stands.
    Canceled,
}

/// Builds the reply to `call`, unless its caller expects none.
pub(crate) fn reply(call: &Message, answer: Answer) -> Option<Message> {
    if !expects_reply(call) {
        return None;
    }

    let header = call.header();
    let reply = match answer {
        Answer::Done => Message::method_return(&header).and_then(|reply| reply.build(&())),
        Answer::Pin(pin) => {
            Message::method_return(&header).and_then(|reply| reply.build(&(pin.as_str(),)))
        }
        Answer::Passkey(passkey) => {
            Message::method_return(&header).and_then(|reply| reply.build(&(passkey,)))
        }
        Answer::Rejected => error(call, BLUEZ_REJECTED, "rejected by the user"),
        Answer::Canceled => error(call, BLUEZ_CANCELED, "canceled"),
    };

    reply
        .inspect_err(|err| warn!(error = %err, "cannot build pairing agent reply"))
        .ok()
}

/// Builds the error reply `name` to `call`, unless its caller expects none.
pub(crate) fn reply_error(call: &Message, name: &'static str) -> Option<Message> {
    if !expects_reply(call) {
        return None;
    }

    error(call, name, "not handled by the bluetooth pairing agent")
        .inspect_err(|err| warn!(error = %err, "cannot build pairing agent reply"))
        .ok()
}

fn error(call: &Message, name: &'static str, text: &str) -> zbus::Result<Message> {
    Message::error(&call.header(), name).and_then(|reply| reply.build(&(text,)))
}

fn expects_reply(call: &Message) -> bool {
    !call
        .primary_header()
        .flags()
        .contains(Flags::NoReplyExpected)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "/org/bluez/hci0/dev_00_11_22_33_44_55";

    fn device() -> ObjectPath<'static> {
        ObjectPath::from_static_str_unchecked(DEVICE)
    }

    fn agent_call(member: &'static str) -> zbus::message::Builder<'static> {
        Message::method_call(AGENT_PATH, member)
            .unwrap()
            .interface(AGENT_INTERFACE)
            .unwrap()
    }

    #[test]
    fn a_confirmation_request_is_asked() {
        let call = agent_call("RequestConfirmation")
            .build(&(device(), 123_456_u32))
            .unwrap();

        let Ok(AgentCall::Ask(PairingRequest::RequestConfirmation { passkey, .. })) =
            AgentCall::parse(&call)
        else {
            panic!("not a confirmation request");
        };
        assert_eq!(passkey, 123_456);
    }

    #[test]
    fn a_passkey_to_display_is_shown() {
        let call = agent_call("DisplayPasskey")
            .build(&(device(), 1_u32, 2_u16))
            .unwrap();

        assert!(matches!(
            AgentCall::parse(&call),
            Ok(AgentCall::Show(PairingRequest::DisplayPasskey {
                entered: 2,
                ..
            }))
        ));
    }

    #[test]
    fn calls_to_anything_else_are_unknown() {
        let elsewhere = Message::method_call("/", "RequestConfirmation")
            .unwrap()
            .interface(AGENT_INTERFACE)
            .unwrap()
            .build(&(device(), 1_u32))
            .unwrap();
        let introspect = Message::method_call(AGENT_PATH, "Introspect")
            .unwrap()
            .interface("org.freedesktop.DBus.Introspectable")
            .unwrap()
            .build(&())
            .unwrap();

        assert_eq!(AgentCall::parse(&elsewhere).unwrap_err(), UNKNOWN_METHOD);
        assert_eq!(AgentCall::parse(&introspect).unwrap_err(), UNKNOWN_METHOD);
    }

    #[test]
    fn malformed_calls_are_invalid() {
        let call = agent_call("RequestPasskey")
            .build(&("not a path",))
            .unwrap();

        assert_eq!(AgentCall::parse(&call).unwrap_err(), INVALID_ARGS);
    }

    #[test]
    fn a_pin_answer_carries_the_pin() {
        let call = agent_call("RequestPinCode").build(&(device(),)).unwrap();

        let reply = reply(&call, Answer::Pin("0000".to_owned())).unwrap();
        let body = reply.body();
        let (pin,): (String,) = body.deserialize().unwrap();
        assert_eq!(pin, "0000");
        assert_eq!(
            reply.header().reply_serial(),
            Some(call.primary_header().serial_num())
        );
    }

    #[test]
    fn a_rejection_is_bluez_rejected() {
        let call = agent_call("RequestConfirmation")
            .build(&(device(), 1_u32))
            .unwrap();

        let reply = reply(&call, Answer::Rejected).unwrap();
        assert_eq!(reply.message_type(), Type::Error);
        assert_eq!(
            reply.header().error_name().unwrap().as_str(),
            BLUEZ_REJECTED
        );
    }

    #[test]
    fn calls_expecting_no_reply_get_none() {
        let call = Message::method_call(AGENT_PATH, "DisplayPasskey")
            .unwrap()
            .with_flags(Flags::NoReplyExpected)
            .unwrap()
            .build(&(device(), 1_u32, 0_u16))
            .unwrap();

        assert!(reply(&call, Answer::Done).is_none());
        assert!(reply_error(&call, UNKNOWN_METHOD).is_none());
    }
}
