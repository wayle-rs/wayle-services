use std::{
    path::PathBuf,
    sync::{Arc, Weak},
};

use futures::StreamExt;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use wayle_traits::ModelMonitoring;

use super::{
    TrackMetadata,
    art::{ArtResolver, ResolveResult, next_fallback, thumbnail_for_page_url},
};
use crate::{error::Error, proxy::MediaPlayer2PlayerProxy};

impl ModelMonitoring for TrackMetadata {
    type Error = Error;

    async fn start_monitoring(self: Arc<Self>) -> Result<(), Self::Error> {
        let Some(ref proxy) = self.proxy else {
            return Err(Error::Initialization(String::from("missing proxy")));
        };

        let Some(ref cancellation_token) = self.cancellation_token else {
            return Err(Error::Initialization(String::from(
                "missing cancellation token",
            )));
        };

        let weak_self = Arc::downgrade(&self);

        tokio::spawn(monitor_dbus(
            weak_self.clone(),
            proxy.clone(),
            cancellation_token.clone(),
        ));

        if let Some(ref resolver) = self.art_resolver {
            tokio::spawn(resolve_art_changes(
                weak_self,
                resolver.clone(),
                cancellation_token.clone(),
            ));
        }

        Ok(())
    }
}

async fn monitor_dbus(
    weak_metadata: Weak<TrackMetadata>,
    proxy: MediaPlayer2PlayerProxy<'static>,
    cancellation_token: CancellationToken,
) {
    let mut metadata_changed = proxy.receive_metadata_changed().await;

    loop {
        let Some(metadata) = weak_metadata.upgrade() else {
            return;
        };

        tokio::select! {
            _ = cancellation_token.cancelled() => {
                debug!("metadata D-Bus monitor cancelled");
                return;
            }
            Some(change) = metadata_changed.next() => {
                if let Ok(new_metadata) = change.get().await {
                    TrackMetadata::update_from_dbus(&metadata, new_metadata);
                }
            }
            else => break
        }
    }

    debug!("metadata D-Bus monitor ended");
}

async fn resolve_art_changes(
    weak_metadata: Weak<TrackMetadata>,
    resolver: ArtResolver,
    cancellation_token: CancellationToken,
) {
    let Some(metadata) = weak_metadata.upgrade() else {
        return;
    };
    let mut art_url_stream = Box::pin(metadata.art_url.watch());
    let mut page_url_stream = Box::pin(metadata.url.watch());
    drop(metadata);

    let mut pending_download: Option<JoinHandle<()>> = None;
    let mut art_url: Option<String> = None;
    let mut page_url: Option<String> = None;
    let mut last_resolved: Option<String> = None;

    loop {
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                abort_pending(&mut pending_download);
                debug!("art resolver cancelled");
                return;
            }
            Some(new_art_url) = art_url_stream.next() => {
                art_url = new_art_url;
            }
            Some(new_page_url) = page_url_stream.next() => {
                page_url = new_page_url;
            }
            else => break,
        }

        let effective = effective_art_source(art_url.as_deref(), page_url.as_deref());
        if effective == last_resolved {
            continue;
        }
        last_resolved = effective.clone();

        abort_pending(&mut pending_download);
        pending_download = handle_art_url_change(effective, &resolver, &weak_metadata);
    }

    debug!("art resolver ended");
}

/// The URL to try resolving as artwork: `art_url` when the player provides
/// one, otherwise a best-effort thumbnail derived from the page/stream URL
/// (e.g. a YouTube watch page) for players that never publish `mpris:artUrl`.
fn effective_art_source(art_url: Option<&str>, page_url: Option<&str>) -> Option<String> {
    if let Some(art_url) = art_url {
        return Some(art_url.to_string());
    }
    thumbnail_for_page_url(page_url?)
}

fn handle_art_url_change(
    art_url: Option<String>,
    resolver: &ArtResolver,
    weak_metadata: &Weak<TrackMetadata>,
) -> Option<JoinHandle<()>> {
    let Some(url) = art_url else {
        set_cover_art(weak_metadata, None);
        return None;
    };

    match resolver.resolve(&url) {
        ResolveResult::Ready(local_path) => {
            set_cover_art(weak_metadata, Some(local_path));
            None
        }
        ResolveResult::NeedsDownload {
            url: download_url,
            dest,
        } => {
            let weak = weak_metadata.clone();
            let resolver = resolver.clone();
            Some(tokio::spawn(async move {
                let Some(local_path) =
                    download_with_fallback(&resolver, download_url.clone(), dest).await
                else {
                    warn!(url = %download_url, "album art download failed (all qualities)");
                    return;
                };

                let Some(metadata) = weak.upgrade() else {
                    return;
                };
                let current = effective_art_source(
                    metadata.art_url.get().as_deref(),
                    metadata.url.get().as_deref(),
                );
                let stale = current.as_deref() != Some(download_url.as_str());
                if !stale {
                    metadata.cover_art.set(Some(local_path));
                }
            }))
        }
        ResolveResult::Unresolvable => {
            set_cover_art(weak_metadata, None);
            None
        }
    }
}

/// Downloads `url`, retrying at progressively lower YouTube thumbnail
/// qualities (see [`next_fallback`]) if it fails -- `maxresdefault` isn't
/// generated for every video, but the chain always bottoms out at
/// `hqdefault`, which is. For non-YouTube URLs `next_fallback` immediately
/// returns `None`, so this is a single attempt, same as before.
async fn download_with_fallback(
    resolver: &ArtResolver,
    mut url: String,
    mut dest: PathBuf,
) -> Option<String> {
    loop {
        match ArtResolver::download(&url, &dest).await {
            Ok(path) => return Some(path),
            Err(err) => {
                let fallback = next_fallback(&url)?;
                debug!(error = %err, failed_url = %url, next_url = %fallback, "art candidate failed, trying next quality");
                dest = resolver.cache_path(&fallback);
                url = fallback;
            }
        }
    }
}

fn abort_pending(handle: &mut Option<JoinHandle<()>>) {
    if let Some(handle) = handle.take() {
        handle.abort();
    }
}

fn set_cover_art(weak_metadata: &Weak<TrackMetadata>, value: Option<String>) {
    if let Some(metadata) = weak_metadata.upgrade() {
        metadata.cover_art.set(value);
    }
}
