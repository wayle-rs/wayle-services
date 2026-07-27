use std::collections::HashSet;

use async_trait::async_trait;
use zbus::Connection;

use crate::{
    core::types::{ActionId, Uri},
    error::Error,
    types::ClosedReason,
};

/// The bits a backend needs to dispatch an action or reply that are NOT part of its own
/// per-notification dispatch data, threaded in via one borrowed context so no backend-specific
/// field leaks into a per-method signature. `urls` is read only by the freedesktop backend
/// (the "default click opens the first link" rule); the others ignore it. The freedesktop
/// directed-signal target lives inside the dispatch itself (`FreedesktopDispatch::owner`), not
/// here, so "owner is meaningful only for freedesktop" is structural.
pub(crate) struct DispatchCtx<'a> {
    pub connection: &'a Connection,
    /// The common `actions.urls` facet, so the freedesktop "default click opens the first
    /// link" rule lives in the freedesktop backend rather than the core. Empty for GTK/portal.
    pub urls: &'a [Uri],
    /// An `xdg-activation-v1` token minted by the shell at click time (the headless daemon
    /// can't mint one), so the invoked app may raise its window past focus-stealing prevention.
    /// Each backend hands it to the app its own way — freedesktop `ActivationToken` signal, GTK
    /// `ActivateAction` platform-data, portal `ActionInvoked` platform-data. `None` when the
    /// shell couldn't mint one (or for a non-raising interaction like reply).
    pub activation_token: Option<&'a str>,
}

/// The per-notification backend interface: every axis on which the three protocols diverge —
/// action dispatch, inline reply, reachability/action-stripping, and the close-back signal —
/// lives here, so the core forwards through [`NotificationSource::backend`] and never matches on
/// the protocol. Each dispatch struct
/// ([`FreedesktopDispatch`](crate::core::types::FreedesktopDispatch) /
/// [`GtkDispatch`](crate::core::types::GtkDispatch) /
/// [`PortalDispatch`](crate::core::types::PortalDispatch)) implements it in its own backend
/// module; adding a fourth backend is one enum variant plus one impl, touching no core logic.
#[async_trait]
pub(crate) trait Backend: Send + Sync {
    /// Dispatches a button, or the default (body-click) action when `action.is_default()`.
    async fn dispatch_action(&self, ctx: &DispatchCtx<'_>, action: &ActionId) -> Result<(), Error>;

    /// Delivers an inline text reply, returning whether it was actually delivered. Default =
    /// `Ok(false)` (GNotification has no reply mechanism, so nothing is sent and the caller must
    /// not treat it as delivered). freedesktop and portal override it and return `Ok(true)` when
    /// they emit the reply.
    async fn reply(&self, _ctx: &DispatchCtx<'_>, _text: &str) -> Result<bool, Error> {
        Ok(false)
    }

    /// Emits the backend's "closed" signal back to the originating app, if it has one. Default =
    /// no-op (GTK/portal have no per-notification close-back signal); freedesktop overrides it to
    /// emit `NotificationClosed(wire_id, reason)` directed to its owner.
    async fn close(&self, _connection: &Connection, _reason: ClosedReason) -> Result<(), Error> {
        Ok(())
    }

    /// Whether this notification's actions can never fire again given the currently-owned
    /// (`live`) and D-Bus-activatable (`activatable`) bus names — so they should be stripped.
    /// freedesktop: its owner is gone (or unknown). GTK: its app is neither running nor
    /// cold-launchable. Portal: never (the persistent portal frontend is always reachable).
    fn is_unreachable(&self, live: &HashSet<String>, activatable: &HashSet<String>) -> bool;

    /// The bus name whose disappearance can invalidate this notification's actions (freedesktop:
    /// the owner's unique name; GTK: the app id; portal: `None`, never invalidated). Used to skip
    /// the reachability re-check when an unrelated name drops off the bus.
    fn dispatch_target(&self) -> Option<&str>;

    /// Whether a *freshly delivered* notification of this backend can already be unreachable (so
    /// it must be checked before display). Only GTK apps can be born unreachable (not running,
    /// not activatable); a freedesktop owner just called `Notify` (alive) and the portal frontend
    /// is always reachable, so those skip the (two-round-trip) ingest check.
    fn may_be_unreachable_at_ingest(&self) -> bool {
        false
    }
}
