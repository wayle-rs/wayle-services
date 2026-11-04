//! The kernel's Bluetooth radio kill switches (rfkill), through `/dev/rfkill`.
//!
//! Every radio has a switch with two blocks: a soft one, set by software, and
//! a hard one, from a hardware switch or the firmware, which software can only
//! read. The kernel keeps a blocked radio off: it powers the controller down,
//! and BlueZ refuses to power it on until it is unblocked.
//!
//! Turning Bluetooth off with a soft block is what desktops do (this follows
//! KDE; see `Dispatcher::set_enabled`). Unlike
//! BlueZ's `Powered`, which any client can set again (and BlueZ sets itself
//! when it starts), a block holds until lifted, and `systemd-rfkill` restores
//! it at boot. And the kernel shuts the controller down even when its
//! firmware fails the orderly power-off that `Powered` asks for.
//!
//! `/dev/rfkill` reports every switch when opened, then each change. A task
//! reads it and writes the blocks requested, in order. The kernel powers
//! controllers down within the write, which can take seconds with a
//! misbehaving one, so writes run on a blocking thread.

use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    sync::Arc,
};

use tokio::{io::unix::AsyncFd, sync::mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::types::RadioBlock;

const DEVICE: &str = "/dev/rfkill";

/// The size of a `struct rfkill_event`. (Newer kernels append a byte for
/// readers asking for more.)
const EVENT_SIZE: usize = 8;

const TYPE_BLUETOOTH: u8 = 2;

const OP_ADD: u8 = 0;
const OP_DEL: u8 = 1;
const OP_CHANGE: u8 = 2;
const OP_CHANGE_ALL: u8 = 3;

/// An event from `/dev/rfkill`: a switch was added, removed or changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Event {
    index: u32,
    kind: u8,
    op: u8,
    blocks: Blocks,
}

/// A switch's blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Blocks {
    soft: bool,
    hard: bool,
}

impl Event {
    fn parse(bytes: [u8; EVENT_SIZE]) -> Self {
        let [i0, i1, i2, i3, kind, op, soft, hard] = bytes;
        Self {
            index: u32::from_ne_bytes([i0, i1, i2, i3]),
            kind,
            op,
            blocks: Blocks {
                soft: soft != 0,
                hard: hard != 0,
            },
        }
    }
}

/// The request setting (or lifting) the soft block of every Bluetooth switch.
fn block_all(blocked: bool) -> [u8; EVENT_SIZE] {
    [
        0,
        0,
        0,
        0,
        TYPE_BLUETOOTH,
        OP_CHANGE_ALL,
        u8::from(blocked),
        0,
    ]
}

/// What the rfkill task reports.
#[derive(Debug)]
pub(crate) enum Update {
    /// A switch was added, removed or changed.
    Event(Event),
    /// Setting the soft block (`blocked`), or lifting it, failed.
    WriteFailed { blocked: bool, error: io::Error },
}

/// Bluetooth's switches, and the task reading and writing `/dev/rfkill`.
#[derive(Debug, Default)]
pub(crate) struct Rfkill {
    /// Bluetooth's switches, by index.
    switches: HashMap<u32, Blocks>,
    /// The task, while it runs.
    task: Option<Task>,
}

#[derive(Debug)]
struct Task {
    /// Soft blocks to set, if `/dev/rfkill` was opened for writing.
    requests: Option<mpsc::UnboundedSender<bool>>,
    updates: mpsc::UnboundedReceiver<Update>,
}

impl Rfkill {
    /// Opens `/dev/rfkill` (only for reading, if it can't be written), reads
    /// the switches it reports, and starts the task, which ends with
    /// `cancellation`. Without `/dev/rfkill` there are no switches.
    pub(crate) fn open(cancellation: &CancellationToken) -> Self {
        let open = |write| {
            OpenOptions::new()
                .read(true)
                .write(write)
                .custom_flags(libc::O_NONBLOCK)
                .open(DEVICE)
        };

        match open(true) {
            Ok(file) => Self::start(file, true, cancellation),
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => match open(false) {
                Ok(file) => {
                    info!("cannot write {DEVICE}; bluetooth will be powered through bluez");
                    Self::start(file, false, cancellation)
                }
                Err(err) => {
                    warn!(error = %err, "cannot open {DEVICE}; bluetooth will be powered through bluez");
                    Self::default()
                }
            },
            Err(err) => {
                debug!(error = %err, "no {DEVICE}; bluetooth will be powered through bluez");
                Self::default()
            }
        }
    }

    /// Reads the switches `file` reports when opened, then starts the task.
    /// `file` must be non-blocking.
    fn start(file: File, writable: bool, cancellation: &CancellationToken) -> Self {
        let mut rfkill = Self::default();
        loop {
            match read_event(&file) {
                Ok(event) => rfkill.apply(event),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) => {
                    warn!(error = %err, "cannot read rfkill events");
                    return Self::default();
                }
            }
        }

