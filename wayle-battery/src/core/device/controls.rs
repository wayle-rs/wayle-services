use std::path::Path;

use tracing::instrument;
use zbus::{Connection, zvariant::OwnedObjectPath};

use crate::{error::Error, proxy::device::DeviceProxy};

pub(super) struct DeviceController;

/// Returns the `charge_types` sysfs path for a device given its UPower
/// `NativePath` (e.g. `/sys/devices/.../power_supply/BAT0`), or `None` if the
/// file does not exist (meaning the kernel uses numeric thresholds instead).
fn charge_types_path(native_path: &str) -> Option<std::path::PathBuf> {
    let path = Path::new(native_path).join("charge_types");
    path.exists().then_some(path)
}

impl DeviceController {
    #[instrument(skip(connection), err)]
    pub(super) async fn refresh(
        connection: &Connection,
        device_path: &OwnedObjectPath,
    ) -> Result<(), Error> {
        let proxy = DeviceProxy::builder(connection)
            .path(device_path)?
            .build()
            .await?;

        proxy.refresh().await?;
        Ok(())
    }

    #[instrument(skip(connection), err)]
    pub(super) async fn get_history(
        connection: &Connection,
        device_path: &OwnedObjectPath,
        history_type: &str,
        timespan: u32,
        resolution: u32,
    ) -> Result<Vec<(u32, f64, u32)>, Error> {
        let proxy = DeviceProxy::builder(connection)
            .path(device_path)?
            .build()
            .await?;

        Ok(proxy
            .get_history(history_type, timespan, resolution)
            .await?)
    }

    #[instrument(skip(connection), err)]
    pub(super) async fn get_statistics(
        connection: &Connection,
        device_path: &OwnedObjectPath,
        stat_type: &str,
    ) -> Result<Vec<(f64, f64)>, Error> {
        let proxy = DeviceProxy::builder(connection)
            .path(device_path)?
            .build()
            .await?;

        Ok(proxy.get_statistics(stat_type).await?)
    }

    /// Enables or disables the battery charge threshold.
    ///
    /// Tries the standard UPower D-Bus method first. If the device does not
    /// support that interface (e.g. Lenovo/HP/Dell laptops that expose a
    /// `charge_types` sysfs file instead of numeric thresholds), falls back to
    /// writing `Long_Life` or `Standard` directly to the sysfs attribute.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dbus`] if UPower reports the device is unsupported and
    /// no `charge_types` sysfs file exists. Returns [`Error::Sysfs`] if the
    /// file exists but the write fails (e.g. insufficient permissions).
    #[instrument(skip(connection), err)]
    pub(super) async fn enable_charge_threshold(
        connection: &Connection,
        device_path: &OwnedObjectPath,
        enabled: bool,
    ) -> Result<(), Error> {
        let proxy = DeviceProxy::builder(connection)
            .path(device_path)?
            .build()
            .await?;

        match proxy.enable_charge_threshold(enabled).await {
            Ok(()) => return Ok(()),
            Err(e) => tracing::debug!(
                error = %e,
                "UPower enable_charge_threshold failed, attempting sysfs fallback"
            ),
        }

        // Obtain the kernel sysfs path for this device from UPower's
        // NativePath property (e.g. /sys/devices/.../power_supply/BAT0).
        let native_path = proxy.native_path().await?;

        if let Some(path) = charge_types_path(&native_path) {
            let mode = if enabled { "Long_Life" } else { "Standard" };
            tracing::debug!(path = %path.display(), mode, "writing charge_types via sysfs");
            std::fs::write(&path, mode).map_err(Error::Sysfs)?;
            return Ok(());
        }

        Err(Error::Dbus(zbus::Error::Failure(
            "charge threshold is not supported on this device".into(),
        )))
    }
}
