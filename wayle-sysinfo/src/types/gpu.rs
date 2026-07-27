/// GPU metrics and per-device snapshots used by the sysinfo service.
use derive_more::Debug;

/// Aggregate GPU metrics across all detected devices.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuData {
    /// Number of detected GPUs.
    pub total_count: usize,

    /// Number of GPUs that are currently reporting metrics.
    pub active_count: usize,

    /// Average GPU core utilization across reporting devices (0.0 - 100.0).
    pub average_utilization_percent: f32,

    /// Average VRAM utilization across reporting devices (0.0 - 100.0).
    pub average_memory_utilization_percent: f32,

    /// Per-device GPU metrics.
    #[debug(skip)]
    pub devices: Vec<GpuDeviceData>,
}

/// Per-device GPU metrics snapshot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuDeviceData {
    /// Zero-based device index from NVML.
    pub index: u32,

    /// Human-readable GPU name (for example: "NVIDIA GeForce RTX 4090").
    pub name: String,

    /// GPU UUID.
    pub uuid: String,

    /// GPU core utilization percentage (0.0 - 100.0), if available.
    pub utilization_percent: Option<f32>,

    /// VRAM used in bytes, if available.
    pub memory_used_bytes: Option<u64>,

    /// Total VRAM in bytes, if available.
    pub memory_total_bytes: Option<u64>,

    /// VRAM utilization percentage (0.0 - 100.0), if available.
    pub memory_utilization_percent: Option<f32>,

    /// GPU temperature in Celsius, if available.
    pub temperature_celsius: Option<f32>,

    /// Current board power draw in watts, if available.
    pub power_watts: Option<f32>,

    /// Configured power limit in watts, if available.
    pub power_limit_watts: Option<f32>,

    /// Current fan speed percentage (0.0 - 100.0), if available.
    pub fan_speed_percent: Option<f32>,

    /// Current graphics clock in MHz, if available.
    pub graphics_clock_mhz: Option<u32>,

    /// Current memory clock in MHz, if available.
    pub memory_clock_mhz: Option<u32>,
}
