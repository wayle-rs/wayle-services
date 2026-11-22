use std::time::Duration;

use nvml_wrapper::{
    Nvml,
    enum_wrappers::device::{Clock, TemperatureSensor},
};
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use wayle_core::Property;

use crate::types::{GpuData, GpuDeviceData};

pub(crate) fn spawn(token: CancellationToken, gpu: Property<GpuData>, poll_interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = interval(poll_interval);
        let mut nvml: Option<Nvml> = None;
        let mut init_error_logged = false;

        loop {
            if !gpu.has_subscribers() {
                // Drop NVML handle when there are no subscribers.
                nvml = None;
                init_error_logged = false;

                tokio::select! {
                    _ = token.cancelled() => {
                        debug!("GPU polling cancelled");
                        return;
                    }
                    _ = gpu.wait_for_subscribers() => {}
                }
                ticker.reset();
            }

            if !gpu.has_subscribers() {
                continue;
            }

            if nvml.is_none() {
                match Nvml::init() {
                    Ok(handle) => {
                        nvml = Some(handle);
                        init_error_logged = false;
                    }
                    Err(err) => {
                        if !init_error_logged {
                            warn!(error = ?err, "NVML initialization failed; GPU metrics unavailable");
                            init_error_logged = true;
                        }

                        gpu.set(GpuData::default());

                        tokio::select! {
                            _ = token.cancelled() => {
                                debug!("GPU polling cancelled");
                                return;
                            }
                            _ = ticker.tick() => {}
                        }
                        continue;
                    }
                }
            }

            let snapshot = nvml
                .as_ref()
                .map_or_else(GpuData::default, collect_gpu_data);

            gpu.set(snapshot);

            tokio::select! {
                _ = token.cancelled() => {
                    debug!("GPU polling cancelled");
                    return;
                }
                _ = ticker.tick() => {}
            }
        }
    });
}

fn collect_gpu_data(nvml: &Nvml) -> GpuData {
    let count = match nvml.device_count() {
        Ok(count) => count,
        Err(err) => {
            warn!(error = ?err, "Failed to read GPU device count from NVML");
            return GpuData::default();
        }
    };

    let mut devices = Vec::with_capacity(count as usize);

    for index in 0..count {
        match collect_device_data(nvml, index) {
            Some(device) => devices.push(device),
            None => {
                debug!(index, "Skipping GPU device due to read errors");
            }
        }
    }

    let total_count = count as usize;

    let mut util_sum = 0.0f32;
    let mut util_count = 0usize;
    let mut mem_sum = 0.0f32;
    let mut mem_count = 0usize;

    for d in &devices {
        if let Some(util) = d.utilization_percent {
            util_sum += util;
            util_count += 1;
        }

        if let Some(mem_util) = d.memory_utilization_percent {
            mem_sum += mem_util;
            mem_count += 1;
        }
    }

    let average_utilization_percent = if util_count > 0 {
        util_sum / util_count as f32
    } else {
        0.0
    };

    let average_memory_utilization_percent = if mem_count > 0 {
        mem_sum / mem_count as f32
    } else {
        0.0
    };

    GpuData {
        total_count,
        active_count: util_count,
        average_utilization_percent,
        average_memory_utilization_percent,
        devices,
    }
}

fn collect_device_data(nvml: &Nvml, index: u32) -> Option<GpuDeviceData> {
    let device = match nvml.device_by_index(index) {
        Ok(device) => device,
        Err(err) => {
            debug!(index, error = ?err, "Failed to access GPU device");
            return None;
        }
    };

    let name = device.name().unwrap_or_else(|_| String::from("unknown"));
    let uuid = device.uuid().unwrap_or_else(|_| String::new());

    let utilization = device.utilization_rates().ok();
    let utilization_percent = utilization.as_ref().map(|u| u.gpu as f32);

    let (memory_used_bytes, memory_total_bytes, memory_utilization_percent) =
        match device.memory_info() {
            Ok(info) => {
                let memory_utilization_percent = if info.total > 0 {
                    Some((info.used as f32 / info.total as f32) * 100.0)
                } else {
                    Some(0.0)
                };

                (
                    Some(info.used),
                    Some(info.total),
                    memory_utilization_percent,
                )
            }
            Err(err) => {
                debug!(index, error = ?err, "Failed to read GPU memory info");
                (None, None, None)
            }
        };

    let temperature_celsius = device
        .temperature(TemperatureSensor::Gpu)
        .ok()
        .map(|t| t as f32);

    let power_watts = device.power_usage().ok().map(|mw| mw as f32 / 1000.0);

    let power_limit_watts = device
        .enforced_power_limit()
        .ok()
        .map(|mw| mw as f32 / 1000.0);

    let fan_speed_percent = device.fan_speed(0).ok().map(|s| s as f32);

    let graphics_clock_mhz = device.clock_info(Clock::Graphics).ok();
    let memory_clock_mhz = device.clock_info(Clock::Memory).ok();

    Some(GpuDeviceData {
        index,
        name,
        uuid,
        utilization_percent,
        memory_used_bytes,
        memory_total_bytes,
        memory_utilization_percent,
        temperature_celsius,
        power_watts,
        power_limit_watts,
        fan_speed_percent,
        graphics_clock_mhz,
        memory_clock_mhz,
    })
}
