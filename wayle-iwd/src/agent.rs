//! This service's side of IWD's agent protocols: the passphrase agent
//! (`net.connman.iwd.Agent`) and the signal-level agent
//! (`net.connman.iwd.SignalLevelAgent`).
//!
//! IWD calls the agents on the service's own connection. There is no zbus
//! object server: the calls arrive on the dispatcher's subscription to IWD, in
//! order with IWD's signals, are decoded here, and are answered by the
//! dispatcher.

use tracing::warn;
use zbus::{
    Message,
    message::{Flags, Type},
    zvariant::OwnedObjectPath,
};

/// Object path of the passphrase agent.
pub(crate) const AGENT_PATH: &str = "/wayle/iwd/agent";

/// Object path of the signal-level agent.
pub(crate) const SIGNAL_LEVEL_AGENT_PATH: &str = "/wayle/iwd/signal_level_agent";

const AGENT_INTERFACE: &str = "net.connman.iwd.Agent";
const SIGNAL_LEVEL_AGENT_INTERFACE: &str = "net.connman.iwd.SignalLevelAgent";

/// The agent's answer to a request it can't or won't satisfy.
const AGENT_CANCELED: &str = "net.connman.iwd.Agent.Error.Canceled";

/// The answer to a call this connection doesn't handle.
pub(crate) const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";

const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";

/// A call from IWD to one of the agents.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AgentCall {
    /// IWD asks for the passphrase of `network`, and awaits the answer.
    RequestPassphrase(OwnedObjectPath),
    /// Credentials this service doesn't provide (enterprise networks).
    Unsupported,
    /// IWD no longer needs the passphrase it asked for.
    Cancel,
    /// IWD no longer uses the passphrase agent.
    Release,
    /// The connected link's signal level changed: `0` is the strongest.
    SignalLevel { device: OwnedObjectPath, level: u8 },
    /// IWD no longer uses the signal-level agent.
    SignalLevelReleased,
}

impl AgentCall {
    /// Decodes a method call to this connection. Returns the D-Bus error to
    /// answer with if it isn't a well-formed call to an agent.
    pub(crate) fn parse(call: &Message) -> Result<Self, &'static str> {
        if call.message_type() != Type::MethodCall {
            return Err(UNKNOWN_METHOD);
        }
        let header = call.header();
        let to = |path: &str, interface: &str| {
            header.path().is_some_and(|at| at.as_str() == path)
                && header.interface().is_none_or(|at| at.as_str() == interface)
        };
        let member = header.member().map(|member| member.as_str());
        let body = call.body();

        let decoded = if to(AGENT_PATH, AGENT_INTERFACE) {
            match member {
                Some("RequestPassphrase") => body
                    .deserialize::<(OwnedObjectPath,)>()
                    .map(|(network,)| Self::RequestPassphrase(network)),
                Some(
                    "RequestPrivateKeyPassphrase"
                    | "RequestUserNameAndPassword"
                    | "RequestUserPassword",
                ) => Ok(Self::Unsupported),
                Some("Cancel") => Ok(Self::Cancel),
                Some("Release") => Ok(Self::Release),
                _ => return Err(UNKNOWN_METHOD),
            }
        } else if to(SIGNAL_LEVEL_AGENT_PATH, SIGNAL_LEVEL_AGENT_INTERFACE) {
            match member {
                Some("Changed") => body
                    .deserialize::<(OwnedObjectPath, u8)>()
                    .map(|(device, level)| Self::SignalLevel { device, level }),
                Some("Release") => Ok(Self::SignalLevelReleased),
                _ => return Err(UNKNOWN_METHOD),
            }
        } else {
            return Err(UNKNOWN_METHOD);
        };

        decoded.map_err(|_| INVALID_ARGS)
    }
}

/// What the agent answers a call with.
#[derive(Debug)]
pub(crate) enum Answer {
    /// Success, for methods that return nothing.
    Done,
    Passphrase(String),
    /// The request was turned down, or no longer stands.
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
        Answer::Passphrase(passphrase) => {
            Message::method_return(&header).and_then(|reply| reply.build(&(passphrase.as_str(),)))
        }
        Answer::Canceled => error(call, AGENT_CANCELED, "canceled"),
    };

    reply
        .inspect_err(|err| warn!(error = %err, "cannot build iwd agent reply"))
        .ok()
}

/// Builds the error reply `name` to `call`, unless its caller expects none.
pub(crate) fn reply_error(call: &Message, name: &'static str) -> Option<Message> {
    if !expects_reply(call) {
        return None;
    }

    error(call, name, "not handled by this iwd agent")
        .inspect_err(|err| warn!(error = %err, "cannot build iwd agent reply"))
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
    use zbus::zvariant::ObjectPath;

    use super::*;

    const NETWORK: &str = "/net/connman/iwd/0/4/6d79776966695f70736b";

    fn call(
        path: &'static str,
        interface: &'static str,
        member: &'static str,
    ) -> zbus::message::Builder<'static> {
        Message::method_call(path, member)
            .unwrap()
            .interface(interface)
            .unwrap()
    }

    #[test]
    fn a_passphrase_request_is_decoded() {
        let message = call(AGENT_PATH, AGENT_INTERFACE, "RequestPassphrase")
            .build(&(ObjectPath::from_static_str_unchecked(NETWORK),))
            .unwrap();

        assert_eq!(
            AgentCall::parse(&message),
            Ok(AgentCall::RequestPassphrase(
                OwnedObjectPath::try_from(NETWORK).unwrap()
            ))
        );
    }

    #[test]
    fn a_signal_level_change_is_decoded() {
        let message = call(
            SIGNAL_LEVEL_AGENT_PATH,
            SIGNAL_LEVEL_AGENT_INTERFACE,
            "Changed",
        )
        .build(&(
            ObjectPath::from_static_str_unchecked("/net/connman/iwd/0/4"),
            2_u8,
        ))
        .unwrap();

        assert!(matches!(
            AgentCall::parse(&message),
            Ok(AgentCall::SignalLevel { level: 2, .. })
        ));
    }

    #[test]
    fn calls_to_other_objects_are_unknown() {
        let message = call("/elsewhere", AGENT_INTERFACE, "RequestPassphrase")
            .build(&(ObjectPath::from_static_str_unchecked(NETWORK),))
            .unwrap();

        assert_eq!(AgentCall::parse(&message), Err(UNKNOWN_METHOD));
    }

    #[test]
    fn a_passphrase_is_the_reply_body() {
        let request = call(AGENT_PATH, AGENT_INTERFACE, "RequestPassphrase")
            .build(&(ObjectPath::from_static_str_unchecked(NETWORK),))
            .unwrap();

        let reply = reply(&request, Answer::Passphrase("hunter22".to_owned())).unwrap();

        assert_eq!(reply.message_type(), Type::MethodReturn);
        let body = reply.body();
        let (passphrase,): (String,) = body.deserialize().unwrap();
        assert_eq!(passphrase, "hunter22");
    }
}
