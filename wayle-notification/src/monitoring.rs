use std::{collections::HashSet, sync::Arc, time::Duration};

use chrono::Utc;
use futures::StreamExt;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};
use wayle_core::Property;
use wayle_traits::ServiceMonitoring;
use zbus::{Connection, fdo::DBusProxy};

use crate::{
    core::{
        notification::Notification,
        types::{Actions, NotificationId, Timeout},
    },
    error::Error,
    events::NotificationEvent,
    persistence::NotificationStore,
    popup_timer::PopupTimerManager,
    service::NotificationService,
    types::ClosedReason,
};

impl ServiceMonitoring for NotificationService {
    type Error = Error;
    #[instrument(skip_all, err)]
    async fn start_monitoring(&self) -> Result<(), Self::Error> {
        handle_notifications(self).await?;
        Ok(())
    }
}

#[instrument(skip_all)]
async fn handle_notifications(service: &NotificationService) -> Result<(), Error> {
    let mut event_receiver = service.notif_tx.subscribe();
    let notification_list = service.notifications.clone();
    let popup_list = service.popups.clone();
    let popup_dur = service.popup_duration.clone();
    let dnd = service.dnd.clone();
    let store = service.store.clone();
    let cancellation_token = service.cancellation_token.clone();
    let remove_expired = service.remove_expired.clone();
    let connection = service.connection.clone();
    let notif_tx = service.notif_tx.clone();
    let popup_timers = service.popup_timers.clone();

    // Notifications restored from disk are seeded straight into the list (they bypass the Add
    // pipeline), so arm their history-expiry timers here — the same arming fresh adds get in
    // handle_notification_added — otherwise a restored finite-timeout notification never expires.
    for notif in notification_list.get() {
        arm_history_expiry(&notif, &remove_expired, &notif_tx);
    }

    tokio::spawn(async move {
        // Best-effort proxy for the add-time reachability check below. If it can't be
        // created, freshly-added notifications simply aren't re-checked here (the startup
        // seed and the disconnect watcher still cover their cases).
        let reachability_proxy = DBusProxy::new(&connection).await.ok();

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    info!("Notification monitoring cancelled, stopping");
                    return;
                }
                event = event_receiver.recv() => {
                    let event = match event {
                        Ok(event) => event,
                        // A refutable `Ok(event)` pattern would silently disable this branch on
                        // Lagged/Closed — on Closed that busy-spins the loop. Handle both: warn on
                        // lag (events were dropped, unreconstructible), and stop cleanly on close.
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!(skipped, "notification monitor lagged; some events were dropped");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Notification event channel closed, stopping monitor");
                            return;
                        }
                    };
                    match event {
                        NotificationEvent::Add(notif) => {
                            // Gate on dispatch reachability BEFORE presenting: if the target
                            // app is neither running nor D-Bus-activatable, a click could only
                            // fail, so strip the actions up front — the card is then built
                            // non-clickable and never flashes a broken click affordance.
                            if let Some(proxy) = &reachability_proxy {
                                strip_if_unreachable(&notif, proxy).await;
                            }
                            handle_notification_added(
                                &notif,
                                &notification_list,
                                &store,
                                &remove_expired,
                                &notif_tx
                            );
                            handle_popup_added(
                                &notif,
                                &popup_list,
                                &popup_dur,
                                dnd.clone(),
                                &popup_timers,
                            );
                        }
                        NotificationEvent::Remove(id, reason) => {
                            handle_notification_removed(
                                id,
                                reason,
                                &notification_list,
                                &popup_list,
                                &store,
                                &connection,
                                &popup_timers,
                            ).await;
                        }
                        NotificationEvent::RemoveMany(ids, reason) => {
                            handle_notifications_removed_batch(
                                ids,
                                reason,
                                &notification_list,
                                &popup_list,
                                &store,
                                &connection,
                                &popup_timers,
                            ).await;
                        }
                        NotificationEvent::DismissPopup(id) => {
                            dismiss_popup(id, &popup_list, &popup_timers);
                        }
                        NotificationEvent::InhibitPopup(id) => popup_timers.pause(id),
                        NotificationEvent::ReleasePopup(id) => popup_timers.resume(id),
                    }
                }
            }
        }
    });

    spawn_owner_watching(
        service.notifications.clone(),
        service.popups.clone(),
        service.connection.clone(),
        service.cancellation_token.clone(),
    );

    Ok(())
}

