//! BlueZ's calls to the pairing agent, and the request they publish.
//!
//! BlueZ asks the agent one question at a time (a PIN, a passkey, a
//! confirmation or an authorization) and cancels it itself (`Cancel`) once it
//! no longer needs the answer, with one exception: a PIN or passkey request
//! it gave up waiting for (after 60 seconds) just fails the authentication,
//! which disconnects the device. So a question also ends when its device
//! disconnects or goes away, as BlueZ's own requests about it do.
//!
//! A passkey or PIN to display expects no answer and gets no `Cancel`, so it
//! ends with the pairing: when its device becomes paired, disconnects or goes
//! away, or when a connect or pair this service sent for it finishes. The
//! published request is the question, if there is one, else what's displayed.
//!
//! Turning a pairing down (or cancelling a display) reports its device: the
//! dispatcher disconnects it if a connect or pair this service sent for it
//! still awaits BlueZ's answer. That is how BlueZ aborts the pairing of a
//! connect, and it makes that connect end as cancelled rather than failed.

use tracing::{debug, info, warn};
use wayle_core::Property;
use zbus::{Message, zvariant::OwnedObjectPath};

use super::{command::PairingResponse, registry::Changes};
use crate::{
    agent::{self, AgentCall, Answer},
    types::agent::PairingRequest,
};

pub(crate) struct Pairing {
    published: Property<Option<PairingRequest>>,
    /// BlueZ's call awaiting an answer, and the request it asks.
    asking: Option<(Message, PairingRequest)>,
    /// A passkey or PIN being displayed.
    showing: Option<PairingRequest>,
}

/// What answering the published request comes to.
#[derive(Debug, Default)]
#[must_use]
pub(crate) struct Answered {
    /// The reply to BlueZ's call, if the answer went to one.
    pub reply: Option<Message>,
    /// The device whose pairing the answer turned down or cancelled.
    pub turned_down: Option<OwnedObjectPath>,
}

impl Pairing {
    pub(crate) fn new(published: &Property<Option<PairingRequest>>) -> Self {
        Self {
            published: published.clone(),
            asking: None,
            showing: None,
        }
    }

    /// Takes a method call BlueZ made to this connection. Returns the replies
    /// to send, in order.
    #[must_use]
    pub(crate) fn on_call(&mut self, call: &Message) -> Vec<Message> {
        let mut replies = Vec::new();

        match AgentCall::parse(call) {
            Ok(AgentCall::Ask(request)) => {
                // BlueZ asks one question at a time: a new one means it no
                // longer waits for the previous one.
                if let Some((previous, _)) = self.asking.take() {
                    replies.extend(agent::reply(&previous, Answer::Canceled));
                }
                self.asking = Some((call.clone(), request));
            }
            Ok(AgentCall::Show(request)) => {
                self.showing = Some(request);
                replies.extend(agent::reply(call, Answer::Done));
            }
            Ok(AgentCall::Cancel) => {
                info!("pairing request cancelled by bluez");
                if let Some((asked, _)) = self.asking.take() {
                    // Frees the call; BlueZ has stopped waiting for it.
                    replies.extend(agent::reply(&asked, Answer::Canceled));
                }
                replies.extend(agent::reply(call, Answer::Done));
            }
            Ok(AgentCall::Release) => {
                debug!("pairing agent released by bluez");
                self.asking = None;
                self.showing = None;
                replies.extend(agent::reply(call, Answer::Done));
            }
            Err(error) => replies.extend(agent::reply_error(call, error)),
        }

        self.publish();
        replies
    }

