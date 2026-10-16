mod cycle_runner;
mod timer;
mod watcher;

use std::sync::Arc;

use cycle_runner::CyclingTask;
use futures::StreamExt;
use tokio::sync::broadcast::error::RecvError;
use tracing::{error, info, warn};
use wayle_output::{OutputEvent, OutputService};
use wayle_traits::ServiceMonitoring;

use crate::{error::Error, service::WallpaperService};

impl ServiceMonitoring for WallpaperService {
    type Error = Error;

    async fn start_monitoring(&self) -> Result<(), Self::Error> {
        discover_initial_outputs(self);
        spawn_cycling_task(self);
        Ok(())
    }
}

fn discover_initial_outputs(service: &WallpaperService) {
    let Some(outputs) = &service.outputs else {
        warn!("cannot query wayland outputs, no monitors registered");
        return;
    };

    sync_monitors(service, outputs);
}

/// Registers connected outputs and drops monitors that are gone.
fn sync_monitors(service: &WallpaperService, outputs: &OutputService) {
    let connected: Vec<String> = outputs
        .outputs
        .get()
        .into_iter()
        .map(|monitor| monitor.connector)
        .collect();

    if connected.is_empty() {
        warn!("No Wayland outputs found");
    }

    for name in service.monitor_names() {
        if !connected.contains(&name) {
            service.unregister_monitor(&name);
        }
    }
    for name in &connected {
        service.register_monitor(name);
    }
}

fn spawn_cycling_task(service: &WallpaperService) {
    let task = CyclingTask::new(
        service.cycling.clone(),
        service.monitors.clone(),
        service.transition.clone(),
        service.shared_cycle.clone(),
        service.engine_active.clone(),
    );

    let cancellation = service.cancellation_token.clone();

    tokio::spawn(async move {
        task.run(cancellation).await;
    });
}

pub(crate) fn spawn_output_watcher(service: Arc<WallpaperService>) {
    let Some(outputs) = service.outputs.clone() else {
        return;
    };

    let mut events = outputs.events();
    // catch outputs that changed between discovery and subscribing
    sync_monitors(&service, &outputs);

    let cancellation = service.cancellation_token.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    info!("Output watcher cancelled");
                    return;
                }

                event = events.recv() => match event {
                    Ok(OutputEvent::Added(monitor)) => service.register_monitor(&monitor.connector),
                    Ok(OutputEvent::Removed(connector)) => service.unregister_monitor(&connector),
                    Ok(OutputEvent::Changed(_)) => {}
                    Err(RecvError::Lagged(_)) => sync_monitors(&service, &outputs),
                    Err(RecvError::Closed) => return,
                },
            }
        }
    });
}

pub(crate) fn spawn_color_extractor(service: Arc<WallpaperService>) {
    let cancellation = service.cancellation_token.clone();

    tokio::spawn(async move {
        let mut monitor_watch = service.monitors.watch();
        let mut color_extractor = service.color_extractor.watch();
        let mut theming_monitor = service.theming_monitor.watch();

        info!("Color extractor task started");

        loop {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    info!("Color extractor cancelled");
                    return;
                }

                _ = monitor_watch.next() => {
                    if let Err(e) = service.extract_colors().await {
                        error!(error = %e, "cannot extract colors");
                    }
                }

                _ = color_extractor.next() => {
                    service.last_extracted_wallpaper.set(None);
                    if let Err(e) = service.extract_colors().await {
                        error!(error = %e, "cannot extract colors");
                    }
                }

                _ = theming_monitor.next() => {
                    service.last_extracted_wallpaper.set(None);
                    if let Err(e) = service.extract_colors().await {
                        error!(error = %e, "cannot extract colors");
                    }
                }
            }
        }
    });
}
