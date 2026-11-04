use std::{sync::Arc, time::Duration};

use derive_more::Debug;
use tokio_util::sync::CancellationToken;
use wayle_core::Property;
use zbus::zvariant::ObjectPath;

use crate::{
    core::{adapter::Adapter, device::Device},
    dispatcher::{
        self, Handle, Published,
        command::{Command, Commands, PairingResponse},
    },
    error::Error,
    types::{RadioBlock, agent::PairingRequest},
};

/// Bluetooth connectivity via BlueZ D-Bus.
///
/// State comes from BlueZ (and the kernel's rfkill switches), published as
/// reactive properties and kept in sync by one background dispatcher, the
/// only thing that changes it; every adapter and device is a single live
/// instance. Actions return nothing: they are
/// queued to the dispatcher, which sends them to BlueZ in call order, and
/// their outcome shows up as state, just as changes made by any other BlueZ
/// client (e.g. `bluetoothctl`) do. See the [crate-level documentation](crate).
#[derive(Debug)]
pub struct BluetoothService {
    #[debug(skip)]
    commands: Commands,
    #[debug(skip)]
    cancellation_token: CancellationToken,

    /// All Bluetooth adapters on the system (live).
    pub adapters: Property<Vec<Arc<Adapter>>>,
    /// Active adapter for discovery and operations (live).
    pub primary_adapter: Property<Option<Arc<Adapter>>>,
    /// All devices BlueZ knows about, across adapters (live).
    pub devices: Property<Vec<Arc<Device>>>,
    /// Whether Bluetooth is present: an adapter is, or rfkill blocks a
    /// Bluetooth radio (on some machines, a block takes the adapter away
    /// until it is lifted).
    pub available: Property<bool>,
    /// Whether Bluetooth is on, or turning on, as KDE reckons it: no Bluetooth
    /// radio is blocked (see [`radio_block`](Self::radio_block)), and an
    /// adapter is on or turning on. Follows a power request (from any BlueZ
    /// client) as soon as BlueZ accepts it, and reverts if it fails; an
    /// adapter starting to turn off no longer counts.
    pub enabled: Property<bool>,
    /// Whether Bluetooth is on, as KDE reckons it: no Bluetooth radio is
    /// blocked, and an adapter is on and not turning off (BlueZ connects
    /// devices and discovers only through such an adapter). It turns true
    /// once a power-on is complete, and false as soon as a radio is blocked
    /// or BlueZ accepts a power-off (as [`enabled`](Self::enabled) does).
    pub powered: Property<bool>,
    /// Whether the primary adapter is discovering devices, for any client.
    /// Every client sees the devices a discovery finds.
    pub discovering: Property<bool>,
    /// Currently connected devices (live).
    pub connected: Property<Vec<Arc<Device>>>,

    /// The pairing request awaiting an answer, or else a passkey or PIN to
    /// display for a pairing under way. BlueZ asks this service's pairing
    /// agent, which is the default agent while the service runs.
    pub pairing_request: Property<Option<PairingRequest>>,

    /// Whether the kernel's radio kill switches (rfkill) block a Bluetooth
    /// radio, which turns Bluetooth off as KDE reckons it, and whether
    /// software can lift that. Set by [`disable`](Self::disable), and by any
    /// other rfkill client (e.g. `rfkill block bluetooth`, or a hardware
    /// switch).
    pub radio_block: Property<RadioBlock>,
}