    /// Answers the published request.
    pub(crate) fn respond(&mut self, response: PairingResponse) -> Answered {
        if matches!(response, PairingResponse::Cancel) {
            return self.cancel();
        }
        let Some((call, request)) = self.asking.take() else {
            warn!("pairing response ignored: no pairing request awaits an answer");
            return Answered::default();
        };

        let answer = match (response, &request) {
            (PairingResponse::Pin(pin), PairingRequest::RequestPinCode { .. }) => {
                Some(Answer::Pin(pin))
            }
            (PairingResponse::Passkey(passkey), PairingRequest::RequestPasskey { .. }) => {
                Some(Answer::Passkey(passkey))
            }
            (
                PairingResponse::Confirmation(accepted),
                PairingRequest::RequestConfirmation { .. },
            )
            | (
                PairingResponse::Authorization(accepted),
                PairingRequest::RequestAuthorization { .. },
            )
            | (
                PairingResponse::ServiceAuthorization(accepted),
                PairingRequest::RequestServiceAuthorization { .. },
            ) => Some(if accepted {
                Answer::Done
            } else {
                Answer::Rejected
            }),
            _ => None,
        };
        let Some(answer) = answer else {
            warn!(
                ?request,
                "pairing response ignored: it doesn't answer the pending request"
            );
            self.asking = Some((call, request));
            return Answered::default();
        };

        let turned_down = if matches!(answer, Answer::Rejected) {
            pairing_device(&request)
        } else {
            None
        };
        let reply = agent::reply(&call, answer);
        self.publish();
        Answered { reply, turned_down }
    }

    /// The user cancelled the published request: turns the question down, or
    /// stops displaying.
    fn cancel(&mut self) -> Answered {
        let answered = if let Some((call, request)) = self.asking.take() {
            Answered {
                reply: agent::reply(&call, Answer::Rejected),
                turned_down: pairing_device(&request),
            }
        } else if let Some(request) = self.showing.take() {
            Answered {
                reply: None,
                turned_down: Some(request.device_path().clone()),
            }
        } else {
            warn!("pairing cancel ignored: no pairing request is pending");
            return Answered::default();
        };

        self.publish();
        answered
    }

    /// Ends what BlueZ reports the end of: any request whose device
    /// disconnected or went away, and a display whose device became paired.
    /// Returns the reply freeing BlueZ's call, if a question ended.
    #[must_use]
    pub(crate) fn devices_changed(&mut self, changes: &Changes) -> Option<Message> {
        let concerns = |request: &PairingRequest, devices: &[String]| {
            devices
                .iter()
                .any(|path| path == request.device_path().as_str())
        };
        let mut reply = None;
        let mut ended = false;

        if let Some((call, request)) = &self.asking
            && (concerns(request, &changes.removed) || concerns(request, &changes.disconnected))
        {
            debug!(device = %request.device_path(), "pairing request's device is gone");
            reply = agent::reply(call, Answer::Canceled);
            self.asking = None;
            ended = true;
        }

        if let Some(request) = &self.showing
            && (concerns(request, &changes.removed)
                || concerns(request, &changes.paired)
                || concerns(request, &changes.disconnected))
        {
            debug!(device = %request.device_path(), "pairing is over; ending its display");
            self.showing = None;
            ended = true;
        }

        if ended {
            self.publish();
        }
        reply
    }

    /// A connect or pair this service sent for `device` finished, and with it
    /// any pairing it involved: ends a display for that device.
    pub(crate) fn pairing_ended(&mut self, device: &str) {
        if self
            .showing
            .as_ref()
            .is_some_and(|request| request.device_path().as_str() == device)
        {
            debug!(device, "pairing is over; ending its display");
            self.showing = None;
            self.publish();
        }
    }

    /// Forgets everything: the bluetoothd that asked is gone, so its calls
    /// can't be answered.
    pub(crate) fn clear(&mut self) {
        self.asking = None;
        self.showing = None;
        self.publish();
    }

    fn publish(&self) {
        let request = self
            .asking
            .as_ref()
            .map(|(_, request)| request.clone())
            .or_else(|| self.showing.clone());
        self.published.set(request);
    }
}

/// The device whose pairing turning `request` down aborts: any pairing's, but
/// not a paired device's asking to use a service.
fn pairing_device(request: &PairingRequest) -> Option<OwnedObjectPath> {
    (!matches!(request, PairingRequest::RequestServiceAuthorization { .. }))
        .then(|| request.device_path().clone())
}

#[cfg(test)]
mod tests {
    use zbus::{
        message::{Flags, Type},
        zvariant::{ObjectPath, OwnedObjectPath},
    };

    use super::*;
    use crate::agent::AGENT_PATH;

    const DEVICE: &str = "/org/bluez/hci0/dev_00_11_22_33_44_55";
    const OTHER: &str = "/org/bluez/hci0/dev_66_77_88_99_AA_BB";

    fn path(at: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(at).unwrap()
    }

    fn pairing() -> (Pairing, Property<Option<PairingRequest>>) {
        let request = Property::new(None);
        (Pairing::new(&request), request)
    }

