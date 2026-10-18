//! Bluetooth LE for integrations that talk to a device themselves, such as a
//! scale. Only built with the `bluetooth` feature.
//!
//! On Linux this goes through BlueZ on the host, over the system D-Bus; in
//! Docker that is `-v /run/dbus:/run/dbus:ro`. BlueZ itself never runs in
//! the container.
//!
//! What an integration does with it:
//!
//! 1. [`Bluetooth::shared`]: the adapter, opened once for the whole process.
//!    Every opening holds its own D-Bus connection, and the system bus limits
//!    how many one user may have, so integrations share this one. After
//!    repeated adapter failures, [`Bluetooth::close`] lets the next
//!    [`Bluetooth::shared`] open it again.
//! 2. [`Bluetooth::find`]: scans until a device matches. One search runs at a
//!    time, and the scan is always stopped afterwards: BlueZ refuses the next
//!    start while one still runs. A device that is not found (switched off,
//!    the normal case) is a [`NotFound`] error, so integrations can tell it
//!    from a failing adapter with `e.is::<NotFound>()`.
//! 3. [`Bluetooth::connect`]: connects, cancelling attempts that hang or that
//!    BlueZ still has pending, and lists the device's characteristics.
//! 4. [`Device`]: subscribe, read notifications, write, disconnect.
//!
//! The device's own protocol (which characteristic, what the bytes mean)
//! stays in the integration's folder, as plain functions with tests.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use btleplug::api::{
    Central as _, CentralState, Characteristic, Manager as _, Peripheral as _, ScanFilter,
    ValueNotification, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures_util::{Stream, StreamExt};
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

/// How often a running search looks at what the scan has seen.
const SCAN_POLL: Duration = Duration::from_millis(500);
/// A connect that takes longer is cancelled.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Connects tried while BlueZ answers that one is still in progress.
const CONNECT_TRIES: u32 = 3;

/// The process-wide adapter. See [`Bluetooth::shared`].
static SHARED: Mutex<Option<Arc<Bluetooth>>> = Mutex::const_new(None);

/// A Bluetooth adapter, shared by every integration that uses one.
pub struct Bluetooth {
    // Holds the D-Bus connection the adapter works through.
    _manager: Manager,
    adapter: Adapter,
    /// What the system calls the adapter, e.g. `hci0 (usb:v0BDAp8771d0001)`.
    info: String,
    /// One search at a time, so one integration's search does not stop
    /// another's scan.
    searching: Mutex<()>,
}

/// A device seen by a scan.
#[derive(Debug, Clone)]
pub struct Seen {
    /// `AA:BB:CC:DD:EE:FF`.
    pub address: String,
    /// The name it advertises; empty when it sends none.
    pub name: String,
    /// Signal strength in dBm, when known.
    pub rssi: Option<i16>,
    /// Services it advertises (not every device lists all of them).
    pub services: Vec<Uuid>,
    peripheral: Peripheral,
}

impl Seen {
    /// `name · address`, or the address alone, for status lines and logs.
    pub fn label(&self) -> String {
        label(&self.name, &self.address)
    }

    /// Whether this is the device at `address`, in any letter case.
    pub fn has_address(&self, address: &str) -> bool {
        self.address.eq_ignore_ascii_case(address.trim())
    }
}

/// [`Bluetooth::find`] saw no matching device: it is switched off or out of
/// range. Normal for a scale between uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotFound;

impl fmt::Display for NotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not found (switched on and in range?)")
    }
}

impl std::error::Error for NotFound {}

impl Bluetooth {
    /// The adapter, opened on first use and then shared. Fails while there
    /// is no adapter or no BlueZ to reach; try again later.
    pub async fn shared() -> anyhow::Result<Arc<Self>> {
        let mut slot = SHARED.lock().await;
        if let Some(bt) = &*slot {
            return Ok(bt.clone());
        }
        let bt = Arc::new(Self::open().await?);
        tracing::info!("Bluetooth: using adapter {}", bt.info);
        *slot = Some(bt.clone());
        Ok(bt)
    }