impl BluetoothService {
    /// Creates a new Bluetooth service instance.
    ///
    /// Connects to the system bus, D-Bus-activates bluetoothd if it isn't
    /// running, and enumerates every adapter and device in a single round
    /// trip. The returned service is fully populated; a background dispatcher
    /// keeps it in sync and registers the pairing agent, including across
    /// bluetoothd restarts.
    ///
    /// If bluetoothd is not running and cannot be activated, the service starts
    /// empty (`available` is false) and fills in once bluetoothd appears.
    ///
    /// Must be called within a Tokio runtime, which runs the dispatcher; the
    /// service's actions can be called from any thread.
    ///
    /// # Errors
    /// Returns error if the system bus connection fails.
    pub async fn new() -> Result<Self, Error> {
        let Handle {
            published,
            commands,
            cancellation_token,
        } = dispatcher::start().await?;
        let Published {
            adapters,
            primary_adapter,
            devices,
            available,
            enabled,
            powered,
            discovering,
            connected,
            pairing_request,
            radio_block,
        } = published;

        Ok(Self {
            commands,
            cancellation_token,
            adapters,
            primary_adapter,
            devices,
            available,
            enabled,
            powered,
            discovering,
            connected,
            pairing_request,
            radio_block,
        })
    }

    /// The live device at `path`, if BlueZ currently exports one.
    pub fn device(&self, path: &ObjectPath<'_>) -> Option<Arc<Device>> {
        self.devices
            .get()
            .into_iter()
            .find(|device| device.object_path.as_str() == path.as_str())
    }

    /// The live adapter at `path`, if BlueZ currently exports one.
    pub fn adapter(&self, path: &ObjectPath<'_>) -> Option<Arc<Adapter>> {
        self.adapters
            .get()
            .into_iter()
            .find(|adapter| adapter.object_path.as_str() == path.as_str())
    }

    /// Turns Bluetooth on, as KDE's Bluetooth switch does: lifts the rfkill
    /// soft block on every Bluetooth radio, and powers every adapter on. (An
    /// adapter BlueZ reports blocked is powered on by BlueZ itself once
    /// unblocked, so only the others are powered through BlueZ.)
    ///
    /// A hardware block can't be lifted (see
    /// [`radio_block`](Self::radio_block)). Without rfkill (`/dev/rfkill`
    /// can't be written), this powers every adapter on through BlueZ, which
    /// fails while one is blocked.
    ///
    /// The outcome shows up in [`enabled`](Self::enabled) and
    /// [`powered`](Self::powered); a failure is recorded in the adapters'
    /// `last_error`.
    pub fn enable(&self) {
        self.commands.send(Command::SetEnabled(true));
    }

    /// Turns Bluetooth off, as KDE's Bluetooth switch does: sets the rfkill
    /// soft block on every Bluetooth radio, which powers every adapter off.
    ///
    /// The kernel disconnects every device and powers the adapters down, even
    /// one whose firmware fails the orderly power-off. (KDE also asks BlueZ to
    /// power each adapter off. The block already does, and a power-off
    /// through BlueZ racing it can fail at misbehaving firmware first, so this
    /// doesn't.) The block holds until
    /// it is lifted ([`enable`](Self::enable), or another rfkill client): BlueZ
    /// won't power a blocked adapter on, and `systemd-rfkill` restores the
    /// block at boot. Without rfkill (`/dev/rfkill` can't be written), this
    /// powers every adapter off through BlueZ, which any BlueZ client can
    /// undo.
    ///
    /// The outcome shows up in [`enabled`](Self::enabled),
    /// [`powered`](Self::powered) and [`radio_block`](Self::radio_block); a
    /// failure is recorded in the adapters' `last_error`.
    pub fn disable(&self) {
        self.commands.send(Command::SetEnabled(false));
    }

    /// Starts device discovery on the primary adapter.
    ///
    /// Discovery continues until [`stop_discovery`](Self::stop_discovery) is
    /// called, the adapter powers off (BlueZ then ends it), or this service
    /// goes away. BlueZ shares one discovery procedure between clients, and
    /// every client sees the devices it finds; [`discovering`](Self::discovering)
    /// reports it.
    ///
    /// A BlueZ bug can end the discovery early: if another client is stopping
    /// its own discovery just then, BlueZ accepts the start but stops
    /// discovering anyway. Calling this again does nothing until the session
    /// is stopped ([`stop_discovery`](Self::stop_discovery), or the timeout of
    /// a timed discovery).
    pub fn start_discovery(&self) {
        self.commands
            .send(Command::StartDiscovery { timeout: None });
    }