/// Removes a notification's actions in place so it is no longer clickable — reset within a single
/// atomic `view` update, so an observer sees a consistent action-less state.
fn strip_actions(notif: &Notification) {
    let mut view = notif.view.get();
    view.actions = Actions::default();
    notif.view.set(view);
}

/// Reactively strips a notification's actions once its dispatch target is unreachable and
/// cannot be reached later — a freedesktop owner whose connection is gone, or a GTK app
/// that has exited and is not D-Bus-activatable (so it can't be cold-launched). GTK
/// notifications for activatable apps keep their actions even after the app exits.
///
/// Fully signal-driven: subscribe to `NameOwnerChanged`, seed once from the current bus
/// names, then react to disconnects. No polling.
fn spawn_owner_watching(
    notifications: Property<Vec<Arc<Notification>>>,
    popups: Property<Vec<Arc<Notification>>>,
    connection: Connection,
    cancellation_token: CancellationToken,
) {
    tokio::spawn(async move {
        let Ok(dbus_proxy) = DBusProxy::new(&connection).await else {
            warn!("cannot create DBus proxy for notification owner watching");
            return;
        };

        // Subscribe before seeding so no disconnect is missed in the gap between them.
        let Ok(mut name_owner_changed) = dbus_proxy.receive_name_owner_changed().await else {
            warn!("cannot subscribe to NameOwnerChanged for notification owner watching");
            return;
        };

        // Seed: reconcile notifications restored from a prior session and apps that
        // exited while we were down.
        let live = fetch_names(&dbus_proxy).await;
        let activatable = fetch_activatable_names(&dbus_proxy).await;
        for notif in notifications.get().iter().chain(popups.get().iter()) {
            if should_strip(notif, &live, &activatable) {
                strip_actions(notif);
            }
        }

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => return,
                signal = name_owner_changed.next() => {
                    let Some(signal) = signal else { return };
                    let Ok(args) = signal.args() else { continue };

                    let disconnected = args.old_owner().is_some() && args.new_owner().is_none();
                    if !disconnected {
                        continue;
                    }

                    let vanished = args.name().to_string();
                    react_to_disconnect(&dbus_proxy, &notifications, &popups, &vanished).await;
                }
            }
        }
    });
}

/// Strips a to-be-added notification's actions when its dispatch target is *already*
/// unreachable, so the card is built non-clickable and never flashes a click that could only
/// fail. Called before the notification is added to the history/popup lists (so the strip is
/// captured when it is cloned into them) — closing the gap between the startup seed and the
/// disconnect watcher, neither of which sees a freshly-delivered notification.
///
/// Only GTK notifications can be born undispatchable: an fdo notification's owner just called
/// `Notify` (so it is alive), and a portal notification always routes through the persistent
/// portal frontend. So the (two D-Bus round-trip) check is skipped for those. In practice a
/// running `GApplication` owns its `app_id` bus name by the time it sends, so a real GTK
/// notification is reachable here and this neither strips nor perceptibly delays it.
async fn strip_if_unreachable(incoming: &Notification, dbus_proxy: &DBusProxy<'_>) {
    // Only backends that can deliver an already-unreachable notification (GTK) pay for the
    // reachability check at ingest; freedesktop (owner just called Notify) and portal (always
    // reachable) skip it. The per-backend rule lives on the trait, not a match here.
    if !incoming.dispatch.get().backend().may_be_unreachable_at_ingest() {
        return;
    }

    let live = fetch_names(dbus_proxy).await;
    let activatable = fetch_activatable_names(dbus_proxy).await;
    if should_strip(incoming, &live, &activatable) {
        strip_actions(incoming);
        debug!(
            id = %incoming.id,
            "notification target unreachable; actions stripped before display"
        );
    }
}