    /// Gives up on this adapter after it kept failing (unplugged, BlueZ
    /// restarted): the next [`Bluetooth::shared`] opens it again. Its D-Bus
    /// connection closes once every integration has let go of it.
    pub async fn close(self: &Arc<Self>) {
        let mut slot = SHARED.lock().await;
        if slot.as_ref().is_some_and(|bt| Arc::ptr_eq(bt, self)) {
            tracing::info!("Bluetooth: closing adapter {}", self.info);
            *slot = None;
        }
    }

    async fn open() -> anyhow::Result<Self> {
        let manager = Manager::new()
            .await
            .context("cannot reach Bluetooth (BlueZ). In Docker, add -v /run/dbus:/run/dbus:ro")?;
        let adapter = manager
            .adapters()
            .await
            .map_err(explain)
            .context("cannot list Bluetooth adapters")?
            .into_iter()
            .next()
            .context("no Bluetooth adapter found (is one plugged in?)")?;
        if let Ok(CentralState::PoweredOff) = adapter.adapter_state().await {
            bail!("the Bluetooth adapter is switched off");
        }
        let info = adapter
            .adapter_info()
            .await
            .unwrap_or_else(|_| "(unknown)".to_owned());
        Ok(Self {
            _manager: manager,
            adapter,
            info,
            searching: Mutex::new(()),
        })
    }

    /// What the system calls the adapter.
    pub fn info(&self) -> &str {
        &self.info
    }

    /// Scans for `scan_for` and returns every device seen, strongest first.
    pub async fn scan(&self, scan_for: Duration) -> anyhow::Result<Vec<Seen>> {
        let _one = self.searching.lock().await;
        self.start_scan().await?;
        sleep(scan_for).await;
        let seen = self.seen().await;
        let _ = self.adapter.stop_scan().await;
        let mut seen = seen?;
        seen.sort_by_key(|s| std::cmp::Reverse(s.rssi.unwrap_or(i16::MIN)));
        Ok(seen)
    }

    /// Scans for up to `scan_for` until a device matches `wanted`. Fails
    /// with [`NotFound`] when none did.
    pub async fn find(
        &self,
        scan_for: Duration,
        wanted: impl Fn(&Seen) -> bool,
    ) -> anyhow::Result<Seen> {
        let _one = self.searching.lock().await;
        self.start_scan().await?;
        // Whatever happens while searching, the scan is stopped again.
        let found = self.search(scan_for, wanted).await;
        let _ = self.adapter.stop_scan().await;
        found
    }

    async fn search(
        &self,
        scan_for: Duration,
        wanted: impl Fn(&Seen) -> bool,
    ) -> anyhow::Result<Seen> {
        let deadline = Instant::now() + scan_for;
        loop {
            if let Some(found) = self.seen().await?.into_iter().find(|s| wanted(s)) {
                return Ok(found);
            }
            if Instant::now() >= deadline {
                return Err(NotFound.into());
            }
            sleep(SCAN_POLL).await;
        }
    }

    /// Starts a scan; one already running (another program's) is fine too.
    async fn start_scan(&self) -> anyhow::Result<()> {
        // No service filter: not every device advertises its services.
        match self.adapter.start_scan(ScanFilter::default()).await {
            Ok(()) => Ok(()),
            Err(e) if in_progress(&e) => {
                tracing::debug!("Bluetooth: a scan is already running ({e})");
                Ok(())
            }
            Err(e) => Err(explain(e)).context("cannot start a Bluetooth scan"),
        }
    }