        let file = match AsyncFd::new(Arc::new(file)) {
            Ok(file) => file,
            Err(err) => {
                warn!(error = %err, "cannot watch rfkill events");
                return rfkill;
            }
        };
        let (requests, request_rx) = mpsc::unbounded_channel();
        let (update_tx, updates) = mpsc::unbounded_channel();
        tokio::spawn(
            cancellation
                .clone()
                .run_until_cancelled_owned(run(file, request_rx, update_tx)),
        );
        rfkill.task = Some(Task {
            requests: writable.then_some(requests),
            updates,
        });
        rfkill
    }

    /// Applies an event to the switches.
    pub(crate) fn apply(&mut self, event: Event) {
        if event.kind != TYPE_BLUETOOTH {
            return;
        }
        match event.op {
            OP_ADD | OP_CHANGE => {
                self.switches.insert(event.index, event.blocks);
            }
            OP_DEL => {
                self.switches.remove(&event.index);
            }
            _ => {}
        }
    }

    /// What blocks Bluetooth, as KDE reckons it: any blocked switch counts,
    /// and a hardware block over a software one.
    pub(crate) fn radio_block(&self) -> RadioBlock {
        if self.switches.values().any(|blocks| blocks.hard) {
            RadioBlock::Hardware
        } else if self.switches.values().any(|blocks| blocks.soft) {
            RadioBlock::Software
        } else {
            RadioBlock::None
        }
    }

    /// Whether Bluetooth can be turned off and on through its switches: there
    /// are some, and `/dev/rfkill` can be written.
    pub(crate) fn can_block(&self) -> bool {
        !self.switches.is_empty()
            && self
                .task
                .as_ref()
                .is_some_and(|task| task.requests.is_some())
    }

    /// Sets (or lifts) the soft block of every Bluetooth switch. The outcome
    /// arrives as events, or as [`Update::WriteFailed`].
    pub(crate) fn set_blocked(&self, blocked: bool) {
        if let Some(requests) = self.task.as_ref().and_then(|task| task.requests.as_ref()) {
            let _ = requests.send(blocked);
        }
    }

    /// The task's next update; none once it has ended (it logs why).
    pub(crate) async fn next(&mut self) -> Update {
        if let Some(task) = &mut self.task {
            if let Some(update) = task.updates.recv().await {
                return update;
            }
            self.task = None;
        }
        std::future::pending().await
    }
}

/// Reads `file`'s events and sets the soft blocks requested, one at a time,
/// reporting both. Ends once the dispatcher is gone, or on a read error.
async fn run(
    file: AsyncFd<Arc<File>>,
    mut requests: mpsc::UnboundedReceiver<bool>,
    updates: mpsc::UnboundedSender<Update>,
) {
    // Whether blocks can still be requested (never, if the file is read-only).
    let mut requesting = true;

    loop {
        tokio::select! {
            readable = file.readable() => {
                let mut ready = match readable {
                    Ok(ready) => ready,
                    Err(err) => {
                        warn!(error = %err, "cannot wait for rfkill events");
                        return;
                    }
                };
                loop {
                    let event = match ready.try_io(|file| read_event(file.get_ref())) {
                        Ok(Ok(event)) => event,
                        Ok(Err(err)) => {
                            warn!(error = %err, "cannot read rfkill events");
                            return;
                        }
                        Err(_would_block) => break,
                    };
                    if updates.send(Update::Event(event)).is_err() {
                        return;
                    }
                }
            }
            request = requests.recv(), if requesting => {
                let Some(blocked) = request else {
                    requesting = false;
                    continue;
                };
                let file = Arc::clone(file.get_ref());
                let written = tokio::task::spawn_blocking(move || {
                    (&*file).write_all(&block_all(blocked))
                })
                .await
                .unwrap_or_else(|err| Err(io::Error::other(err)));
                if let Err(error) = written
                    && updates.send(Update::WriteFailed { blocked, error }).is_err()
                {
                    return;
                }
            }
        }
    }
}

/// Reads one event from `file`.
fn read_event(mut file: &File) -> io::Result<Event> {
    let mut bytes = [0; EVENT_SIZE];
    let read = file.read(&mut bytes)?;
    if read != EVENT_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("an rfkill event of {read} bytes"),
        ));
    }
    Ok(Event::parse(bytes))
}

#[cfg(test)]
mod tests {
    use std::os::{fd::OwnedFd, unix::net::UnixDatagram};

    use super::*;

    const TYPE_WLAN: u8 = 1;

    fn event(index: u32, kind: u8, op: u8, soft: bool, hard: bool) -> [u8; EVENT_SIZE] {
        let [i0, i1, i2, i3] = index.to_ne_bytes();
        [i0, i1, i2, i3, kind, op, u8::from(soft), u8::from(hard)]
    }