    fn call(member: &'static str) -> zbus::message::Builder<'static> {
        Message::method_call(AGENT_PATH, member)
            .unwrap()
            .interface("org.bluez.Agent1")
            .unwrap()
    }

    fn confirmation(at: &'static str) -> Message {
        call("RequestConfirmation")
            .build(&(ObjectPath::from_static_str_unchecked(at), 123_456_u32))
            .unwrap()
    }

    /// As BlueZ sends it: expecting no reply.
    fn display_passkey(at: &'static str) -> Message {
        call("DisplayPasskey")
            .with_flags(Flags::NoReplyExpected)
            .unwrap()
            .build(&(
                ObjectPath::from_static_str_unchecked(at),
                123_456_u32,
                0_u16,
            ))
            .unwrap()
    }

    fn changes(paired: &[&str], disconnected: &[&str], removed: &[&str]) -> Changes {
        let owned = |paths: &[&str]| paths.iter().map(|path| (*path).to_owned()).collect();
        Changes {
            paired: owned(paired),
            disconnected: owned(disconnected),
            removed: owned(removed),
            ..Changes::default()
        }
    }

    /// Replies as (type, error name).
    fn kinds<'a>(replies: impl IntoIterator<Item = &'a Message>) -> Vec<(Type, Option<String>)> {
        replies
            .into_iter()
            .map(|reply| {
                (
                    reply.message_type(),
                    reply
                        .header()
                        .error_name()
                        .map(|name| name.as_str().to_owned()),
                )
            })
            .collect()
    }

    fn rejected() -> (Type, Option<String>) {
        (Type::Error, Some("org.bluez.Error.Rejected".to_owned()))
    }

    fn canceled() -> (Type, Option<String>) {
        (Type::Error, Some("org.bluez.Error.Canceled".to_owned()))
    }

    #[test]
    fn a_confirmation_is_answered() {
        let (mut pairing, request) = pairing();
        assert!(
            pairing.on_call(&confirmation(DEVICE)).is_empty(),
            "the answer waits for the user"
        );
        assert!(request.get().is_some());

        let answered = pairing.respond(PairingResponse::Confirmation(true));

        assert!(answered.turned_down.is_none());
        assert_eq!(kinds(&answered.reply), vec![(Type::MethodReturn, None)]);
        assert!(request.get().is_none());
    }

    #[test]
    fn rejecting_a_pairing_reports_its_device() {
        let (mut pairing, _request) = pairing();
        let _ = pairing.on_call(&confirmation(DEVICE));

        let answered = pairing.respond(PairingResponse::Confirmation(false));

        assert_eq!(answered.turned_down, Some(path(DEVICE)));
        assert_eq!(kinds(&answered.reply), vec![rejected()]);
    }

    #[test]
    fn denying_a_service_turns_down_no_pairing() {
        let (mut pairing, _request) = pairing();
        let _ = pairing.on_call(
            &call("AuthorizeService")
                .build(&(
                    ObjectPath::from_static_str_unchecked(DEVICE),
                    "0000110b-0000-1000-8000-00805f9b34fb",
                ))
                .unwrap(),
        );

        let answered = pairing.respond(PairingResponse::ServiceAuthorization(false));

        assert!(answered.turned_down.is_none());
        assert_eq!(kinds(&answered.reply), vec![rejected()]);
    }

    #[test]
    fn cancelling_a_display_reports_its_device() {
        let (mut pairing, request) = pairing();
        assert!(
            pairing.on_call(&display_passkey(DEVICE)).is_empty(),
            "BlueZ expects no reply"
        );

        let answered = pairing.respond(PairingResponse::Cancel);

        assert_eq!(answered.turned_down, Some(path(DEVICE)));
        assert!(answered.reply.is_none());
        assert!(request.get().is_none());
    }

    #[test]
    fn a_response_of_the_wrong_kind_leaves_the_request_pending() {
        let (mut pairing, request) = pairing();
        let _ = pairing.on_call(&confirmation(DEVICE));

        let answered = pairing.respond(PairingResponse::Pin("0000".to_owned()));

        assert!(answered.reply.is_none());
        assert!(request.get().is_some());
    }

    #[test]
    fn a_stray_answer_leaves_a_display_alone() {
        let (mut pairing, request) = pairing();
        let _ = pairing.on_call(&display_passkey(DEVICE));

        let _ = pairing.respond(PairingResponse::Passkey(1));

        assert!(request.get().is_some());
    }