    /// What the running scan has seen so far.
    async fn seen(&self) -> anyhow::Result<Vec<Seen>> {
        let peripherals = self
            .adapter
            .peripherals()
            .await
            .context("cannot list Bluetooth devices")?;
        let mut seen = Vec::with_capacity(peripherals.len());
        for p in peripherals {
            // A device can vanish between listing and asking (BlueZ drops
            // stale ones during a scan): skip it.
            let Ok(Some(props)) = p.properties().await else {
                continue;
            };
            seen.push(Seen {
                address: p.address().to_string(),
                name: props.local_name.unwrap_or_default(),
                rssi: props.rssi,
                services: props.services,
                peripheral: p,
            });
        }
        Ok(seen)
    }

    /// Connects to a device [`Bluetooth::find`] returned and lists its
    /// characteristics.
    pub async fn connect(&self, seen: Seen) -> anyhow::Result<Device> {
        let label = seen.label();
        let peripheral = seen.peripheral;
        open_link(&peripheral)
            .await
            .with_context(|| format!("cannot connect to {label}"))?;
        match set_up(&peripheral).await {
            Ok((characteristics, notifications)) => Ok(Device {
                peripheral,
                characteristics,
                notifications,
                label,
            }),
            Err(e) => {
                // Half set up: let go of it so the next attempt starts clean.
                let _ = peripheral.disconnect().await;
                Err(e.context(format!("cannot set up {label}")))
            }
        }
    }
}

/// Connects. BlueZ answers "in progress" while an earlier attempt is still
/// pending (one that timed out, or from before a restart): that one is
/// cancelled and the connect tried again. A connect that hangs is cancelled
/// too, or BlueZ answers "in progress" to the next one.
async fn open_link(peripheral: &Peripheral) -> anyhow::Result<()> {
    for attempt in 1..=CONNECT_TRIES {
        match timeout(CONNECT_TIMEOUT, peripheral.connect()).await {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(e)) if in_progress(&e) => {
                // Maybe the pending attempt is about to succeed.
                sleep(Duration::from_secs(1)).await;
                if peripheral.is_connected().await.unwrap_or(false) {
                    return Ok(());
                }
                tracing::debug!(
                    "Bluetooth: a connection is still pending, cancelling it \
                     (try {attempt} of {CONNECT_TRIES})"
                );
                let _ = peripheral.disconnect().await;
                sleep(Duration::from_secs(1)).await;
            }
            Ok(Err(e)) => {
                let _ = peripheral.disconnect().await;
                return Err(e.into());
            }
            Err(_) => {
                let _ = peripheral.disconnect().await;
                bail!("no answer within {} s", CONNECT_TIMEOUT.as_secs());
            }
        }
    }
    bail!("still in progress after {CONNECT_TRIES} tries")
}

type Notifications = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

async fn set_up(peripheral: &Peripheral) -> anyhow::Result<(Vec<Characteristic>, Notifications)> {
    peripheral
        .discover_services()
        .await
        .context("cannot list its services")?;
    let characteristics = peripheral.characteristics().into_iter().collect();
    let notifications = peripheral.notifications().await?;
    Ok((characteristics, notifications))
}

/// A connected device.
///
/// Everything takes `&mut self`: the notification stream is `Send` but not
/// `Sync`, so a shared borrow held across an await would make the
/// integration's task not `Send`.
pub struct Device {
    peripheral: Peripheral,
    characteristics: Vec<Characteristic>,
    notifications: Notifications,
    label: String,
}

impl Device {
    /// `name · address`, for status lines and logs.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether the device has the characteristic `uuid`.
    pub fn has(&self, uuid: Uuid) -> bool {
        self.characteristic(uuid).is_ok()
    }

    fn characteristic(&self, uuid: Uuid) -> anyhow::Result<&Characteristic> {
        self.characteristics
            .iter()
            .find(|c| c.uuid == uuid)
            .with_context(|| format!("{} has no characteristic {uuid}", self.label))
    }

