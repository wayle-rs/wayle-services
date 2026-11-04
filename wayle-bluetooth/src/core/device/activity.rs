//! The request bookkeeping behind [`DeviceInfo::activity`] and
//! [`DeviceInfo::last_error`].
//!
//! BlueZ publishes the outcome of a request as state (`Connected`, `Paired`,
//! the device disappearing) but never that one is under way, nor that one
//! failed. So this is the only device state not taken from BlueZ's
//! properties, and it follows BlueZ wherever it can:
//!
//! - An activity starts when its request is sent. A request of the kind
//!   already under way joins it instead (BlueZ turns the duplicate down as
//!   `InProgress`).
//! - It ends when BlueZ reports the outcome ([`Tracker::settle`]): the target
//!   state, or (once BlueZ accepted the request) any report of the property
//!   the request acts on, since BlueZ may merge the outcome with later changes
//!   (connected, then dropped, reported once as `Connected=false`).
//! - A reply ends it only where BlueZ publishes nothing: a failure (recorded
//!   in `last_error`), a cancellation, or a success whose outcome already
//!   holds. A success also clears an earlier failure of the same action.
//!
//! A newer request supersedes the activity under way (e.g. a disconnect
//! cancelling a connect), and the superseded request's failure is not
//! reported: its actual outcome still arrives as state. If the newer request
//! doesn't happen after all (BlueZ turns it down as busy, or it fails), the
//! activity falls back to the superseded one, if that is still under way.
//!
//! So every activity shown has an end in sight: the reply to its request, or
//! BlueZ's report of its outcome. The dispatcher owns every [`Tracker`] and
//! drives it from one task, in the order events happened, so there is nothing
//! to lock.

use tracing::{debug, warn};

use super::{DeviceInfo, reached};
use crate::{
    error::{BLUEZ_IN_PROGRESS, Error},
    types::device::{DeviceAction, DeviceActivity, DeviceError},
};

/// BlueZ's error for a pairing that was cancelled (by `CancelPairing`, or the
/// link dropping mid-pairing): its report of the outcome, not a failure.
const BLUEZ_AUTHENTICATION_CANCELED: &str = "org.bluez.Error.AuthenticationCanceled";

/// BlueZ's error for `CancelPairing` with no pairing in progress: there was
/// nothing to cancel.
const BLUEZ_DOES_NOT_EXIST: &str = "org.bluez.Error.DoesNotExist";

/// BlueZ's error for `Pair` on a device that is already paired: there was
/// nothing to do.
const BLUEZ_ALREADY_EXISTS: &str = "org.bluez.Error.AlreadyExists";

/// Request bookkeeping for one device. See the [module docs](self).
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    /// Allocates request ids; never reused.
    next_id: u64,
    /// The requests under way, oldest first. The newest is the activity
    /// shown; the others are those it superseded, kept to fall back to.
    requests: Vec<Request>,
    /// The request whose failure is reported: the newest, even once BlueZ's
    /// outcome ended its activity.
    latest: Option<u64>,
}

/// A request under way.
#[derive(Debug, Clone, Copy)]
struct Request {
    id: u64,
    activity: DeviceActivity,
    /// BlueZ accepted the request, but its outcome isn't visible yet.
    replied: bool,
}

/// Identifies a request, so its reply is applied to the right activity.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Ticket {
    /// A request that sets no activity (e.g. `SetAlias`).
    Untracked,
    /// A request that started an activity, and the request that was latest
    /// before it (latest again if this one turns out not to happen).
    Started { id: u64, previous: Option<u64> },
    /// A duplicate of the request under way, which it joins.
    Joined(u64),
}

/// Which properties a `Device1` update reported, whatever their values.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Reported {
    pub connected: bool,
    pub paired: bool,
}

/// How a failed request is interpreted.
enum Failure {
    /// BlueZ is busy: already doing this (for us, or for someone else), or
    /// something it can't do alongside.
    InProgress,
    /// BlueZ reports the request cancelled, or there was nothing to do: an
    /// outcome, not a failure.
    Cancelled,
    /// The request failed.
    Failed,
}

impl Tracker {
    /// The activity shown: the newest request's.
    pub(crate) fn activity(&self) -> DeviceActivity {
        self.requests
            .last()
            .map_or(DeviceActivity::Idle, |request| request.activity)
    }