    /// Starts device discovery on the primary adapter for a limited time.
    ///
    /// BlueZ has no timed discovery, so this starts discovery and stops it
    /// `duration` after the most recent call: calling it again while
    /// discovering restarts the timeout. A `duration` of 30 years or more
    /// means no timeout. As with [`start_discovery`](Self::start_discovery),
    /// a BlueZ bug can end the discovery early.
    pub fn start_timed_discovery(&self, duration: Duration) {
        self.commands.send(Command::StartDiscovery {
            timeout: Some(duration),
        });
    }

    /// Stops every device discovery this service started, on any adapter.
    ///
    /// BlueZ shares one discovery procedure between clients, so an adapter
    /// keeps discovering while another client still is.
    pub fn stop_discovery(&self) {
        self.commands.send(Command::StopDiscovery);
    }

    /// Provides a PIN code for legacy device pairing.
    ///
    /// Responds to `PairingRequest::RequestPinCode`. PIN must be 1-16
    /// alphanumeric characters. Ignored (and logged) if no such request is
    /// pending.
    pub fn provide_pin(&self, pin: String) {
        self.respond(PairingResponse::Pin(pin));
    }

    /// Provides a numeric passkey for device pairing.
    ///
    /// Responds to `PairingRequest::RequestPasskey`. Passkey must be between
    /// 0-999999. Ignored (and logged) if no such request is pending.
    pub fn provide_passkey(&self, passkey: u32) {
        self.respond(PairingResponse::Passkey(passkey));
    }

    /// Provides confirmation for passkey matching.
    ///
    /// Responds to `PairingRequest::RequestConfirmation`: whether the displayed
    /// passkey matches the remote device. Rejecting may also disconnect the
    /// device (see [`cancel_pending_request`](Self::cancel_pending_request)).
    /// Ignored (and logged) if no such request is pending.
    pub fn provide_confirmation(&self, confirmation: bool) {
        self.respond(PairingResponse::Confirmation(confirmation));
    }

    /// Provides authorization for device pairing.
    ///
    /// Responds to `PairingRequest::RequestAuthorization`. Rejecting may also
    /// disconnect the device (see
    /// [`cancel_pending_request`](Self::cancel_pending_request)). Ignored (and
    /// logged) if no such request is pending.
    pub fn provide_authorization(&self, authorization: bool) {
        self.respond(PairingResponse::Authorization(authorization));
    }

    /// Provides authorization for specific Bluetooth service access.
    ///
    /// Responds to `PairingRequest::RequestServiceAuthorization`. Rejecting
    /// turns down only that service; the device stays connected. Ignored (and
    /// logged) if no such request is pending.
    pub fn provide_service_authorization(&self, authorization: bool) {
        self.respond(PairingResponse::ServiceAuthorization(authorization));
    }

    /// Cancels the pending pairing request: rejects a request awaiting an
    /// answer, or stops displaying a passkey or PIN.
    ///
    /// If a connect or pair this service sent for the device still awaits
    /// BlueZ's answer, this also disconnects the device, which aborts the
    /// pairing: BlueZ has no other way to stop a pairing a connect started,
    /// or one that is only displaying a passkey. That connect then ends as
    /// cancelled rather than failed. Any other pairing is only rejected (a
    /// display is only hidden, and BlueZ may show it again): one the device
    /// started, or one following an LE connect, which BlueZ answers before
    /// pairing. A service authorization is only rejected.
    pub fn cancel_pending_request(&self) {
        self.respond(PairingResponse::Cancel);
    }

    fn respond(&self, response: PairingResponse) {
        self.commands.send(Command::Pairing(response));
    }
}

impl Drop for BluetoothService {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
    }
}
