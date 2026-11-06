//! IWD's passphrase requests to this service's agent, and the request they
//! publish.
//!
//! IWD asks the agent for a network's passphrase when it connects to a secured
//! network it has none saved for, and waits for the answer. That wait is the
//! published request: it ends when the agent answers or turns it down, or when
//! IWD cancels it (it stopped waiting, or the network went away).
//!
//! IWD reports a rejected passphrase only as the failure of the connect
//! (`Failed`, or `InvalidFormat` for one it can't use), and then asks for the
//! passphrase again on the next connect to that network. So when a connect this
//! service sent fails right after the agent answered for its network, the
//! dispatcher connects again, and IWD's next request is published as
//! [`rejected`](PassphraseRequest::rejected). Each round needs a new answer,
//! so this ends when one is accepted or the request is turned down.

use std::sync::Arc;

use tracing::{debug, warn};
use wayle_core::Property;
use zbus::{Message, zvariant::OwnedObjectPath};

use crate::{
    agent::{self, Answer},
    error::{Error, IWD_FAILED, IWD_INVALID_FORMAT},
    network::Network,
    types::PassphraseRequest,
};

pub(crate) struct Passphrase {
    published: Property<Option<PassphraseRequest>>,
    /// IWD's call awaiting an answer, and the request it asks.
    asking: Option<(Message, PassphraseRequest)>,
    /// The network the agent last gave a passphrase for, for a connect this
    /// service sent, until that connect finishes.
    answered: Option<OwnedObjectPath>,
    /// The network whose next request follows a rejected passphrase.
    rejected: Option<OwnedObjectPath>,
}

impl Passphrase {
    pub(crate) fn new(published: &Property<Option<PassphraseRequest>>) -> Self {
        Self {
            published: published.clone(),
            asking: None,
            answered: None,
            rejected: None,
        }
    }

    /// IWD asks for `network`'s passphrase with `call`. Returns the reply
    /// freeing an earlier call this one replaces: IWD asks one at a time.
    #[must_use]
    pub(crate) fn ask(&mut self, call: Message, network: Arc<Network>) -> Option<Message> {
        let replaced = self
            .asking
            .take()
            .and_then(|(previous, _)| agent::reply(&previous, Answer::Canceled));
        let rejected = self
            .rejected
            .take_if(|path| path == network.object_path())
            .is_some();

        self.asking = Some((call, PassphraseRequest { network, rejected }));
        self.publish();
        replaced
    }

    /// IWD cancelled the request. Returns the reply freeing its call.
    #[must_use]
    pub(crate) fn cancelled(&mut self) -> Option<Message> {
        debug!("iwd cancelled its passphrase request");
        let (call, _) = self.asking.take()?;
        self.publish();
        agent::reply(&call, Answer::Canceled)
    }

    /// IWD no longer uses the agent.
    pub(crate) fn released(&mut self) {
        self.asking = None;
        self.publish();
    }

    /// The network IWD is asking a passphrase for.
    pub(crate) fn asking_for(&self) -> Option<&Arc<Network>> {
        self.asking.as_ref().map(|(_, request)| &request.network)
    }

    /// Answers the request with `passphrase`. `ours`: the request is for a
    /// connect this service sent, which `connect_finished` will see end (IWD
    /// also asks this agent for other clients' connects, if they have none).
    /// Returns the reply to IWD's call.
    #[must_use]
    pub(crate) fn provide(&mut self, passphrase: String, ours: bool) -> Option<Message> {
        let Some((call, request)) = self.asking.take() else {
            warn!("passphrase ignored: iwd isn't asking for one");
            return None;
        };

        self.answered = ours.then(|| request.network.object_path().clone());
        self.publish();
        agent::reply(&call, Answer::Passphrase(passphrase))
    }

