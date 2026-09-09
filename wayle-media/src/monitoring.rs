use std::{collections::HashMap, sync::Arc, time::Duration};

use futures::StreamExt;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};
use wayle_core::Property;
use wayle_traits::{Reactive, ServiceMonitoring};
use zbus::{Connection, fdo::DBusProxy};

use super::{core::player::Player, error::Error, types::PlayerId};
use crate::{
    core::{metadata::art::ArtResolver, player::LivePlayerParams},
    selection::{SelectionContext, select_best_player},
    service::MediaService,
    types::PlayerSlot,
};

/// Everything a player dispatcher needs, owned, so per-client work can be
/// spawned freely without borrowing from the service.
#[derive(Clone)]
struct MonitoringContext {
    connection: Connection,
    players: Arc<RwLock<HashMap<PlayerId, PlayerSlot>>>,
    player_list: Property<Vec<Arc<Player>>>,
    active_player: Property<Option<Arc<Player>>>,
    ignored_patterns: Vec<String>,
    priority_patterns: Vec<String>,
    cancellation_token: CancellationToken,
    art_resolver: Option<ArtResolver>,
    position_poll_interval: Duration,
}

const MPRIS_BUS_PREFIX: &str = "org.mpris.MediaPlayer2.";

/// How long a player may sit on its initial snapshot before a diagnostic is
/// logged. Log-only by design: behavior never depends on this deadline — a
/// slow client is published whenever it answers, and a wedged one is reaped
/// when its name leaves the bus.
const INIT_WATCHDOG: Duration = Duration::from_secs(10);

impl ServiceMonitoring for MediaService {
    type Error = Error;

    #[instrument(skip_all)]
    async fn start_monitoring(&self) -> Result<(), Self::Error> {
        let ctx = MonitoringContext {
            connection: self.connection.clone(),
            players: Arc::clone(&self.players),
            player_list: self.player_list.clone(),
            active_player: self.active_player.clone(),
            ignored_patterns: self.ignored_patterns.clone(),
            priority_patterns: self.priority_patterns.clone(),
            cancellation_token: self.cancellation_token.clone(),
            art_resolver: self.art_resolver.clone(),
            position_poll_interval: self.position_poll_interval,
        };

        discover_existing_players(&ctx).await?;
        spawn_name_monitoring(ctx);

        Ok(())
    }
}

/// Enumerate MPRIS names already on the bus and dispatch an initialization
/// task for each. Only the bus daemon is awaited here — never a client — so
/// this returns as soon as the name list is known, and players surface on
/// `player_list` as each answers its initial snapshot.
async fn discover_existing_players(ctx: &MonitoringContext) -> Result<(), Error> {
    let dbus_proxy = DBusProxy::new(&ctx.connection)
        .await
        .map_err(|err| Error::Initialization(format!("d-bus proxy: {err}")))?;

    let names = dbus_proxy
        .list_names()
        .await
        .map_err(|err| Error::Dbus(err.into()))?;

    for name in names {
        if name.starts_with(MPRIS_BUS_PREFIX) && !should_ignore(&name, &ctx.ignored_patterns) {
            let player_id = PlayerId::from_bus_name(&name);
            dispatch_player_added(ctx, player_id).await;
        }
    }

    Ok(())
}