/// Whether a notification's actions should be stripped given the currently-owned (`live`) and
/// D-Bus-activatable (`activatable`) bus names — the per-backend rule lives on the [`Backend`]
/// trait ([`Backend::is_unreachable`](crate::core::backend::Backend::is_unreachable)).
fn should_strip(
    notif: &Notification,
    live: &HashSet<String>,
    activatable: &HashSet<String>,
) -> bool {
    notif.dispatch.get().backend().is_unreachable(live, activatable)
}

/// Re-evaluates notifications when a bus name vanishes: if any notification's dispatch target is
/// that name, refetch the live/activatable sets and strip everything now unreachable. The
/// relevance check and the reachability rule are both the [`Backend`] trait's, so no protocol
/// match happens here.
async fn react_to_disconnect(
    dbus_proxy: &DBusProxy<'_>,
    notifications: &Property<Vec<Arc<Notification>>>,
    popups: &Property<Vec<Arc<Notification>>>,
    vanished: &str,
) {
    let notifs = notifications.get();
    let pops = popups.get();

    // Skip the (two-round-trip) reconciliation unless some notification actually dispatches to
    // the vanished name.
    let affected = notifs
        .iter()
        .chain(pops.iter())
        .any(|notif| notif.dispatch.get().backend().dispatch_target() == Some(vanished));
    if !affected {
        return;
    }

    // Fetched *after* the disconnect, so `live` no longer contains `vanished`; a freedesktop
    // owner or a non-activatable GTK app that just left is therefore judged unreachable.
    let live = fetch_names(dbus_proxy).await;
    let activatable = fetch_activatable_names(dbus_proxy).await;
    for notif in notifs.iter().chain(pops.iter()) {
        if should_strip(notif, &live, &activatable) {
            debug!(id = %notif.id, name = %vanished, "dispatch target unreachable, stripping actions");
            strip_actions(notif);
        }
    }
}

async fn fetch_names(dbus_proxy: &DBusProxy<'_>) -> HashSet<String> {
    match dbus_proxy.list_names().await {
        Ok(names) => names.into_iter().map(|name| name.to_string()).collect(),
        Err(err) => {
            warn!(error = %err, "cannot list D-Bus names to reconcile notification owners");
            HashSet::new()
        }
    }
}

async fn fetch_activatable_names(dbus_proxy: &DBusProxy<'_>) -> HashSet<String> {
    match dbus_proxy.list_activatable_names().await {
        Ok(names) => names.into_iter().map(|name| name.to_string()).collect(),
        Err(err) => {
            warn!(error = %err, "cannot list activatable D-Bus names");
            HashSet::new()
        }
    }
}

/// Removes a notification from the visible popups (keeping it in history) and cancels its
/// popup timer — the popup-only counterpart to a full removal, driven by
/// [`Notification::dismiss_popup`].
fn dismiss_popup(
    id: NotificationId,
    popups: &Property<Vec<Arc<Notification>>>,
    popup_timers: &Arc<PopupTimerManager>,
) {
    popup_timers.cancel(id);
    let mut list = popups.get();
    list.retain(|popup| popup.id != id);
    popups.set(list);
}

fn handle_popup_added(
    incoming_popup: &Notification,
    popups: &Property<Vec<Arc<Notification>>>,
    popup_duration: &Property<u32>,
    dnd: Property<bool>,
    popup_timers: &Arc<PopupTimerManager>,
) {
    if dnd.get() {
        return;
    }

    let mut list = popups.get();

    // Stable identity (mirrors the history list): update an existing popup's Property
    // fields in place instead of replacing its Arc, so the popup card reacts to the
    // change rather than being left bound to a stale Arc.
    let popup = match list.iter().find(|popup| popup.id == incoming_popup.id).cloned() {
        Some(existing) => {
            existing.update_from(incoming_popup);
            existing
        }
        None => Arc::new(incoming_popup.clone()),
    };

    list.retain(|p| p.id != popup.id);
    list.insert(0, popup.clone());
    popups.replace(list);

    let default_duration = Duration::from_millis(popup_duration.get() as u64);

    // The banner always auto-hides (it stays in history); only the app's explicit
    // never-expire suppresses the timer. A finite request is clamped to the default.
    match popup.view.get().lifecycle.timeout {
        Timeout::NeverByApp => {}
        Timeout::After(ttl) => popup_timers.start(popup.id, default_duration.min(ttl)),
        Timeout::ServerDefault | Timeout::PersistentByBackend => {
            popup_timers.start(popup.id, default_duration);
        }
    }
}

