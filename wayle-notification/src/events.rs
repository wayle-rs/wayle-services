use crate::{
    core::{notification::Notification, types::NotificationId},
    types::ClosedReason,
};

#[derive(Clone)]
pub(crate) enum NotificationEvent {
    Add(Box<Notification>),
    Remove(NotificationId, ClosedReason),
    /// Batch removal of several notifications in one atomic pass ("clear all" / clearing
    /// a group), so the store delete and the reactive list update happen once instead of
    /// once per notification.
    RemoveMany(Vec<NotificationId>, ClosedReason),
    /// Remove a notification from the visible popups only (keeping it in history) and cancel
    /// its popup timer. Emitted by [`Notification::dismiss_popup`].
    DismissPopup(NotificationId),
    /// Pause a popup's auto-dismiss timer (e.g. while hovered). Emitted by
    /// [`Notification::inhibit_popup`].
    InhibitPopup(NotificationId),
    /// Resume a popup's auto-dismiss timer after an inhibit. Emitted by
    /// [`Notification::release_popup`].
    ReleasePopup(NotificationId),
}