    fn switches(blocks: &[(bool, bool)]) -> Rfkill {
        let mut rfkill = Rfkill::default();
        for (index, &(soft, hard)) in (0..).zip(blocks) {
            rfkill.apply(Event::parse(event(
                index,
                TYPE_BLUETOOTH,
                OP_ADD,
                soft,
                hard,
            )));
        }
        rfkill
    }

    #[test]
    fn an_event_is_parsed() {
        assert_eq!(
            Event::parse(event(7, TYPE_BLUETOOTH, OP_CHANGE, true, false)),
            Event {
                index: 7,
                kind: TYPE_BLUETOOTH,
                op: OP_CHANGE,
                blocks: Blocks {
                    soft: true,
                    hard: false,
                },
            }
        );
    }

    #[test]
    fn blocking_asks_for_every_bluetooth_switch() {
        assert_eq!(
            block_all(true),
            event(0, TYPE_BLUETOOTH, OP_CHANGE_ALL, true, false)
        );
        assert_eq!(
            block_all(false),
            event(0, TYPE_BLUETOOTH, OP_CHANGE_ALL, false, false)
        );
    }

    #[test]
    fn any_blocked_switch_blocks_bluetooth() {
        let block = |blocks: &[(bool, bool)]| switches(blocks).radio_block();

        assert_eq!(block(&[]), RadioBlock::None);
        assert_eq!(block(&[(false, false)]), RadioBlock::None);
        assert_eq!(block(&[(true, false)]), RadioBlock::Software);
        assert_eq!(block(&[(false, true)]), RadioBlock::Hardware);
        assert_eq!(
            block(&[(true, false), (false, false)]),
            RadioBlock::Software
        );
        assert_eq!(
            block(&[(false, true), (false, false)]),
            RadioBlock::Hardware
        );
        assert_eq!(
            block(&[(false, true), (true, false)]),
            RadioBlock::Hardware,
            "a hardware block over a software one"
        );
    }

    #[test]
    fn switches_follow_their_events() {
        let mut rfkill = switches(&[(false, false)]);

        rfkill.apply(Event::parse(event(
            0,
            TYPE_BLUETOOTH,
            OP_CHANGE,
            true,
            false,
        )));
        assert_eq!(rfkill.radio_block(), RadioBlock::Software);

        rfkill.apply(Event::parse(event(0, TYPE_BLUETOOTH, OP_DEL, true, false)));
        assert_eq!(rfkill.radio_block(), RadioBlock::None);
    }

    #[test]
    fn other_radios_are_ignored() {
        let mut rfkill = Rfkill::default();

        rfkill.apply(Event::parse(event(1, TYPE_WLAN, OP_ADD, true, false)));

        assert_eq!(rfkill.radio_block(), RadioBlock::None);
        assert!(!rfkill.can_block(), "no bluetooth switch");
    }

    /// `/dev/rfkill` as a datagram socket, one event per read or write: the
    /// device's end, and the kernel's.
    fn fake_device() -> (File, tokio::net::UnixDatagram) {
        let (device, kernel) = UnixDatagram::pair().unwrap();
        device.set_nonblocking(true).unwrap();
        kernel.set_nonblocking(true).unwrap();
        (
            File::from(OwnedFd::from(device)),
            tokio::net::UnixDatagram::from_std(kernel).unwrap(),
        )
    }

    #[tokio::test]
    async fn the_task_reports_events_and_writes_blocks() {
        let (device, kernel) = fake_device();
        kernel
            .send(&event(0, TYPE_BLUETOOTH, OP_ADD, false, false))
            .await
            .unwrap();
        let token = CancellationToken::new();

        let mut rfkill = Rfkill::start(device, true, &token);
        assert_eq!(rfkill.radio_block(), RadioBlock::None, "read when opened");
        assert!(rfkill.can_block());

        rfkill.set_blocked(true);
        let mut written = [0; 16];
        let length = kernel.recv(&mut written).await.unwrap();
        assert_eq!(&written[..length], &block_all(true));

        kernel
            .send(&event(0, TYPE_BLUETOOTH, OP_CHANGE, true, false))
            .await
            .unwrap();
        let update = rfkill.next().await;
        assert!(matches!(update, Update::Event(_)), "{update:?}");
        if let Update::Event(event) = update {
            rfkill.apply(event);
        }
        assert_eq!(rfkill.radio_block(), RadioBlock::Software);

        token.cancel();
    }

    #[tokio::test]
    async fn a_read_only_device_blocks_nothing() {
        let (device, kernel) = fake_device();
        kernel
            .send(&event(0, TYPE_BLUETOOTH, OP_ADD, true, false))
            .await
            .unwrap();
        let token = CancellationToken::new();

        let rfkill = Rfkill::start(device, false, &token);

        assert_eq!(rfkill.radio_block(), RadioBlock::Software);
        assert!(!rfkill.can_block());
        token.cancel();
    }
}