    /// Starts `activity` for a request about to be sent (or joins the request
    /// under way for it), returning the request's [`Ticket`].
    pub(crate) fn begin(
        &mut self,
        info: &mut DeviceInfo,
        activity: Option<DeviceActivity>,
    ) -> Ticket {
        let Some(activity) = activity else {
            return Ticket::Untracked;
        };

        if let Some(newest) = self.requests.last()
            && newest.activity == activity
        {
            return Ticket::Joined(newest.id);
        }

        self.next_id += 1;
        let id = self.next_id;
        self.requests.push(Request {
            id,
            activity,
            replied: false,
        });
        let previous = self.latest.replace(id);
        info.last_error = None;

        Ticket::Started { id, previous }
    }

    /// Applies the reply to a request.
    pub(crate) fn finish(
        &mut self,
        info: &mut DeviceInfo,
        action: DeviceAction,
        ticket: Ticket,
        result: Result<(), Error>,
    ) {
        let id = match ticket {
            Ticket::Untracked => None,
            Ticket::Started { id, .. } | Ticket::Joined(id) => Some(id),
        };
        // The request, if still under way, and whether a newer request has
        // superseded it.
        let position = id.and_then(|id| self.requests.iter().position(|request| request.id == id));
        let superseded = id.is_some_and(|id| match position {
            Some(position) => position + 1 != self.requests.len(),
            None => self.latest != Some(id),
        });

        let err = match result {
            Ok(()) => {
                // A success replaces an earlier failure of the same action.
                info.last_error.take_if(|error| error.action == action);
                if let Some(position) = position {
                    let request = &mut self.requests[position];
                    request.replied = true;
                    if !superseded && reached(info, request.activity) {
                        self.requests.clear();
                    }
                }
                return;
            }
            Err(err) => err,
        };

        match (classify(action, &err), ticket) {
            // A duplicate is expected to be turned down: the request it
            // joined carries on.
            (Failure::InProgress, Ticket::Joined(_)) => {}
            // BlueZ is busy with someone else's request of the same kind:
            // this one didn't happen, and that one's outcome shows as state.
            // The request before it is the latest again (and its failure
            // still counts).
            (Failure::InProgress, Ticket::Started { id, previous }) => {
                self.remove(info, position);
                if self.latest == Some(id) {
                    self.latest = previous;
                }
            }
            (Failure::Cancelled, _) => {
                debug!(?action, error = %err, "bluetooth request cancelled");
                self.remove(info, position);
            }
            (Failure::Failed, _) if superseded => {
                debug!(?action, error = %err, "superseded bluetooth request failed");
                self.remove(info, position);
            }
            // Including an untracked request BlueZ turned down as busy:
            // nothing else shows that it didn't happen.
            (Failure::Failed, _) | (Failure::InProgress, Ticket::Untracked) => {
                warn!(?action, error = %err, "bluetooth device action failed");
                info.last_error = Some(DeviceError::new(action, err));
                self.remove(info, position);
            }
        }
    }

    /// Ends the current activity if a `Device1` update reports its outcome.
    pub(crate) fn settle(&mut self, info: &DeviceInfo, reported: Reported) {
        let Some(newest) = self.requests.last() else {
            return;
        };

        if outcome_arrived(info, reported, newest) {
            // Along with the requests it superseded.
            self.requests.clear();
        } else {
            // Nothing can fall back to a superseded request that is over.
            self.requests
                .retain(|request| !outcome_arrived(info, reported, request));
        }
    }

    /// Drops a request that didn't happen or failed. If it was the activity
    /// shown, falls back to the newest request it superseded, if any.
    fn remove(&mut self, info: &DeviceInfo, position: Option<usize>) {
        let Some(position) = position else {
            return;
        };
        let newest = position + 1 == self.requests.len();
        self.requests.remove(position);

        if newest && let Some(fallback) = self.requests.last() {
            self.latest = Some(fallback.id);
            // Its outcome may have arrived while it was superseded.
            if reached(info, fallback.activity) {
                self.requests.clear();
            }
        }
    }
}

/// Whether BlueZ reports the outcome of `request`: its target state, or (once
/// BlueZ accepted it) any report of the property it acts on.
fn outcome_arrived(info: &DeviceInfo, reported: Reported, request: &Request) -> bool {
    let property_reported = match request.activity {
        DeviceActivity::Connecting | DeviceActivity::Disconnecting => reported.connected,
        DeviceActivity::Pairing => reported.paired,
        // A forgotten device is removed, activity and all.
        DeviceActivity::Idle | DeviceActivity::Forgetting => false,
    };

    reached(info, request.activity) || (request.replied && property_reported)
}

