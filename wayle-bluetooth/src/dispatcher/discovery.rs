//! This service's device discovery, per adapter.
//!
//! BlueZ keeps one discovery session per client and adapter. It turns down a
//! start or stop while the client's previous one is still being carried out,
//! and answers a start that fails with the same `InProgress` it answers
//! "already discovering" with. So the dispatcher keeps, per adapter, what this
//! service wants and whether its session is running (as BlueZ's replies
//! report), and sends one request at a time only when the two differ: a start
//! is never redundant, and its `InProgress` means it failed. Any reply to a
//! stop means the session is over: BlueZ removes it even when the controller
//! refuses to stop (answering `InProgress`).
//!
//! BlueZ ends every session of an adapter that powers off, without answering
//! all the requests it drops; the dispatcher then [forgets](Discovery::forget)
//! the adapter's session.
//!
//! Two BlueZ bugs are not worked around, as nothing BlueZ publishes tells
//! them apart from a working session:
//! - Another client's start can take the reply to this service's start, which
//!   then never comes. BlueZ would turn down a stop until the adapter powers
//!   off, so nothing more is sent for that adapter until then.
//! - A start while another client's stop is under way is accepted at once,
//!   and BlueZ then stops discovering anyway. As far as BlueZ's answers go
//!   the session is running, so asking to discover sends nothing until this
//!   service stops the session (e.g. a timed discovery's deadline passes).

use std::{collections::HashMap, time::Duration};

use tokio::time::Instant;

use crate::error::Error;

/// Timeouts this long or longer mean no timeout: far enough never to matter,
/// and well short of the deadlines the timer can't represent.
const LONGEST_TIMEOUT: Duration = Duration::from_secs(60 * 60 * 24 * 365 * 30);

/// The deadline of a discovery started now for `timeout`, if it has one.
pub(crate) fn deadline(timeout: Duration) -> Option<Instant> {
    (timeout < LONGEST_TIMEOUT).then(|| Instant::now() + timeout)
}

#[derive(Debug, Default)]
struct Session {
    /// Whether this service's latest request was to discover.
    wanted: bool,
    /// Whether this service's session is running, as BlueZ's replies report.
    running: bool,
    /// The start (`true`) or stop (`false`) awaiting BlueZ's reply.
    in_flight: Option<bool>,
    /// When a timed discovery ends.
    deadline: Option<Instant>,
}

impl Session {
    /// The request to send now: the wanted state, unless it holds already or
    /// a request is awaiting its reply.
    fn next(&mut self) -> Option<bool> {
        if self.in_flight.is_some() || self.wanted == self.running {
            return None;
        }
        self.in_flight = Some(self.wanted);
        self.in_flight
    }

    fn idle(&self) -> bool {
        !self.wanted && !self.running && self.in_flight.is_none()
    }
}

/// Sessions by adapter path.
#[derive(Debug, Default)]
pub(crate) struct Discovery(HashMap<String, Session>);

impl Discovery {
    /// Asks to start (with an optional `deadline`) or stop discovering on
    /// `adapter`. Returns the request to send now, if any.
    pub(crate) fn request(
        &mut self,
        adapter: &str,
        start: bool,
        deadline: Option<Instant>,
    ) -> Option<bool> {
        let session = self.0.entry(adapter.to_owned()).or_default();
        session.wanted = start;
        session.deadline = deadline.filter(|_| start);

        let next = session.next();
        self.prune(adapter);
        next
    }

    /// Applies BlueZ's reply to the request in flight for `adapter`. Returns
    /// the request to send next, if the wanted state changed meanwhile.
    pub(crate) fn answered(&mut self, adapter: &str, result: &Result<(), Error>) -> Option<bool> {
        let session = self.0.get_mut(adapter)?;
        let start = session.in_flight.take()?;

        match (start, result) {
            (true, Ok(())) => session.running = true,
            // Never redundant, so this failed. Not retried: the failure is
            // recorded, and asking again is up to the caller.
            (true, Err(_)) => {
                session.running = false;
                session.wanted = false;
                session.deadline = None;
            }
            // Stopped, refused by the controller (BlueZ ends the session
            // anyway), or nothing was running to stop.
            (false, _) => session.running = false,
        }

        let next = session.next();
        self.prune(adapter);
        next
    }

    /// The adapters this service wants discovering.
    pub(crate) fn wanted(&self) -> Vec<String> {
        self.0
            .iter()
            .filter(|(_, session)| session.wanted)
            .map(|(adapter, _)| adapter.clone())
            .collect()
    }