fn spawn_name_monitoring(ctx: MonitoringContext) {
    let loop_token = ctx.cancellation_token.child_token();

    tokio::spawn(async move {
        debug!("MprisMonitoring task spawned");
        let Ok(dbus_proxy) = DBusProxy::new(&ctx.connection).await else {
            warn!("cannot create DBus proxy for name monitoring");
            return;
        };

        let Ok(mut name_owner_changed) = dbus_proxy.receive_name_owner_changed().await else {
            warn!("cannot subscribe to NameOwnerChanged");
            return;
        };

        loop {
            tokio::select! {
                () = loop_token.cancelled() => {
                    debug!("MprisMonitoring received cancellation signal, stopping all discovery");
                    return;
                }
                Some(signal) = name_owner_changed.next() => {
                    let Ok(args) = signal.args() else { continue };

                    if !args.name().starts_with(MPRIS_BUS_PREFIX) {
                        continue;
                    }

                    let player_id = PlayerId::from_bus_name(args.name());

                    let is_player_added = args.old_owner().is_none() && args.new_owner().is_some();
                    let is_player_removed = args.old_owner().is_some() && args.new_owner().is_none();
                    let is_owner_replaced = args.old_owner().is_some() && args.new_owner().is_some();

                    // Only local bookkeeping happens on this loop: adds spawn
                    // their client I/O and removals touch in-process state, so
                    // one unresponsive client can never stall name events for
                    // the others.
                    if is_player_added && !should_ignore(args.name(), &ctx.ignored_patterns) {
                        dispatch_player_added(&ctx, player_id).await;
                    } else if is_player_removed {
                        handle_player_removed(&ctx, player_id).await;
                    } else if is_owner_replaced {
                        handle_player_removed(&ctx, player_id.clone()).await;

                        if !should_ignore(args.name(), &ctx.ignored_patterns) {
                            dispatch_player_added(&ctx, player_id).await;
                        }
                    }
                }
                else => {
                    return;
                }
            }
        }
    });
}

/// Claim a dispatcher generation for `player_id` and spawn its initialization.
///
/// Every round-trip to the client happens on the spawned task; nothing here
/// awaits the client, so a wedged player can never block discovery, the
/// name-event loop, or service startup. Any previous generation for the same
/// id — a live player, or a still-pending init — is cancelled and retracted
/// first, and the slot is claimed before spawning so a removal that races the
/// init can always find and cancel it.
async fn dispatch_player_added(ctx: &MonitoringContext, player_id: PlayerId) {
    let token = ctx.cancellation_token.child_token();

    {
        let mut players = ctx.players.write().await;
        let slot = PlayerSlot {
            token: token.clone(),
            player: None,
        };
        if let Some(old) = players.insert(player_id.clone(), slot) {
            old.token.cancel();
            if old.player.is_some() {
                retract_player(ctx, &player_id);
            }
        }
    }

    let ctx = ctx.clone();
    tokio::spawn(async move {
        init_player(&ctx, player_id, token).await;
    });
}

/// Dispatcher body: snapshot the client's initial state, then publish it.
///
/// The player joins `player_list` only after the client has answered the full
/// snapshot, so a client that never answers simply never appears. There is
/// deliberately no timeout — cancellation is lifecycle-driven: when the client
/// leaves the bus (or is superseded by a new owner), the generation token is
/// cancelled and the pending snapshot is dropped mid-await. The watchdog only
/// logs, so a wedged client is diagnosable without introducing load-dependent
/// behavior.
async fn init_player(ctx: &MonitoringContext, player_id: PlayerId, token: CancellationToken) {
    let init = Player::get_live(LivePlayerParams {
        connection: &ctx.connection,
        player_id: player_id.clone(),
        cancellation_token: &token,
        art_resolver: ctx.art_resolver.clone(),
        position_poll_interval: ctx.position_poll_interval,
    });
    tokio::pin!(init);

    let mut watchdog_fired = false;
    let result = loop {
        tokio::select! {
            () = token.cancelled() => {
                debug!(player_id = %player_id, "player init cancelled (client left or was superseded)");
                return;
            }
            result = &mut init => break result,
            () = tokio::time::sleep(INIT_WATCHDOG), if !watchdog_fired => {
                watchdog_fired = true;
                warn!(
                    player_id = %player_id,
                    "player has not answered its initial property snapshot after {INIT_WATCHDOG:?}; \
                     still waiting (it will be listed when it answers, or dropped when it leaves the bus)"
                );
            }
        }
    };

    let player = match result {
        Ok(player) => player,
        Err(err) => {
            warn!(error = %err, player_id = %player_id, "cannot create player");
            let mut players = ctx.players.write().await;
            if !token.is_cancelled() {
                players.remove(&player_id);
            }
            drop(players);
            // Reap anything a partial init may have spawned (metadata
            // monitors, art fetches).
            token.cancel();
            return;
        }
    };

    let mut players = ctx.players.write().await;
    // Supersession and removal both cancel the generation token under this
    // lock before touching the slot, so an uncancelled token here proves the
    // slot is still ours.
    let slot = if token.is_cancelled() {
        None
    } else {
        players.get_mut(&player_id)
    };
    let Some(slot) = slot else {
        drop(players);
        // The player's monitors are children of this token; unwind them.
        token.cancel();
        debug!(player_id = %player_id, "player removed or superseded before publish");
        return;
    };

    slot.player = Some(Arc::clone(&player));
    publish_player(ctx, player);

    debug!("Player {} added", player_id);
}