fn classify(action: DeviceAction, err: &Error) -> Failure {
    let Some((name, _)) = err.bluez_error() else {
        return Failure::Failed;
    };

    match (action, name) {
        (_, BLUEZ_IN_PROGRESS) => Failure::InProgress,
        (_, BLUEZ_AUTHENTICATION_CANCELED)
        | (DeviceAction::CancelPairing, BLUEZ_DOES_NOT_EXIST)
        | (DeviceAction::Pair, BLUEZ_ALREADY_EXISTS) => Failure::Cancelled,
        _ => Failure::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bluez_error(name: &'static str) -> Error {
        let message = zbus::Message::method_call("/", "Connect")
            .unwrap()
            .build(&())
            .unwrap();
        Error::Dbus(zbus::Error::MethodError(
            zbus::names::ErrorName::from_static_str(name)
                .unwrap()
                .into(),
            Some("detail".to_owned()),
            message,
        ))
    }

    fn failed() -> Result<(), Error> {
        Err(bluez_error("org.bluez.Error.Failed"))
    }

    /// A device and its tracker, driven the way the dispatcher drives them.
    #[derive(Default)]
    struct Harness {
        info: DeviceInfo,
        tracker: Tracker,
    }

    impl Default for DeviceInfo {
        fn default() -> Self {
            Self::empty()
        }
    }

    impl Harness {
        fn begin(&mut self, activity: DeviceActivity) -> Ticket {
            let ticket = self.tracker.begin(&mut self.info, Some(activity));
            self.info.activity = self.tracker.activity();
            ticket
        }

        fn finish(&mut self, action: DeviceAction, ticket: Ticket, result: Result<(), Error>) {
            self.tracker.finish(&mut self.info, action, ticket, result);
            self.info.activity = self.tracker.activity();
        }

        fn connect(&mut self, result: Result<(), Error>) {
            let ticket = self.begin(DeviceActivity::Connecting);
            self.finish(DeviceAction::Connect, ticket, result);
        }

        /// BlueZ reports `Connected`.
        fn report_connected(&mut self, connected: bool) {
            self.info.connected = connected;
            let reported = Reported {
                connected: true,
                paired: false,
            };
            self.tracker.settle(&self.info, reported);
            self.info.activity = self.tracker.activity();
        }
    }

    #[test]
    fn activity_waits_for_the_bluez_outcome() {
        let mut device = Harness::default();

        device.connect(Ok(()));
        // BlueZ replied, but hasn't signalled `Connected` yet.
        assert_eq!(device.info.activity, DeviceActivity::Connecting);

        device.report_connected(true);
        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn any_outcome_report_after_the_reply_ends_the_activity() {
        let mut device = Harness::default();

        device.connect(Ok(()));
        // Connected, then dropped, merged by BlueZ into one report.
        device.report_connected(false);
        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn reports_before_the_reply_do_not_end_the_activity() {
        let mut device = Harness::default();

        device.begin(DeviceActivity::Connecting);
        device.report_connected(false);
        assert_eq!(device.info.activity, DeviceActivity::Connecting);
    }

    #[test]
    fn activity_ends_on_success_when_outcome_already_holds() {
        let mut device = Harness::default();
        device.info.connected = true;

        device.connect(Ok(()));
        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn failure_ends_the_activity_and_is_recorded() {
        let mut device = Harness::default();

        device.connect(failed());

        assert_eq!(device.info.activity, DeviceActivity::Idle);
        let error = device.info.last_error.as_ref().unwrap();
        assert_eq!(error.action, DeviceAction::Connect);
        assert_eq!(error.name(), Some("org.bluez.Error.Failed"));
        assert_eq!(error.message(), Some("detail"));
    }

    #[test]
    fn a_new_request_clears_the_error() {
        let mut device = Harness::default();
        device.connect(failed());

        device.begin(DeviceActivity::Connecting);
        assert!(device.info.last_error.is_none());
    }

    #[test]
    fn a_failure_after_the_outcome_ended_the_activity_is_recorded() {
        let mut device = Harness::default();

        let ticket = device.begin(DeviceActivity::Connecting);
        // The link came up (ending "Connecting")...
        device.report_connected(true);
        // ...but no profile connected, so BlueZ failed the request.
        device.finish(DeviceAction::Connect, ticket, failed());

        assert_eq!(
            device.info.last_error.as_ref().unwrap().action,
            DeviceAction::Connect
        );
    }

    #[test]
    fn cancelling_a_connect_records_no_error() {
        let mut device = Harness::default();

        let connect = device.begin(DeviceActivity::Connecting);
        let disconnect = device.begin(DeviceActivity::Disconnecting);
        // BlueZ fails the connect the disconnect cancelled...
        device.finish(DeviceAction::Connect, connect, failed());
        // ...and the disconnect succeeds (nothing was connected).
        device.finish(DeviceAction::Disconnect, disconnect, Ok(()));

        assert!(device.info.last_error.is_none());
        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn a_cancelled_pairing_records_no_error() {
        let mut device = Harness::default();

        let pair = device.begin(DeviceActivity::Pairing);
        let cancel = device.tracker.begin(&mut device.info, None);
        device.finish(DeviceAction::CancelPairing, cancel, Ok(()));
        device.finish(
            DeviceAction::Pair,
            pair,
            Err(bluez_error(BLUEZ_AUTHENTICATION_CANCELED)),
        );

        assert!(device.info.last_error.is_none());
        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn cancelling_with_nothing_to_cancel_records_no_error() {
        let mut device = Harness::default();

        let cancel = device.tracker.begin(&mut device.info, None);
        device.finish(
            DeviceAction::CancelPairing,
            cancel,
            Err(bluez_error(BLUEZ_DOES_NOT_EXIST)),
        );

        assert!(device.info.last_error.is_none());
    }

    #[test]
    fn a_duplicate_joins_the_request_under_way() {
        let mut device = Harness::default();

        // The first connect was accepted and awaits its outcome.
        device.connect(Ok(()));
        // A duplicate is turned down as already in progress.
        device.connect(Err(bluez_error(BLUEZ_IN_PROGRESS)));
        assert_eq!(device.info.activity, DeviceActivity::Connecting);
        assert!(device.info.last_error.is_none());

        // The first request's end condition survives the duplicate.
        device.report_connected(false);
        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn a_duplicate_accepted_before_the_rejection_keeps_the_end_condition() {
        let mut device = Harness::default();

        let first = device.begin(DeviceActivity::Connecting);
        let duplicate = device.begin(DeviceActivity::Connecting);
        device.finish(DeviceAction::Connect, first, Ok(()));
        device.finish(
            DeviceAction::Connect,
            duplicate,
            Err(bluez_error(BLUEZ_IN_PROGRESS)),
        );

        device.report_connected(false);
        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn in_progress_for_someone_else_ends_the_activity() {
        let mut device = Harness::default();

        // BlueZ is busy with another client's connect, not ours.
        device.connect(Err(bluez_error(BLUEZ_IN_PROGRESS)));
        assert_eq!(device.info.activity, DeviceActivity::Idle);
        assert!(device.info.last_error.is_none());
    }

    #[test]
    fn a_request_that_does_not_happen_falls_back_to_the_one_it_superseded() {
        let mut device = Harness::default();

        let pair = device.begin(DeviceActivity::Pairing);
        let connect = device.begin(DeviceActivity::Connecting);
        device.finish(DeviceAction::Connect, connect, failed());

        // The pairing is still under way, and its reply still counts.
        assert_eq!(device.info.activity, DeviceActivity::Pairing);
        assert_eq!(
            device.info.last_error.as_ref().unwrap().action,
            DeviceAction::Connect
        );
        device.finish(DeviceAction::Pair, pair, failed());
        assert_eq!(device.info.activity, DeviceActivity::Idle);
        assert_eq!(
            device.info.last_error.as_ref().unwrap().action,
            DeviceAction::Pair
        );
    }

    #[test]
    fn a_superseded_request_that_failed_meanwhile_is_not_fallen_back_to() {
        let mut device = Harness::default();

        let connect = device.begin(DeviceActivity::Connecting);
        let disconnect = device.begin(DeviceActivity::Disconnecting);
        device.finish(DeviceAction::Connect, connect, failed());
        device.finish(
            DeviceAction::Disconnect,
            disconnect,
            Err(bluez_error(BLUEZ_IN_PROGRESS)),
        );

        assert_eq!(device.info.activity, DeviceActivity::Idle);
        assert!(device.info.last_error.is_none());
    }

    #[test]
    fn a_superseded_request_whose_outcome_arrived_is_not_fallen_back_to() {
        let mut device = Harness::default();

        device.connect(Ok(()));
        let pair = device.begin(DeviceActivity::Pairing);
        // The connect's outcome: up and dropped again, reported once.
        device.report_connected(false);
        assert_eq!(device.info.activity, DeviceActivity::Pairing);
        device.finish(DeviceAction::Pair, pair, failed());

        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn a_later_success_of_the_same_action_clears_its_failure() {
        let mut device = Harness::default();
        let alias = device.tracker.begin(&mut device.info, None);
        device.finish(DeviceAction::SetAlias, alias, failed());

        // Another action's success leaves it...
        let trusted = device.tracker.begin(&mut device.info, None);
        device.finish(DeviceAction::SetTrusted, trusted, Ok(()));
        assert!(device.info.last_error.is_some());

        // ...a retry's success doesn't.
        let retry = device.tracker.begin(&mut device.info, None);
        device.finish(DeviceAction::SetAlias, retry, Ok(()));
        assert!(device.info.last_error.is_none());
    }

    #[test]
    fn a_busy_refusal_leaves_the_earlier_failure_reportable() {
        let mut device = Harness::default();

        // The link comes up, ending "Connecting" while profiles still connect.
        let first = device.begin(DeviceActivity::Connecting);
        device.report_connected(true);
        // Another connect is refused: BlueZ is still busy with the first.
        device.connect(Err(bluez_error(BLUEZ_IN_PROGRESS)));

        device.finish(DeviceAction::Connect, first, failed());
        assert_eq!(
            device.info.last_error.as_ref().unwrap().action,
            DeviceAction::Connect
        );
    }

    #[test]
    fn an_untracked_request_turned_down_as_busy_is_recorded() {
        let mut device = Harness::default();
        device.begin(DeviceActivity::Connecting);

        // A profile connect while BlueZ is still connecting the device.
        device.finish(
            DeviceAction::ConnectProfile,
            Ticket::Untracked,
            Err(bluez_error(BLUEZ_IN_PROGRESS)),
        );

        assert_eq!(
            device.info.last_error.as_ref().unwrap().action,
            DeviceAction::ConnectProfile
        );
        assert_eq!(device.info.activity, DeviceActivity::Connecting);
    }

    #[test]
    fn pairing_a_paired_device_records_no_error() {
        let mut device = Harness::default();

        let pair = device.begin(DeviceActivity::Pairing);
        device.finish(
            DeviceAction::Pair,
            pair,
            Err(bluez_error(BLUEZ_ALREADY_EXISTS)),
        );

        assert_eq!(device.info.activity, DeviceActivity::Idle);
        assert!(device.info.last_error.is_none());
    }

    #[test]
    fn a_fallen_back_activity_whose_outcome_arrived_ends() {
        let mut device = Harness::default();

        let connect = device.begin(DeviceActivity::Connecting);
        device.finish(DeviceAction::Connect, connect, Ok(()));
        let disconnect = device.begin(DeviceActivity::Disconnecting);
        // The connect completes while the disconnect is outstanding...
        device.info.connected = true;
        // ...then BlueZ turns the disconnect down as busy.
        device.finish(
            DeviceAction::Disconnect,
            disconnect,
            Err(bluez_error(BLUEZ_IN_PROGRESS)),
        );

        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn request_ids_are_not_reused_after_a_rejection() {
        let mut device = Harness::default();

        let rejected = device.begin(DeviceActivity::Connecting);
        let duplicate = device.begin(DeviceActivity::Connecting);
        device.finish(
            DeviceAction::Connect,
            rejected,
            Err(bluez_error(BLUEZ_IN_PROGRESS)),
        );

        // A new request must not be mistaken for the rejected one.
        let next = device.begin(DeviceActivity::Connecting);
        device.finish(DeviceAction::Connect, duplicate, failed());
        assert_eq!(device.info.activity, DeviceActivity::Connecting);
        assert!(device.info.last_error.is_none());

        device.finish(DeviceAction::Connect, next, Ok(()));
        device.report_connected(true);
        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn replies_to_superseded_requests_are_ignored() {
        let mut device = Harness::default();

        // Connect #1 briefly connected, then the link dropped...
        let first = device.begin(DeviceActivity::Connecting);
        device.report_connected(true);
        device.report_connected(false);
        // ...and the user connects again.
        let second = device.begin(DeviceActivity::Connecting);

        // #1's late failure neither ends #2 nor reports an error...
        device.finish(DeviceAction::Connect, first, failed());
        assert_eq!(device.info.activity, DeviceActivity::Connecting);
        assert!(device.info.last_error.is_none());

        // ...and a late success can't mark #2 accepted, so an unrelated report
        // before #2's own reply doesn't end it.
        device.finish(DeviceAction::Connect, first, Ok(()));
        device.report_connected(false);
        assert_eq!(device.info.activity, DeviceActivity::Connecting);

        // #2's own reply still counts.
        device.finish(DeviceAction::Connect, second, Ok(()));
        device.report_connected(true);
        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }

    #[test]
    fn untracked_failures_are_recorded() {
        let mut device = Harness::default();

        let ticket = device.tracker.begin(&mut device.info, None);
        device.finish(DeviceAction::SetAlias, ticket, failed());

        assert_eq!(
            device.info.last_error.as_ref().unwrap().action,
            DeviceAction::SetAlias
        );
        assert_eq!(device.info.activity, DeviceActivity::Idle);
    }
}