fn handle_notification_added(
    incoming_notif: &Notification,
    notifications: &Property<Vec<Arc<Notification>>>,
    store: &Option<NotificationStore>,
    remove_expired: &Property<bool>,
    notif_tx: &broadcast::Sender<NotificationEvent>,
) {
    let mut list = notifications.get();

    // Transient notifications are not kept in history. If a notification is flipped to
    // transient over an existing id, drop the stale history entry so it leaves history.
    if incoming_notif.view.get().classification.transient {
        if list.iter().any(|notif| notif.id == incoming_notif.id) {
            list.retain(|notif| notif.id != incoming_notif.id);
            notifications.replace(list);
            if let Some(store) = store.as_ref() {
                let _ = store.remove(incoming_notif.id);
            }
        }
        return;
    }

    // Stable identity: update an existing notification in place (observers react via its
    // Property fields) rather than replacing its Arc; only mint a new Arc for a new id.
    let notif_arc = match list.iter().find(|notif| notif.id == incoming_notif.id).cloned() {
        Some(existing) => {
            // `update_from` replaces the whole dispatch, which now carries the owner, so an app
            // that reconnected and replaced its own notification has its directed-signal target
            // refreshed automatically.
            existing.update_from(incoming_notif);
            debug!(
                id = %existing.id,
                app = ?existing.view.get().origin.name,
                "updating existing notification in place"
            );
            existing
        }
        None => {
            debug!(
                id = %incoming_notif.id,
                app = ?incoming_notif.view.get().origin.name,
                summary = %incoming_notif.view.get().content.summary,
                list_size = list.len(),
                "adding new notification"
            );
            Arc::new(incoming_notif.clone())
        }
    };

    // Move to (or keep at) the front, preserving the Arc identity.
    list.retain(|notif| notif.id != notif_arc.id);
    list.insert(0, notif_arc.clone());
    notifications.replace(list);

    if let Some(store) = store.as_ref() {
        let _ = store.add(incoming_notif);
    };

    arm_history_expiry(&notif_arc, remove_expired, notif_tx);
}

/// Arms the history-expiry timer for a notification with an explicit finite timeout (or removes
/// it immediately if already past its deadline). No-op unless `remove_expired` is set and the
/// timeout is [`Timeout::After`]. Shared by fresh adds ([`handle_notification_added`]) and the
/// startup pass over notifications restored from disk, so a restored notification expires on the
/// same schedule as a freshly-received one instead of lingering in history until dismissed.
fn arm_history_expiry(
    notif: &Arc<Notification>,
    remove_expired: &Property<bool>,
    notif_tx: &broadcast::Sender<NotificationEvent>,
) {
    if !remove_expired.get() {
        return;
    }

    // Only an explicit finite timeout auto-removes from history; server-default and the
    // backend-persistent / never-by-app cases stay until dismissed (the banner still hides).
    let Timeout::After(ttl) = notif.view.get().lifecycle.timeout else {
        return;
    };

    let expiration_time = notif.view.get().received + ttl;
    let id = notif.id;

    // All expiry — already-past or future — is delivered through the normal `Remove` event from a
    // spawned timer task, so it drops from BOTH the live list and the store (and emits any
    // close-back signal) via the one removal path. A past deadline yields a zero-length sleep, so
    // even then the send happens asynchronously AFTER this (synchronous) arming pass returns and
    // the event loop is draining — never a synchronous burst into the broadcast channel before any
    // receiver runs. This also closes the startup race: a finite notification whose deadline passes
    // between the persistence load snapshot and arming was still loaded (kept in the store), so
    // removing it here deletes it from the store too — it is never orphaned.
    let time_until_expiration = (expiration_time - Utc::now())
        .to_std()
        .unwrap_or(Duration::ZERO);
    let tx = notif_tx.clone();

    tokio::spawn(async move {
        tokio::time::sleep(time_until_expiration).await;
        let _ = tx.send(NotificationEvent::Remove(id, ClosedReason::Expired));
    });
}