    #[test]
    fn bluez_cancelling_ends_the_question_only() {
        let (mut pairing, request) = pairing();
        let _ = pairing.on_call(&display_passkey(OTHER));
        let _ = pairing.on_call(&confirmation(DEVICE));
        assert_eq!(request.get().unwrap().device_path().as_str(), DEVICE);

        let replies = pairing.on_call(&call("Cancel").build(&()).unwrap());

        // The question's call is freed, and the display is shown again.
        assert_eq!(
            kinds(&replies),
            vec![canceled(), (Type::MethodReturn, None)]
        );
        assert_eq!(request.get().unwrap().device_path().as_str(), OTHER);
    }

    #[test]
    fn a_display_does_not_replace_a_question() {
        let (mut pairing, request) = pairing();
        let _ = pairing.on_call(&confirmation(DEVICE));

        let _ = pairing.on_call(&display_passkey(OTHER));

        assert_eq!(request.get().unwrap().device_path().as_str(), DEVICE);
        let _ = pairing.respond(PairingResponse::Confirmation(true));
        assert_eq!(request.get().unwrap().device_path().as_str(), OTHER);
    }

    #[test]
    fn a_display_ends_when_its_device_pairs_or_disconnects() {
        for (paired, disconnected) in [(&[DEVICE][..], &[][..]), (&[][..], &[DEVICE][..])] {
            let (mut pairing, request) = pairing();
            let _ = pairing.on_call(&display_passkey(DEVICE));

            let _ = pairing.devices_changed(&changes(&[OTHER], &[OTHER], &[]));
            assert!(request.get().is_some(), "another device's change");

            let _ = pairing.devices_changed(&changes(paired, disconnected, &[]));
            assert!(request.get().is_none());
        }
    }

    #[test]
    fn a_question_outlives_the_pairing_call() {
        let (mut pairing, request) = pairing();
        let _ = pairing.on_call(&confirmation(DEVICE));

        // BlueZ answers an LE connect before the pairing it leads to.
        pairing.pairing_ended(DEVICE);
        let reply = pairing.devices_changed(&changes(&[DEVICE], &[], &[]));

        assert!(reply.is_none());
        assert!(request.get().is_some());
    }

    #[test]
    fn a_question_ends_when_its_device_disconnects() {
        let (mut pairing, request) = pairing();
        // A PIN request BlueZ gave up on gets no `Cancel`; the failed
        // authentication disconnects the device.
        let _ = pairing.on_call(
            &call("RequestPinCode")
                .build(&(ObjectPath::from_static_str_unchecked(DEVICE),))
                .unwrap(),
        );

        let reply = pairing.devices_changed(&changes(&[], &[OTHER], &[]));
        assert!(reply.is_none(), "another device's change");

        let reply = pairing.devices_changed(&changes(&[], &[DEVICE], &[]));
        assert_eq!(kinds(&reply), vec![canceled()]);
        assert!(request.get().is_none());
    }

    #[test]
    fn a_request_ends_when_its_device_goes_away() {
        let (mut pairing, request) = pairing();
        let _ = pairing.on_call(&confirmation(DEVICE));

        let reply = pairing.devices_changed(&changes(&[], &[], &[DEVICE]));

        assert!(request.get().is_none());
        assert_eq!(kinds(&reply), vec![canceled()]);
    }

    #[test]
    fn a_finished_connect_ends_its_display() {
        let (mut pairing, request) = pairing();
        let _ = pairing.on_call(&display_passkey(DEVICE));

        pairing.pairing_ended(OTHER);
        assert!(request.get().is_some());

        pairing.pairing_ended(DEVICE);
        assert!(request.get().is_none());
    }

    #[test]
    fn unknown_calls_get_an_error() {
        let (mut pairing, request) = pairing();

        let replies = pairing.on_call(
            &Message::method_call(AGENT_PATH, "Introspect")
                .unwrap()
                .interface("org.freedesktop.DBus.Introspectable")
                .unwrap()
                .build(&())
                .unwrap(),
        );

        assert_eq!(
            kinds(&replies),
            vec![(Type::Error, Some(agent::UNKNOWN_METHOD.to_owned()))]
        );
        assert!(request.get().is_none());
    }
}