    /// Turns the request down. IWD then ends its connect as aborted. Returns
    /// the reply to IWD's call.
    #[must_use]
    pub(crate) fn cancel(&mut self) -> Option<Message> {
        let Some((call, _)) = self.asking.take() else {
            warn!("passphrase cancel ignored: iwd isn't asking for one");
            return None;
        };

        self.publish();
        agent::reply(&call, Answer::Canceled)
    }

    /// A connect this service sent for `network` finished with `result`.
    /// Returns whether to connect again: it failed right after the agent gave
    /// a passphrase for it, so IWD rejected the passphrase and will ask again.
    pub(crate) fn connect_finished(
        &mut self,
        network: &OwnedObjectPath,
        result: &Result<Message, Error>,
    ) -> bool {
        let answered = self.answered.take_if(|path| path == network).is_some();
        let rejected = answered
            && result.as_ref().is_err_and(|err| {
                err.is_iwd_error(IWD_FAILED) || err.is_iwd_error(IWD_INVALID_FORMAT)
            });

        if rejected {
            self.rejected = Some(network.clone());
        } else {
            // A connect for it ended without asking again.
            self.rejected.take_if(|path| path == network);
        }
        rejected
    }

    /// `network` went away. Returns the reply freeing IWD's call, if it was
    /// asking for that network.
    #[must_use]
    pub(crate) fn network_removed(&mut self, network: &OwnedObjectPath) -> Option<Message> {
        let asking_for = self
            .asking
            .as_ref()
            .is_some_and(|(_, request)| request.network.object_path() == network);
        if !asking_for {
            return None;
        }
        self.cancelled()
    }

    /// Forgets everything: the IWD instance that asked is gone.
    pub(crate) fn clear(&mut self) {
        self.asking = None;
        self.answered = None;
        self.rejected = None;
        self.publish();
    }

    fn publish(&self) {
        self.published
            .set(self.asking.as_ref().map(|(_, request)| request.clone()));
    }
}

#[cfg(test)]
mod tests {
    use zbus::{message::Type, zvariant::ObjectPath};

    use super::*;
    use crate::{agent::AGENT_PATH, dispatcher::command::Commands};

    const NETWORK: &str = "/net/connman/iwd/0/4/6d79776966695f70736b";
    const OTHER: &str = "/net/connman/iwd/0/4/6f74686572_psk";

    fn network(at: &str) -> Arc<Network> {
        let (commands, _) = Commands::channel();
        Arc::new(Network::new(
            &commands,
            OwnedObjectPath::try_from(at).unwrap(),
        ))
    }

    fn request(at: &'static str) -> Message {
        Message::method_call(AGENT_PATH, "RequestPassphrase")
            .unwrap()
            .interface("net.connman.iwd.Agent")
            .unwrap()
            .build(&(ObjectPath::from_static_str_unchecked(at),))
            .unwrap()
    }

    fn connect_error(name: &'static str) -> Result<Message, Error> {
        let call = Message::method_call(NETWORK, "Connect")
            .unwrap()
            .build(&())
            .unwrap();
        Err(Error::Dbus(zbus::Error::MethodError(
            zbus::names::ErrorName::from_static_str(name)
                .unwrap()
                .into(),
            None,
            call,
        )))
    }

    fn passphrase() -> (Passphrase, Property<Option<PassphraseRequest>>) {
        let published = Property::new(None);
        (Passphrase::new(&published), published)
    }

    #[test]
    fn a_request_is_published_until_answered() {
        let (mut passphrase, published) = passphrase();
        let wifi = network(NETWORK);

        assert!(
            passphrase
                .ask(request(NETWORK), Arc::clone(&wifi))
                .is_none()
        );
        let shown = published.get().unwrap();
        assert!(Arc::ptr_eq(&shown.network, &wifi));
        assert!(!shown.rejected);

        let reply = passphrase.provide("hunter22".to_owned(), true).unwrap();
        assert_eq!(reply.message_type(), Type::MethodReturn);
        assert!(published.get().is_none());
    }