async fn handle_notification_removed(
    id: NotificationId,
    reason: ClosedReason,
    notifications: &Property<Vec<Arc<Notification>>>,
    popups: &Property<Vec<Arc<Notification>>>,
    store: &Option<NotificationStore>,
    connection: &Connection,
    popup_timers: &Arc<PopupTimerManager>,
) {
    if !matches!(reason, ClosedReason::Expired) {
        popup_timers.cancel(id);

        let mut popup_list = popups.get();
        popup_list.retain(|popup| popup.id != id);
        popups.set(popup_list);
    }

    let mut notif_list = notifications.get();
    let prev_len = notif_list.len();
    // Keep the removed notification so its backend can emit any close-back signal (freedesktop
    // directs `NotificationClosed(wire_id, reason)` to its owner; GTK/portal no-op). The core
    // never matches on the protocol — it forwards through the `Backend` trait.
    let removed = notif_list.iter().find(|notif| notif.id == id).cloned();
    notif_list.retain(|notif| notif.id != id);

    if notif_list.len() == prev_len {
        return;
    }

    notifications.set(notif_list);

    if let Some(store) = store.as_ref() {
        let _ = store.remove(id);
    };

    if let Some(notif) = removed {
        let dispatch = notif.dispatch.get();
        if let Err(err) = dispatch.backend().close(connection, reason).await {
            warn!(id = %id, error = %err, "cannot emit close signal");
        }
    }
}

/// Removes several notifications at once ("clear all" / clear a group). Does the reactive
/// list updates and the store delete a SINGLE time for the whole batch, instead of the
/// O(n) rebuild + one `DELETE` per id that repeated [`handle_notification_removed`] calls
/// would incur. Only `NotificationClosed` stays per-notification: each fdo client must be
/// told about its own id (directed to its owner; GTK notifications have no owner and no
/// close signal, so they're skipped).
async fn handle_notifications_removed_batch(
    ids: Vec<NotificationId>,
    reason: ClosedReason,
    notifications: &Property<Vec<Arc<Notification>>>,
    popups: &Property<Vec<Arc<Notification>>>,
    store: &Option<NotificationStore>,
    connection: &Connection,
    popup_timers: &Arc<PopupTimerManager>,
) {
    if ids.is_empty() {
        return;
    }
    let id_set: HashSet<NotificationId> = ids.iter().copied().collect();

    // Drop all matched popups (and cancel their timers) in one update, mirroring the
    // single-removal rule that expiry leaves popups untouched.
    if !matches!(reason, ClosedReason::Expired) {
        for id in &ids {
            popup_timers.cancel(*id);
        }
        let mut popup_list = popups.get();
        popup_list.retain(|popup| !id_set.contains(&popup.id));
        popups.set(popup_list);
    }

    // Keep the removed notifications so each backend can emit its own close-back signal after the
    // single list update, then drop them all from history at once.
    let mut notif_list = notifications.get();
    let removed: Vec<Arc<Notification>> = notif_list
        .iter()
        .filter(|notif| id_set.contains(&notif.id))
        .cloned()
        .collect();
    if removed.is_empty() {
        return;
    }
    notif_list.retain(|notif| !id_set.contains(&notif.id));
    notifications.set(notif_list);

    if let Some(store) = store.as_ref() {
        let removed_ids: Vec<NotificationId> = removed.iter().map(|notif| notif.id).collect();
        let _ = store.remove_many(&removed_ids);
    }

    // Each fdo entry is told about its own id (directed to its owner); GTK/portal no-op.
    for notif in removed {
        let dispatch = notif.dispatch.get();
        if let Err(err) = dispatch.backend().close(connection, reason).await {
            warn!(id = %notif.id, error = %err, "cannot emit close signal");
        }
    }
}