/// Remove a player whose bus name disappeared: cancel its generation token —
/// killing its dispatcher, property/position monitors, and any in-flight art
/// fetch, whether or not init ever completed — and retract it from the list.
async fn handle_player_removed(ctx: &MonitoringContext, player_id: PlayerId) {
    let mut players = ctx.players.write().await;
    let Some(slot) = players.remove(&player_id) else {
        return;
    };
    slot.token.cancel();

    if slot.player.is_some() {
        retract_player(ctx, &player_id);
    }

    debug!("Player {} removed", player_id);
}

/// Publish a fully-initialized player on `player_list` and reselect the
/// active player.
///
/// Must be called with the `players` write lock held: list updates are
/// read-modify-write, and that lock is what serializes concurrent dispatchers.
fn publish_player(ctx: &MonitoringContext, player: Arc<Player>) {
    let mut list = ctx.player_list.get();
    list.retain(|existing| existing.id != player.id);
    list.push(player);
    ctx.player_list.set(list.clone());

    let best = select_best_player(&SelectionContext {
        players: &list,
        priority_patterns: &ctx.priority_patterns,
    });
    ctx.active_player.set(best);
}

/// Retract a player from `player_list`, reselecting the active player if it
/// was the one removed.
///
/// Must be called with the `players` write lock held (see [`publish_player`]).
fn retract_player(ctx: &MonitoringContext, player_id: &PlayerId) {
    let mut list = ctx.player_list.get();
    let len_before = list.len();
    list.retain(|player| player.id != *player_id);
    if list.len() == len_before {
        return;
    }
    ctx.player_list.set(list.clone());

    let was_active = ctx
        .active_player
        .get()
        .is_some_and(|current| current.id == *player_id);
    if was_active {
        let best = select_best_player(&SelectionContext {
            players: &list,
            priority_patterns: &ctx.priority_patterns,
        });
        ctx.active_player.set(best);
    }
}

/// Bus name of `playerctld`, the playerctl project's player-mirroring daemon.
/// Only a proxy to other media players — it mirrors the most recently active
/// one verbatim, `Identity` included, so listing it would show that player
/// twice under the same name. Always ignored, like KDE's MPRIS handling does.
/// It can still be addressed explicitly via
/// [`MediaService::player_monitored`](crate::MediaService::player_monitored).
const PLAYERCTLD_BUS_NAME: &str = "org.mpris.MediaPlayer2.playerctld";

fn should_ignore(bus_name: &str, ignored_patterns: &[String]) -> bool {
    if bus_name == PLAYERCTLD_BUS_NAME {
        return true;
    }

    ignored_patterns
        .iter()
        .any(|pattern| bus_name.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_ignore_returns_true_when_pattern_matches() {
        let patterns = vec![String::from("spotify"), String::from("chrome")];
        let bus_name = "org.mpris.MediaPlayer2.spotify";

        assert!(should_ignore(bus_name, &patterns));
    }

    #[test]
    fn should_ignore_returns_false_when_no_patterns_match() {
        let patterns = vec![String::from("spotify"), String::from("chrome")];
        let bus_name = "org.mpris.MediaPlayer2.vlc";

        assert!(!should_ignore(bus_name, &patterns));
    }

    #[test]
    fn should_ignore_with_empty_patterns_returns_false() {
        let patterns = vec![];
        let bus_name = "org.mpris.MediaPlayer2.spotify";

        assert!(!should_ignore(bus_name, &patterns));
    }

    #[test]
    fn should_ignore_matches_substring_in_bus_name() {
        let patterns = vec![String::from("chromium")];
        let bus_name = "org.mpris.MediaPlayer2.chromium.instance123";

        assert!(should_ignore(bus_name, &patterns));
    }

    #[test]
    fn should_ignore_always_ignores_playerctld() {
        assert!(should_ignore("org.mpris.MediaPlayer2.playerctld", &[]));
    }
}