    /// The earliest deadline of a timed discovery.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.0.values().filter_map(|session| session.deadline).min()
    }

    /// The adapters whose timed discovery has ended by `now`.
    pub(crate) fn expired(&self, now: Instant) -> Vec<String> {
        self.0
            .iter()
            .filter(|(_, session)| session.deadline.is_some_and(|deadline| deadline <= now))
            .map(|(adapter, _)| adapter.clone())
            .collect()
    }

    /// Forgets `adapter`'s session: BlueZ ended it (the adapter powered off
    /// or went away).
    pub(crate) fn forget(&mut self, adapter: &str) {
        self.0.remove(adapter);
    }

    /// Forgets everything: the bluetoothd that had the sessions is gone.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    fn prune(&mut self, adapter: &str) {
        if self.0.get(adapter).is_some_and(Session::idle) {
            self.0.remove(adapter);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::BLUEZ_IN_PROGRESS;

    const HCI0: &str = "/org/bluez/hci0";

    fn bluez_error(name: &'static str) -> Result<(), Error> {
        let message = zbus::Message::method_call("/", "StartDiscovery")
            .unwrap()
            .build(&())
            .unwrap();
        Err(Error::Dbus(zbus::Error::MethodError(
            zbus::names::ErrorName::from_static_str(name)
                .unwrap()
                .into(),
            None,
            message,
        )))
    }

    #[test]
    fn a_stop_waits_for_the_start_in_flight() {
        let mut discovery = Discovery::default();

        assert_eq!(discovery.request(HCI0, true, None), Some(true));
        assert_eq!(discovery.request(HCI0, false, None), None);

        assert_eq!(discovery.answered(HCI0, &Ok(())), Some(false));
        assert_eq!(discovery.answered(HCI0, &Ok(())), None);
        assert!(discovery.wanted().is_empty());
    }

    #[test]
    fn a_running_session_is_not_started_again() {
        let mut discovery = Discovery::default();
        discovery.request(HCI0, true, None);
        discovery.answered(HCI0, &Ok(()));

        // Extending a scan only moves its deadline.
        let deadline = deadline(Duration::from_secs(30));
        assert_eq!(discovery.request(HCI0, true, deadline), None);
        assert_eq!(discovery.next_deadline(), deadline);
    }

    #[test]
    fn a_failed_start_is_given_up() {
        let mut discovery = Discovery::default();
        discovery.request(HCI0, true, deadline(Duration::from_secs(30)));

        // BlueZ answers a failed start with InProgress.
        assert_eq!(
            discovery.answered(HCI0, &bluez_error(BLUEZ_IN_PROGRESS)),
            None
        );

        assert!(discovery.wanted().is_empty());
        assert_eq!(discovery.next_deadline(), None);
        // Asking again sends a start again.
        assert_eq!(discovery.request(HCI0, true, None), Some(true));
    }

    #[test]
    fn a_failed_stop_ends_the_session() {
        // The controller refused to stop (BlueZ removes the session anyway),
        // or nothing was running to stop.
        for error in [BLUEZ_IN_PROGRESS, "org.bluez.Error.Failed"] {
            let mut discovery = Discovery::default();
            discovery.request(HCI0, true, None);
            discovery.answered(HCI0, &Ok(()));
            discovery.request(HCI0, false, None);

            assert_eq!(discovery.answered(HCI0, &bluez_error(error)), None);

            assert_eq!(discovery.request(HCI0, true, None), Some(true));
        }
    }

    #[test]
    fn a_timed_discovery_expires() {
        let mut discovery = Discovery::default();
        let deadline = deadline(Duration::from_secs(30)).unwrap();
        discovery.request(HCI0, true, Some(deadline));
        discovery.answered(HCI0, &Ok(()));

        assert_eq!(discovery.next_deadline(), Some(deadline));
        assert!(discovery.expired(Instant::now()).is_empty());
        assert_eq!(discovery.expired(deadline), vec![HCI0.to_owned()]);

        // Stopping clears the deadline.
        discovery.request(HCI0, false, None);
        assert_eq!(discovery.next_deadline(), None);
    }

    #[test]
    fn an_open_ended_start_clears_the_deadline() {
        let mut discovery = Discovery::default();
        discovery.request(HCI0, true, deadline(Duration::from_secs(30)));
        discovery.answered(HCI0, &Ok(()));

        discovery.request(HCI0, true, None);

        assert_eq!(discovery.next_deadline(), None);
    }

    #[test]
    fn a_forgotten_session_starts_afresh() {
        let mut discovery = Discovery::default();
        // A start BlueZ dropped when the adapter powered off.
        discovery.request(HCI0, true, None);

        discovery.forget(HCI0);

        assert_eq!(discovery.answered(HCI0, &Ok(())), None);
        assert_eq!(discovery.request(HCI0, true, None), Some(true));
    }

    #[test]
    fn huge_timeouts_mean_no_timeout() {
        assert!(deadline(Duration::MAX).is_none());
        assert!(deadline(Duration::from_secs(60)).is_some());
    }
}