    #[test]
    fn a_rejected_passphrase_asks_again() {
        let (mut passphrase, published) = passphrase();
        let wifi = network(NETWORK);
        let path = wifi.object_path().clone();
        let _ = passphrase.ask(request(NETWORK), Arc::clone(&wifi));
        let _ = passphrase.provide("wrong".to_owned(), true);

        assert!(passphrase.connect_finished(&path, &connect_error(IWD_FAILED)));

        // IWD asks again on the next connect.
        let _ = passphrase.ask(request(NETWORK), wifi);
        assert!(published.get().unwrap().rejected);
    }

    #[test]
    fn answering_another_clients_request_doesnt_ask_again_later() {
        let (mut passphrase, _) = passphrase();
        let network = network(NETWORK);
        let _ = passphrase.ask(request(NETWORK), Arc::clone(&network));

        // IWD asked for a connect some other client sent.
        let _ = passphrase.provide("hunter22".to_owned(), false);

        // A later connect of ours to the network fails for another reason.
        assert!(!passphrase.connect_finished(network.object_path(), &connect_error(IWD_FAILED)));
    }

    #[test]
    fn a_failure_without_an_answer_doesnt_ask_again() {
        let (mut passphrase, _published) = passphrase();
        let path = OwnedObjectPath::try_from(NETWORK).unwrap();

        // E.g. a saved passphrase IWD rejected: the agent gave none.
        assert!(!passphrase.connect_finished(&path, &connect_error(IWD_FAILED)));
    }

    #[test]
    fn other_failures_dont_ask_again() {
        let (mut passphrase, _published) = passphrase();
        let wifi = network(NETWORK);
        let path = wifi.object_path().clone();
        let _ = passphrase.ask(request(NETWORK), wifi);
        let _ = passphrase.provide("hunter22".to_owned(), true);

        assert!(!passphrase.connect_finished(&path, &connect_error("net.connman.iwd.Timeout")));
    }

    #[test]
    fn a_retry_that_ends_without_asking_forgets_the_rejection() {
        let (mut passphrase, published) = passphrase();
        let wifi = network(NETWORK);
        let path = wifi.object_path().clone();
        let _ = passphrase.ask(request(NETWORK), Arc::clone(&wifi));
        let _ = passphrase.provide("wrong".to_owned(), true);
        assert!(passphrase.connect_finished(&path, &connect_error(IWD_FAILED)));

        // The retry fails before IWD asks (e.g. the network went out of range).
        assert!(!passphrase.connect_finished(&path, &connect_error("net.connman.iwd.NotFound")));

        let _ = passphrase.ask(request(NETWORK), wifi);
        assert!(!published.get().unwrap().rejected);
    }

    #[test]
    fn turning_a_request_down_cancels_it() {
        let (mut passphrase, published) = passphrase();
        let _ = passphrase.ask(request(NETWORK), network(NETWORK));

        let reply = passphrase.cancel().unwrap();

        assert_eq!(
            reply.header().error_name().unwrap().as_str(),
            "net.connman.iwd.Agent.Error.Canceled"
        );
        assert!(published.get().is_none());
    }

    #[test]
    fn a_new_request_replaces_the_previous_one() {
        let (mut passphrase, published) = passphrase();
        let _ = passphrase.ask(request(NETWORK), network(NETWORK));

        let freed = passphrase.ask(request(OTHER), network(OTHER));

        assert!(freed.is_some(), "the previous call is answered");
        assert_eq!(
            published.get().unwrap().network.object_path().as_str(),
            OTHER
        );
    }

    #[test]
    fn a_request_ends_with_its_network() {
        let (mut passphrase, published) = passphrase();
        let _ = passphrase.ask(request(NETWORK), network(NETWORK));

        assert!(
            passphrase
                .network_removed(&OwnedObjectPath::try_from(OTHER).unwrap())
                .is_none()
        );
        assert!(published.get().is_some());

        assert!(
            passphrase
                .network_removed(&OwnedObjectPath::try_from(NETWORK).unwrap())
                .is_some()
        );
        assert!(published.get().is_none());
    }
}