    /// Asks for notifications of `uuid`; they arrive through
    /// [`Device::next_notification`].
    pub async fn subscribe(&mut self, uuid: Uuid) -> anyhow::Result<()> {
        let c = self.characteristic(uuid)?.clone();
        self.peripheral
            .subscribe(&c)
            .await
            .with_context(|| format!("cannot subscribe to {uuid}"))
    }

    /// Writes `value` to `uuid`, with or without asking for a response, as
    /// the device requires.
    pub async fn write(
        &mut self,
        uuid: Uuid,
        value: &[u8],
        with_response: bool,
    ) -> anyhow::Result<()> {
        let c = self.characteristic(uuid)?.clone();
        let kind = if with_response {
            WriteType::WithResponse
        } else {
            WriteType::WithoutResponse
        };
        self.peripheral
            .write(&c, value, kind)
            .await
            .with_context(|| format!("cannot write to {uuid}"))
    }

    /// The next notification: which characteristic, and its value. Waits
    /// through silence as long as the device stays connected (many only
    /// send on change), checking every `silence`; fails once it is gone.
    /// Cancel-safe.
    pub async fn next_notification(
        &mut self,
        silence: Duration,
    ) -> anyhow::Result<(Uuid, Vec<u8>)> {
        loop {
            match timeout(silence, self.notifications.next()).await {
                Ok(Some(n)) => {
                    tracing::trace!("Bluetooth: {} {} {:02X?}", self.label, n.uuid, n.value);
                    return Ok((n.uuid, n.value));
                }
                Ok(None) => bail!("{} disconnected", self.label),
                Err(_) => {
                    if !self.peripheral.is_connected().await? {
                        bail!("{} disconnected (switched off or out of range)", self.label);
                    }
                }
            }
        }
    }

    /// Disconnects. Errors are left out: the device may be gone already.
    pub async fn disconnect(self) {
        let _ = self.peripheral.disconnect().await;
    }
}

/// `name · address`, or the address alone.
fn label(name: &str, address: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        address.to_owned()
    } else {
        format!("{name} · {address}")
    }
}

/// Adds what to do when the system bus refused this user: the host's D-Bus
/// policy decides who may use BlueZ, by the host's users and groups.
fn explain(e: btleplug::Error) -> anyhow::Error {
    let text = e.to_string().to_lowercase();
    if matches!(e, btleplug::Error::PermissionDenied)
        || text.contains("accessdenied")
        || text.contains("not allowed")
    {
        anyhow::Error::from(e).context(
            "BlueZ refused this user; allow it in the host's D-Bus policy for              org.bluez, or run as root",
        )
    } else {
        e.into()
    }
}

/// Whether BlueZ refused because the same operation is still running.
fn in_progress(e: &btleplug::Error) -> bool {
    e.to_string().to_lowercase().contains("in progress")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_show_the_name_when_there_is_one() {
        assert_eq!(label("CFS-9002", "AA:BB"), "CFS-9002 · AA:BB");
        assert_eq!(label("  ", "AA:BB"), "AA:BB");
    }

    #[test]
    fn not_found_is_told_apart_from_other_errors() {
        let e = anyhow::Error::from(NotFound).context("looking for the scale");
        assert!(e.is::<NotFound>());
        assert_eq!(
            format!("{e:#}"),
            "looking for the scale: not found (switched on and in range?)"
        );
        assert!(!anyhow::anyhow!("adapter gone").is::<NotFound>());
    }

    #[test]
    fn in_progress_is_recognised() {
        let other = |s: &str| btleplug::Error::Other(s.into());
        assert!(in_progress(&other(
            "org.bluez.Error.InProgress: Operation already in progress"
        )));
        assert!(!in_progress(&btleplug::Error::NotConnected));

        let refused = explain(other(
            "org.freedesktop.DBus.Error.AccessDenied: Rejected send",
        ));
        assert!(format!("{refused:#}").starts_with("BlueZ refused this user"));
        let plain = explain(btleplug::Error::NotConnected);
        assert_eq!(format!("{plain:#}"), "Not connected");
    }
}
