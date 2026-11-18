// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Passive HCI user-channel source: raw AD only, never ODID decoding.
//! ADR 0005: dedicated controller must already be DOWN and unmanaged.
//! No power-down, advertising, connection, pairing or subprocess commands here.
use std::future::Future;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use cybrrd_rid_protocol::ble::{BleMeta, BlePhy};
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;

use crate::audit::{SubstrateAuditEvent, KITTLER_LINEAGE};
use crate::node_config::{BleAdapterYaml, RidBleYaml};
use crate::sensor::{now_unix_ms, Sensor, SensorContext, SensorHandle, SensorHealth, SensorState};

const SUPPORTED_USB: [&str; 3] = ["0bda:876e", "0bda:8771", "0bda:a728"];
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LongRange {
    #[default]
    NotYetObserved,
    Observed,
    Unavailable,
}
static LONG_RANGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
impl LongRange {
    pub fn current() -> Self {
        match LONG_RANGE.load(std::sync::atomic::Ordering::Relaxed) {
            1 => Self::Observed,
            2 => Self::Unavailable,
            _ => Self::NotYetObserved,
        }
    }
    fn publish(self) {
        LONG_RANGE.store(match self { Self::NotYetObserved => 0, Self::Observed => 1, Self::Unavailable => 2 }, std::sync::atomic::Ordering::Relaxed);
    }
    fn observe(&mut self, meta: BleMeta) {
        if meta.phy == BlePhy::LeCoded { *self = Self::Observed; }
    }
}

const RESCAN: Duration = Duration::from_secs(5);
const HCI_SYSFS: &str = "/sys/class/bluetooth";
const OWNERSHIP_CONFLICT: &str = "/run/brrdfeeder-bluetooth/conflict";
const CONFLICT_REASON: &str = "Bluetooth ownership conflict: bluetooth.service was started or unmasked after the RID.BLE claim; leave it under operator control. Set sensors.rid_ble.enabled: false, or explicitly reclaim with sudo /usr/local/libexec/brrdfeeder-bluetooth apply.";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Controller {
    index: u16,
    address: String,
    usb_id: Option<String>,
    path: PathBuf,
    inode: u64,
}

// The supervisor and its real selection logic also run against synthetic sysfs
// trees. Only capture is replaced in hardware-free lifecycle tests.
trait BleRuntime: Send {
    fn select(&self, identity: &BleAdapterYaml) -> io::Result<Controller>;
    fn capture(
        &mut self,
        sensor: &RidBle,
        controller: Controller,
        ctx: &SensorContext,
        tx: &mpsc::Sender<Advertisement>,
        health: &Arc<ArcSwap<SensorHealth>>,
    ) -> impl Future<Output = io::Result<()>> + Send;
}

struct LinuxBle;
impl BleRuntime for LinuxBle {
    fn select(&self, identity: &BleAdapterYaml) -> io::Result<Controller> {
        check_ownership(Path::new(OWNERSHIP_CONFLICT))?;
        select_adapter_with(Path::new(HCI_SYSFS), identity, |index, _| {
            controller_address(index)
        })
    }
    async fn capture(
        &mut self,
        sensor: &RidBle,
        controller: Controller,
        ctx: &SensorContext,
        tx: &mpsc::Sender<Advertisement>,
        health: &Arc<ArcSwap<SensorHealth>>,
    ) -> io::Result<()> {
        sensor.run(controller, ctx, tx, health).await
    }
}

#[derive(Debug)]
pub struct Advertisement {
    pub ad: Vec<u8>,
    pub addr: [u8; 6],
    pub rssi: i32,
    pub meta: BleMeta,
}

pub struct RidBle(
    pub RidBleYaml,
    pub Arc<arc_swap::ArcSwapOption<crate::status::BleInventory>>,
    pub Arc<crate::status::FrameActivity>,
);

impl Sensor for RidBle {
    type Reading = Advertisement;
    fn name(&self) -> &'static str {
        "rid_ble"
    }
    fn start(self, ctx: SensorContext) -> SensorHandle<Advertisement> {
        let (tx, readings) = mpsc::channel(256);
        let health = Arc::new(ArcSwap::from_pointee(SensorHealth::initializing(
            self.name(),
        )));
        let task_health = Arc::clone(&health);
        tokio::spawn(async move {
            self.supervise(&ctx, &tx, &task_health, &mut LinuxBle, RESCAN)
                .await;
        });
        SensorHandle { readings, health }
    }
}

impl RidBle {
    fn inventory(&self, controller: Option<&Controller>, health: &Arc<ArcSwap<SensorHealth>>) {
        LongRange::NotYetObserved.publish();
        self.1.store(Some(Arc::new(crate::status::BleInventory {
            enabled: Some(self.0.enabled),
            bd_addr: controller.map(|c| c.address.clone()).unwrap_or_default(),
            usb_id: controller.and_then(|c| c.usb_id.clone()),
            observed_at: crate::status::utc_now(),
            health: Some(Arc::clone(health)),
            ..Default::default()
        })));
    }

    async fn supervise<R: BleRuntime>(
        &self,
        ctx: &SensorContext,
        tx: &mpsc::Sender<Advertisement>,
        health: &Arc<ArcSwap<SensorHealth>>,
        runtime: &mut R,
        retry: Duration,
    ) {
        self.inventory(None, health);
        if !self.0.enabled {
            transition(
                ctx,
                health,
                SensorState::Initializing,
                Some("Disabled by sensors.rid_ble.enabled: false".into()),
                false,
            );
            println!("[rid_ble] disabled");
            return; // Explicit off never inventories or opens HCI.
        }
        if let Err(error) = self.0.validate() {
            transition(ctx, health, SensorState::Failed, Some(error), false);
            return;
        }
        loop {
            if ctx.cancel.is_cancelled() || tx.is_closed() {
                return;
            }
            let result = match runtime.select(&self.0.adapter) {
                Ok(controller) => {
                    self.inventory(Some(&controller), health);
                    transition(
                        ctx,
                        health,
                        SensorState::Initializing,
                        Some(
                            "Binding the supported Bluetooth adapter for passive reception".into(),
                        ),
                        false,
                    );
                    runtime.capture(self, controller, ctx, tx, health).await
                }
                Err(error) => Err(error),
            };
            let Err(error) = result else {
                return;
            }; // Cancelled/receiver closed.
            if ctx.cancel.is_cancelled() || tx.is_closed() {
                return;
            }
            // Lost hardware/ownership is impaired coverage, never a terminal
            // sensor failure. Clear the old controller and rfkill observation.
            self.inventory(None, health);
            transition(
                ctx,
                health,
                SensorState::Degraded,
                Some(format!("Waiting: {error}; rescanning in 5 seconds")),
                false,
            );
            tokio::select! {
                _ = ctx.cancel.cancelled() => return,
                _ = tx.closed() => return,
                _ = tokio::time::sleep(retry) => {},
            }
        }
    }

    async fn run(
        &self,
        controller: Controller,
        ctx: &SensorContext,
        tx: &mpsc::Sender<Advertisement>,
        health: &Arc<ArcSwap<SensorHealth>>,
    ) -> io::Result<()> {
        self.0.validate().map_err(io::Error::other)?;
        let setup = async {
            let index = controller.index;
            let address = parse_address(&controller.address)?;
            // Recheck selection immediately before opening; never use a stale
            // hci index after unplug/re-enumeration.
            if LinuxBle.select(&self.0.adapter)? != controller {
                return Err(io::Error::other(
                    "Bluetooth adapter changed before exclusive bind",
                ));
            }
            let mut inventory = crate::status::BleInventory {
                enabled: Some(true),
                bd_addr: address
                    .iter()
                    .rev()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(":"),
                usb_id: controller.usb_id.clone(),
                observed_at: crate::status::utc_now(),
                rfkill: None,
                health: Some(Arc::clone(health)),
                rfkill_observer: crate::rfkill::RfkillObserver::for_controller(index)
                    .ok()
                    .flatten(),
                ..Default::default()
            };
            self.1.store(Some(Arc::new(inventory.clone())));
            let prepared = crate::rfkill::prepare_controller(
                index,
                self.0.unblock_rfkill,
                &mut |soft, hard| {
                    inventory.rfkill = Some(crate::status::RfkillSnapshot {
                        soft_blocked: soft,
                        hard_blocked: hard,
                        observed_at: crate::status::utc_now(),
                    });
                    self.1.store(Some(Arc::new(inventory.clone())));
                },
            );
            // Observe the result, not the fact that an unblock was requested.
            // StatusSource repeats this read on every snapshot/write, even if
            // setup fails or capture is quiet.
            inventory.refresh();
            self.1.store(Some(Arc::new(inventory)));
            prepared?;
            // USER bind refuses UP/busy controllers. Never evict bluetoothd.
            let socket = HciSocket::open(index, 1).map_err(|error| {
                io::Error::other(format!("Bluetooth exclusive bind unavailable ({error}); check bluetooth.service ownership and the takeover service; the engine never evicts another owner"))
            })?;
            socket.command(0x0c03, &[]).await?; // Reset owned controller.
            let actual = socket.command(0x1009, &[]).await?; // Read BD_ADDR.
            if actual.get(1..7) != Some(address.as_slice()) {
                return Err(io::Error::other(
                    "adapter identity changed before exclusive bind",
                ));
            }
            let features = socket.command(0x2003, &[]).await?;
            let flags = *features
                .get(2)
                .ok_or_else(|| io::Error::other("short LE features"))?;
            let coded = flags & 0x08 != 0;
            let extended = flags & 0x10 != 0;
            socket.command(0x0c01, &[0xff; 8]).await?; // Event mask includes LE Meta.
            let (extended, range) = configure_scan(&socket, extended, coded).await?;
            range.publish();
            Ok::<_, io::Error>((socket, extended))
        };
        let (socket, extended) = tokio::select! {
            _ = ctx.cancel.cancelled() => return Ok(()),
            result = setup => result?,
        };
        let mut last_ad = Instant::now();
        let quiet = Duration::from_secs(self.0.quiet_window_s);
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        let mut rescan = tokio::time::interval(RESCAN);
        rescan.tick().await;
        transition(
            ctx,
            health,
            SensorState::Initializing,
            Some("Passive Bluetooth scan active; waiting for advertisements".into()),
            false,
        );
        loop {
            tokio::select! {
                _ = ctx.cancel.cancelled() => break,
                _ = tx.closed() => break,
                _ = rescan.tick() => {
                    if LinuxBle.select(&self.0.adapter)? != controller {
                        return Err(io::Error::other("Bluetooth adapter removed or replaced"));
                    }
                }
                _ = ticker.tick() => {
                    if quiet_expired(last_ad.elapsed(), quiet) {
                        transition(ctx, health, SensorState::Degraded, Some("no advertisers".into()), false);
                    }
                }
                packet = socket.recv() => {
                    let packet = packet?;
                    if packet.get(..2) == Some(&[4, 0x10]) {
                        return Err(io::Error::other("HCI hardware error"));
                    }
                    let (seen, reports, coded_report) = advertising_reports(&packet)?;
                    if coded_report { LongRange::Observed.publish(); }
                    if seen > 0 {
                        self.2.record(seen as u64);
                        last_ad = Instant::now();
                        transition(ctx, health, SensorState::Healthy, None, true);
                    }
                    for report in reports {
                        let mut range = LongRange::current();
                        range.observe(report.meta);
                        range.publish();
                        match tx.try_send(report) {
                            Ok(()) => {},
                            Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                let mut h = (**health.load()).clone();
                                h.error_count = h.error_count.saturating_add(1);
                                health.store(Arc::new(h));
                            }
                        }
                    }
                }
            }
        }
        let (opcode, params) = scan_enable(extended, false);
        socket.command(opcode, &params).await?;
        println!("[rid_ble] passive scan stopped");
        Ok(())
    }
}

fn quiet_expired(elapsed: Duration, quiet: Duration) -> bool {
    elapsed >= quiet
}

fn transition(
    ctx: &SensorContext,
    health: &ArcSwap<SensorHealth>,
    state: SensorState,
    detail: Option<String>,
    received: bool,
) {
    let mut h = (**health.load()).clone();
    let changed = h.state != state || h.detail != detail;
    h.state = state;
    h.detail = detail.clone();
    if received {
        h.last_reading_ms = Some(now_unix_ms());
    }
    if changed && state == SensorState::Failed {
        h.error_count = h.error_count.saturating_add(1);
    }
    health.store(Arc::new(h));
    if changed {
        println!("[rid_ble] state={state:?} detail={detail:?}");
        let _ = ctx.substrate_audit.try_send(SubstrateAuditEvent {
            node_id: ctx.node_id.to_string(),
            timestamp_utc: now_unix_ms() as u64,
            lineage: KITTLER_LINEAGE.to_string(),
            event_type: "sensor_rid_ble_state_changed".into(),
            channel: None,
            success: Some(state == SensorState::Healthy),
            error_message: detail,
            elapsed_ms: None,
        });
    }
}

// BlueZ monitor/bt.h: scan type 0 = passive, duplicate filtering OFF.
// Parameters use 0x0060 (60 ms) interval/window, public own address, no filter.
trait ScanCommands {
    fn scan_command(&self, opcode: u16, params: &[u8]) -> impl Future<Output = io::Result<Vec<u8>>> + Send;
}
impl ScanCommands for HciSocket {
    async fn scan_command(&self, opcode: u16, params: &[u8]) -> io::Result<Vec<u8>> {
        self.command(opcode, params).await
    }
}

// Feature bits choose commands, never prove reception. A controller that refuses
// extended setup still gets the unchanged passive legacy scan parameters.
async fn configure_scan(socket: &impl ScanCommands, extended: bool, coded: bool) -> io::Result<(bool, LongRange)> {
    async fn start(socket: &impl ScanCommands, extended: bool, coded: bool) -> io::Result<()> {
        socket.scan_command(0x2001, &[2, if extended { 0x10 } else { 0 }, 0, 0, 0, 0, 0, 0]).await?;
        let (opcode, params) = scan_parameters(extended, coded);
        socket.scan_command(opcode, &params).await?;
        let (opcode, params) = scan_enable(extended, true);
        socket.scan_command(opcode, &params).await?;
        Ok(())
    }
    if extended {
        match start(socket, true, coded).await {
            Ok(()) => return Ok((true, LongRange::NotYetObserved)),
            Err(error) => {
                eprintln!("[rid_ble] extended scan unavailable ({error}); continuing with legacy scan");
                let (opcode, params) = scan_enable(true, false);
                let _ = socket.scan_command(opcode, &params).await;
            }
        }
    }
    start(socket, false, false).await?;
    Ok((false, LongRange::Unavailable))
}

fn scan_parameters(extended: bool, coded: bool) -> (u16, Vec<u8>) {
    if extended {
        let mut p = vec![0, 0, if coded { 5 } else { 1 }];
        p.extend_from_slice(&[0, 0x60, 0, 0x60, 0]);
        if coded {
            p.extend_from_slice(&[0, 0x60, 0, 0x60, 0]);
        }
        (0x2041, p)
    } else {
        (0x200b, vec![0, 0x60, 0, 0x60, 0, 0, 0])
    }
}

fn scan_enable(extended: bool, enabled: bool) -> (u16, Vec<u8>) {
    if extended {
        (0x2042, vec![u8::from(enabled), 0, 0, 0, 0, 0])
    } else {
        (0x200c, vec![u8::from(enabled), 0])
    }
}

// Bluetooth Core v5.4, Vol 4, Part E, 7.7.65.13 (including Table 7.1)
// defines report fields; 7.8.53 forbids connectable+scannable extended events.
// Vol 6, Part B, 2.3.1.5-.6 and 4.4.3.5 constrain primary-only reports and
// anonymous/connectable/scannable combinations. Address_Type 0xff is anonymous,
// not reserved. PHY 4 requires Advertising Coding Selection Host Support,
// which this scanner never enables; it accepts PHY 3 for Coded reception.
fn valid_extended_metadata(h: &[u8]) -> bool {
    let flags = u16::from_le_bytes([h[0], h[1]]);
    let legacy = flags & 0x10 != 0;
    if flags & !0x7f != 0 || flags & 0x60 == 0x60
        || !matches!(h[2], 0..=3 | 0xff)
        || !matches!(h[9], 1 | 3) || !matches!(h[10], 0..=3)
    {
        return false;
    }
    let interval = u16::from_le_bytes([h[14], h[15]]);
    let power_valid = |byte: u8| byte == 0x7f || (-127..=20).contains(&(byte as i8));
    if !matches!(h[11], 0..=0x0f | 0xff)
        || !power_valid(h[12]) || !power_valid(h[13])
        || (interval != 0 && interval < 6)
        || (flags & 4 != 0 && !matches!(h[16], 0..=3 | 0xfe))
    {
        return false;
    }
    if legacy {
        return matches!(flags, 0x10 | 0x12 | 0x13 | 0x15 | 0x1a | 0x1b)
            && h[2] != 0xff && h[9] == 1 && h[10] == 0;
    }
    if flags & 3 == 3 || (flags & 8 != 0 && flags & 2 == 0)
        || (h[2] == 0xff && flags & 3 != 0)
    {
        return false;
    }
    // Without a secondary packet there is no payload, fragmentation or SID,
    // and the primary PDU must supply an address. Directed is still possible.
    if h[10] == 0 && (flags & 0x6b != 0 || h[2] == 0xff
        || h[11] != 0xff || h[23] != 0 || interval != 0)
    {
        return false;
    }
    // Periodic advertising cannot be connectable, scannable or anonymous.
    interval == 0 || (flags & 3 == 0 && h[2] != 0xff)
}

/// Return ALL advertising-report count for health, plus eligible complete ADs.
/// Fragmented/truncated extended data is refused (no guessed reassembly).
fn advertising_reports(packet: &[u8]) -> io::Result<(usize, Vec<Advertisement>, bool)> {
    let mut out = Vec::new();
    if packet.get(..2) != Some(&[4, 0x3e]) {
        return Ok((0, out, false));
    }
    if packet.len() < 5 || usize::from(packet[2]) + 3 != packet.len() {
        return Err(io::Error::other("malformed LE event length"));
    }
    let kind = packet[3];
    if kind != 2 && kind != 0x0d {
        return Ok((0, out, false));
    }
    let count = usize::from(packet[4]);
    let mut cursor = 5;
    let mut coded_observed = false;
    for _ in 0..count {
        let header_len = if kind == 2 { 9 } else { 24 };
        let h = packet
            .get(cursor..cursor + header_len)
            .ok_or_else(|| io::Error::other("short advertising header"))?;
        let data_len = usize::from(h[header_len - 1]);
        let end = cursor + header_len + data_len;
        let ad = packet
            .get(cursor + header_len..end)
            .ok_or_else(|| io::Error::other("short advertising data"))?;
        let (addr_bytes, rssi, meta, eligible) = if kind == 2 {
            let rssi = *packet
                .get(end)
                .ok_or_else(|| io::Error::other("missing RSSI"))? as i8;
            (
                &h[2..8],
                rssi,
                BleMeta {
                    phy: BlePhy::Le1M,
                    extended: false,
                },
                h[0] == 3 && h[1] <= 1,
            )
        } else {
            let flags = u16::from_le_bytes([h[0], h[1]]);
            let legacy = flags & 0x10 != 0;
            let coded = h[9] == 3 || h[10] == 3;
            let valid = valid_extended_metadata(h);
            // Any valid advertiser proves PHY reception, even without RID/RSSI.
            coded_observed |= valid && coded;
            (
                &h[3..9],
                h[13] as i8,
                BleMeta {
                    phy: if coded { BlePhy::LeCoded } else { BlePhy::Le1M },
                    extended: !legacy,
                },
                valid && flags & 0x6f == 0 && h[2] <= 1 && ((legacy && h[9] == 1) || (!legacy && coded)),
            )
        };
        if eligible && rssi != 127 {
            let mut addr = [0; 6];
            addr.copy_from_slice(addr_bytes);
            addr.reverse();
            out.push(Advertisement {
                ad: ad.to_vec(),
                addr,
                rssi: i32::from(rssi),
                meta,
            });
        }
        cursor = end + usize::from(kind == 2);
    }
    if cursor != packet.len() {
        return Err(io::Error::other("trailing advertising bytes"));
    }
    Ok((count, out, coded_observed))
}

fn check_ownership(conflict: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(conflict) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io::Error::other(format!(
            "Bluetooth ownership observation unavailable: {error}"
        ))),
        Ok(_) => Err(io::Error::other(CONFLICT_REASON)),
    }
}

fn parse_address(text: &str) -> io::Result<[u8; 6]> {
    let bytes: Result<Vec<_>, _> = text
        .split(':')
        .map(|part| u8::from_str_radix(part, 16))
        .collect();
    let bytes = bytes.map_err(|_| io::Error::other("Bluetooth controller address is not ready"))?;
    if bytes.len() != 6 || text.split(':').any(|part| part.len() != 2) {
        return Err(io::Error::other(
            "Bluetooth controller address is not ready",
        ));
    }
    let mut address = [0; 6];
    for (dest, source) in address.iter_mut().zip(bytes.into_iter().rev()) {
        *dest = source;
    }
    Ok(address)
}

// HCIGETDEVINFO is a read-only kernel snapshot, including while USER owns HCI.
// Linux does not expose a portable hciN/address sysfs attribute.
fn controller_address(index: u16) -> io::Result<String> {
    let fd = unsafe { libc::socket(libc::AF_BLUETOOTH, libc::SOCK_RAW | libc::SOCK_CLOEXEC, 1) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut info = [0u8; 92];
    info[..2].copy_from_slice(&index.to_ne_bytes());
    if unsafe { libc::ioctl(fd.as_raw_fd(), 0x800448d3, info.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    decode_controller_address(index, &info)
}
fn decode_controller_address(index: u16, info: &[u8; 92]) -> io::Result<String> {
    if u16::from_ne_bytes([info[0], info[1]]) != index || info[10..16] == [0; 6] {
        return Err(io::Error::other(
            "Bluetooth controller address is not ready",
        ));
    }
    Ok(info[10..16]
        .iter()
        .rev()
        .map(|v| format!("{v:02X}"))
        .collect::<Vec<_>>()
        .join(":"))
}
#[cfg(test)]
fn select_adapter_at(root: &Path, identity: &BleAdapterYaml) -> io::Result<Controller> {
    select_adapter_with(root, identity, |_, path| {
        std::fs::read_to_string(path.join("address"))
    })
}
fn select_adapter_with(
    root: &Path,
    identity: &BleAdapterYaml,
    address_of: impl Fn(u16, &Path) -> io::Result<String>,
) -> io::Result<Controller> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut controllers = Vec::new();
    for entry in entries.into_iter().flatten() {
        let entry = entry?;
        let Some(index) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_prefix("hci"))
            .and_then(|name| name.parse::<u16>().ok())
            .filter(|index| *index != u16::MAX)
        else {
            continue;
        };
        let path = entry.path();
        let Ok(metadata) = path.metadata() else {
            continue;
        }; // Unplug during rescan.
        let usb_id = usb_identity(&path);
        let address = if usb_id
            .as_deref()
            .is_some_and(|id| SUPPORTED_USB.contains(&id))
        {
            address_of(index, &path)?.trim().to_uppercase()
        } else {
            String::new()
        };
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        controllers.push(Controller {
            index,
            address,
            usb_id,
            path: canonical,
            inode: metadata.ino(),
        });
    }
    controllers.sort_by_key(|controller| controller.index);
    let supported: Vec<_> = controllers
        .iter()
        .filter(|controller| {
            controller
                .usb_id
                .as_deref()
                .is_some_and(|usb| SUPPORTED_USB.contains(&usb))
        })
        .collect();
    let matches: Vec<_> = supported
        .iter()
        .filter(|controller| {
            identity_matches(identity, &controller.address, controller.usb_id.as_deref())
        })
        .collect();
    if matches.len() > 1 {
        return Err(io::Error::other("Multiple supported Bluetooth adapters match; configure one explicit sensors.rid_ble.adapter.bd_addr or unplug extra adapters"));
    }
    if let Some(controller) = matches.first() {
        parse_address(&controller.address)?;
        return Ok((***controller).clone());
    }
    let present: Vec<_> = controllers
        .iter()
        .map(|controller| {
            format!(
                "hci{} {}",
                controller.index,
                controller.usb_id.as_deref().unwrap_or("unknown USB identity")
            )
        })
        .collect();
    let pin = identity.usb_id.as_ref().or(identity.bd_addr.as_ref());
    if let Some(pin) = pin {
        return Err(io::Error::other(format!(
            "Pinned Bluetooth adapter {pin} is absent; present: {}. Set sensors.rid_ble.adapter: {{}} for automatic selection or update the pin; recommended: {}",
            if present.is_empty() { "none".into() } else { present.join(", ") }, SUPPORTED_USB.join(" / "))));
    }
    if !controllers.is_empty() {
        return Err(io::Error::other(format!(
            "Bluetooth adapter {} is not supported; recommended: {}",
            present.join(", "),
            SUPPORTED_USB.join(" / ")
        )));
    }
    Err(io::Error::other(format!(
        "No supported Bluetooth adapter present; plug in {} to activate automatically",
        SUPPORTED_USB.join(" / ")
    )))
}

fn identity_matches(identity: &BleAdapterYaml, addr: &str, usb: Option<&str>) -> bool {
    match (&identity.bd_addr, &identity.usb_id) {
        (Some(wanted), None) => wanted.eq_ignore_ascii_case(addr),
        (None, Some(wanted)) => usb.is_some_and(|id| wanted.eq_ignore_ascii_case(id)),
        (None, None) => usb.is_some_and(|id| SUPPORTED_USB.contains(&id)),
        _ => false,
    }
}

#[cfg(test)]
fn unique_adapter<T>(matches: Vec<T>) -> io::Result<T> {
    if matches.len() != 1 {
        return Err(io::Error::other(format!(
            "adapter identity matched {} controllers; require exactly one",
            matches.len()
        )));
    }
    matches
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::other("missing adapter"))
}

fn usb_identity(path: &Path) -> Option<String> {
    let canonical = path
        .join("device")
        .canonicalize()
        .or_else(|_| path.canonicalize())
        .ok()?;
    for ancestor in canonical.ancestors() {
        if let (Ok(vendor), Ok(product)) = (
            std::fs::read_to_string(ancestor.join("idVendor")),
            std::fs::read_to_string(ancestor.join("idProduct")),
        ) {
            return Some(format!("{}:{}", vendor.trim(), product.trim()).to_ascii_lowercase());
        }
    }
    None
}

struct HciSocket(AsyncFd<OwnedFd>);
#[repr(C)]
struct HciAddress {
    family: libc::sa_family_t,
    index: u16,
    channel: u16,
}

impl HciSocket {
    fn open(index: u16, channel: u16) -> io::Result<Self> {
        // SAFETY: fixed Linux HCI sockaddr layout; OwnedFd closes on all exits.
        let raw = unsafe {
            libc::socket(
                libc::AF_BLUETOOTH,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                1,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let addr = HciAddress {
            family: libc::AF_BLUETOOTH as libc::sa_family_t,
            index,
            channel,
        };
        let rc = unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&addr as *const HciAddress).cast(),
                std::mem::size_of::<HciAddress>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(AsyncFd::new(fd)?))
    }
    async fn send(&self, bytes: &[u8]) -> io::Result<()> {
        loop {
            let mut ready = self.0.writable().await?;
            match ready.try_io(|fd| {
                let n = unsafe {
                    libc::send(
                        fd.as_raw_fd(),
                        bytes.as_ptr().cast(),
                        bytes.len(),
                        libc::MSG_NOSIGNAL,
                    )
                };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else if n as usize != bytes.len() {
                    Err(io::Error::other("short HCI write"))
                } else {
                    Ok(())
                }
            }) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
    }
    async fn recv(&self) -> io::Result<Vec<u8>> {
        loop {
            let mut ready = self.0.readable().await?;
            match ready.try_io(|fd| {
                let mut bytes = vec![0; 4096];
                let n = unsafe {
                    libc::recv(fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len(), 0)
                };
                if n < 0 {
                    return Err(io::Error::last_os_error());
                }
                if n == 0 {
                    return Err(io::Error::other("HCI socket closed"));
                }
                bytes.truncate(n as usize);
                Ok(bytes)
            }) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
    }
    async fn command(&self, opcode: u16, params: &[u8]) -> io::Result<Vec<u8>> {
        tokio::time::timeout(Duration::from_secs(2), async {
            let [lo, hi] = opcode.to_le_bytes();
            let len = u8::try_from(params.len()).map_err(io::Error::other)?;
            let mut command = vec![1, lo, hi, len];
            command.extend_from_slice(params);
            self.send(&command).await?;
            loop {
                let p = self.recv().await?;
                if p.len() >= 7 && p[..2] == [4, 0x0e] && p[4..6] == [lo, hi] {
                    if p[6] != 0 {
                        return Err(io::Error::other(format!(
                            "HCI opcode {opcode:04x} status {:02x}",
                            p[6]
                        )));
                    }
                    return Ok(p[6..].to_vec());
                }
                if p.len() >= 7 && p[..2] == [4, 0x0f] && p[5..7] == [lo, hi] && p[3] != 0 {
                    return Err(io::Error::other(format!(
                        "HCI opcode {opcode:04x} status {:02x}",
                        p[3]
                    )));
                }
            }
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "HCI command timeout"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_req_brrd_014_passive_scan_parameters() {
        assert_eq!(
            scan_parameters(false, false),
            (0x200b, vec![0, 96, 0, 96, 0, 0, 0])
        );
        assert_eq!(
            scan_parameters(true, false),
            (0x2041, vec![0, 0, 1, 0, 96, 0, 96, 0])
        );
        assert_eq!(
            scan_parameters(true, true),
            (0x2041, vec![0, 0, 5, 0, 96, 0, 96, 0, 0, 96, 0, 96, 0])
        );
        assert_eq!(scan_enable(true, false), (0x2042, vec![0; 6]));
    }
    #[test]
    fn verify_req_brrd_014_identity_never_index_only() {
        let id = BleAdapterYaml {
            bd_addr: Some("00:E0:4C:31:5E:C6".into()),
            usb_id: None,
        };
        assert!(identity_matches(&id, "00:e0:4c:31:5e:c6", None));
        assert!(!identity_matches(&id, "00:E0:4C:31:61:1A", None));
        assert!(unique_adapter::<(u16, [u8; 6])>(vec![]).is_err());
        assert!(unique_adapter(vec![(0, [0; 6]), (1, [0; 6])]).is_err());
        assert_eq!(unique_adapter(vec![(7, [1; 6])]).unwrap().0, 7);
    }
    #[test]
    fn verify_req_brrd_014_ad_reports_preserve_phy_and_refuse_fragments() {
        let legacy = [4, 0x3e, 13, 2, 1, 3, 0, 6, 5, 4, 3, 2, 1, 1, 0, 200];
        let (count, ads, _) = advertising_reports(&legacy).unwrap();
        assert_eq!(count, 1);
        assert_eq!(ads[0].addr, [1, 2, 3, 4, 5, 6]);
        assert_eq!(ads[0].rssi, -56);
        assert!(!ads[0].meta.extended);
        let mut ext = vec![4, 0x3e, 27, 0x0d, 1];
        let mut h = [0; 24];
        h[9] = 1;
        h[10] = 3;
        h[13] = 200;
        h[23] = 1;
        ext.extend_from_slice(&h);
        ext.push(0);
        let (_, ads, _) = advertising_reports(&ext).unwrap();
        assert_eq!(ads[0].meta.phy, BlePhy::LeCoded);
        ext[5] = 0x20;
        let (count, ads, _) = advertising_reports(&ext).unwrap();
        assert_eq!(count, 1);
        assert!(ads.is_empty());
        for n in 2..legacy.len() {
            assert!(advertising_reports(&legacy[..n]).is_err());
        }
    }
    #[tokio::test]
    async fn verify_req_brrd_014_health_and_disabled_no_hardware() {
        let (audit, mut events) = mpsc::channel(8);
        let ctx = SensorContext {
            node_id: Arc::from("test"),
            cancel: tokio_util::sync::CancellationToken::new(),
            substrate_audit: audit,
        };
        let health = ArcSwap::from_pointee(SensorHealth::initializing("rid_ble"));
        transition(&ctx, &health, SensorState::Healthy, None, true);
        assert!(health.load().last_reading_ms.is_some());
        assert!(quiet_expired(
            Duration::from_secs(30),
            Duration::from_secs(30)
        ));
        transition(
            &ctx,
            &health,
            SensorState::Degraded,
            Some("no advertisers".into()),
            false,
        );
        transition(
            &ctx,
            &health,
            SensorState::Failed,
            Some("socket loss".into()),
            false,
        );
        assert_eq!(health.load().error_count, 1);
        for _ in 0..3 {
            assert!(events.try_recv().is_ok());
        }
        let mut disabled = RidBle(
            RidBleYaml {
                enabled: false,
                ..Default::default()
            },
            Arc::new(arc_swap::ArcSwapOption::empty()),
            Arc::new(crate::status::FrameActivity::default()),
        )
        .start(ctx);
        assert!(disabled.readings.recv().await.is_none());
        assert_eq!(disabled.health.load().state, SensorState::Initializing);
    }
}

#[cfg(test)]
#[path = "rid_ble_hotplug_tests.rs"]
mod hotplug_tests;

#[cfg(test)]
mod bluetooth_diagnostics_tests {
    use super::*;
    use std::sync::Mutex;

    fn extended_header() -> [u8; 24] {
        let mut h = [0; 24];
        h[9] = 1;
        h[10] = 3;
        h[11] = 0xff;
        h[12] = 0x7f;
        h[13] = 0x7f;
        h
    }

    fn extended_event(headers: &[[u8; 24]]) -> Vec<u8> {
        let mut event = vec![4, 0x3e, (2 + headers.len() * 24) as u8, 0x0d, headers.len() as u8];
        for h in headers { event.extend(h); }
        event
    }

    #[test]
    fn coded_evidence_rejects_malformed_metadata() {
        let good = extended_header();
        let mut reproducer = good;
        reproducer[0] = 0xff; reproducer[1] = 0xff;
        reproducer[2] = 0xff; reproducer[9] = 0xff;
        let mut cases = vec![("complete malformed reproducer", reproducer)];
        for (name, index, byte) in [
            ("reserved event bit 7", 0, 0x80),
            ("reserved event bit 8", 1, 1),
            ("reserved data status", 0, 0x60),
            ("reserved address", 2, 4),
            ("reserved primary", 9, 0xff),
            ("zero primary", 9, 0),
            ("2M primary", 9, 2),
            ("unnegotiated primary coding selection", 9, 4),
            ("connectable and scannable", 0, 3),
            ("response to non-scannable", 0, 8),
            ("legacy with secondary PHY", 0, 0x10),
            ("reserved SID", 11, 0x10),
            ("invalid TX power", 12, 21),
            ("invalid RSSI", 13, 0x80),
            ("reserved periodic interval", 14, 1),
        ] {
            let mut h = good; h[index] = byte; cases.push((name, h));
        }
        for (name, secondary) in [("reserved secondary", 0xff), ("unnegotiated secondary coding selection", 4)] {
            let mut h = good; h[9] = 3; h[10] = secondary; cases.push((name, h));
        }
        for (name, flags, address, secondary) in [
            ("legacy with Coded primary", 0x10, 0, 0),
            ("legacy reserved combination", 0x11, 0, 0),
            ("legacy incomplete", 0x30, 0, 0),
            ("legacy anonymous", 0x10, 0xff, 0),
            ("anonymous connectable", 1, 0xff, 3),
            ("anonymous scannable", 2, 0xff, 3),
            ("anonymous primary-only", 0, 0xff, 0),
            ("connectable primary-only", 1, 0, 0),
            ("scannable primary-only", 2, 0, 0),
            ("fragmented primary-only", 0x20, 0, 0),
        ] {
            let mut h = good; h[0] = flags; h[2] = address; h[9] = 3; h[10] = secondary;
            cases.push((name, h));
        }
        let mut directed = good; directed[0] = 4; directed[16] = 0xff;
        cases.push(("reserved direct address type", directed));
        let mut periodic = good; periodic[0] = 1; periodic[14] = 6;
        cases.push(("connectable periodic", periodic));
        // Every fixture retains consistent HCI lengths and a Coded byte.
        for (name, h) in &cases {
            let (count, ads, observed) = advertising_reports(&extended_event(&[*h])).unwrap();
            assert_eq!(count, 1, "{name}");
            assert!(!observed, "malformed metadata proved Coded: {name}");
            assert!(ads.is_empty(), "malformed metadata entered RID decode: {name}");
        }
        println!("{} malformed metadata controls", cases.len());
    }

    #[test]
    fn coded_evidence_accepts_valid_metadata_without_rid_or_rssi() {
        let good = extended_header();
        let mut cases = vec![];
        for address in [0, 1, 2, 3, 0xff] {
            for (primary, secondary) in [(1, 3), (3, 1), (3, 2), (3, 3)] {
                let mut h = good; h[2] = address; h[9] = primary; h[10] = secondary;
                cases.push(h);
            }
        }
        for flags in [1, 2, 4, 5, 6, 0x0a, 0x0e, 0x20, 0x40] {
            let mut h = good; h[0] = flags; cases.push(h);
        }
        let mut primary_only = good; primary_only[9] = 3; primary_only[10] = 0;
        cases.push(primary_only);
        let mut anonymous_directed = good; anonymous_directed[0] = 4; anonymous_directed[2] = 0xff;
        cases.push(anonymous_directed);
        let mut periodic = good; periodic[14] = 6; cases.push(periodic);
        for h in &cases {
            let (_, ads, observed) = advertising_reports(&extended_event(&[*h])).unwrap();
            assert!(observed, "valid Coded metadata refused: {h:?}");
            assert!(ads.is_empty()); // no RSSI, no RID, still reception evidence
        }
        for flags in [0x10, 0x12, 0x13, 0x15, 0x1a, 0x1b] {
            let mut h = good; h[0] = flags; h[10] = 0;
            assert!(!advertising_reports(&extended_event(&[h])).unwrap().2);
        }
        for secondary in [0, 1, 2] {
            let mut h = good; h[10] = secondary;
            assert!(!advertising_reports(&extended_event(&[h])).unwrap().2);
        }
        println!("{} valid Coded and 9 non-Coded controls", cases.len());
    }

    #[test]
    fn coded_evidence_mixed_reports_are_independent() {
        let good = extended_header();
        let mut bad = good; bad[1] = 0x80;
        let mut one_m = good; one_m[10] = 1;
        for headers in [[bad, good], [good, bad]] {
            let (count, _, observed) = advertising_reports(&extended_event(&headers)).unwrap();
            assert_eq!(count, 2); assert!(observed);
        }
        for headers in [[bad, one_m], [one_m, bad]] {
            assert!(!advertising_reports(&extended_event(&headers)).unwrap().2);
        }
        // A malformed report cannot enter the secondary promotion path via ads.
        bad[13] = (-40i8) as u8;
        let mut eligible = good; eligible[13] = (-40i8) as u8;
        for headers in [[bad, eligible], [eligible, bad]] {
            let (_, ads, observed) = advertising_reports(&extended_event(&headers)).unwrap();
            assert!(observed); assert_eq!(ads.len(), 1);
            assert_eq!(ads[0].meta.phy, BlePhy::LeCoded);
        }
    }

    struct Fake { fail: Option<u16>, calls: Mutex<Vec<(u16, Vec<u8>)>> }
    impl ScanCommands for Fake {
        async fn scan_command(&self, op: u16, bytes: &[u8]) -> io::Result<Vec<u8>> {
            self.calls.lock().unwrap().push((op, bytes.to_vec()));
            if self.fail == Some(op) { Err(io::Error::other("controller refused")) } else { Ok(vec![0]) }
        }
    }
    #[tokio::test]
    async fn extended_refusals_continue_legacy_capture() {
        for fail in [None, Some(0x2041), Some(0x2042)] {
            let socket = Fake { fail, calls: Mutex::new(Vec::new()) };
            let (extended, range) = configure_scan(&socket, true, true).await.unwrap();
            assert_eq!(extended, fail.is_none());
            assert_eq!(range, if extended { LongRange::NotYetObserved } else { LongRange::Unavailable });
            let calls = socket.calls.lock().unwrap();
            let expected = if extended { scan_enable(true,true) } else { scan_enable(false,true) };
            assert_eq!(calls.last(), Some(&expected));
            if !extended { assert!(calls.contains(&scan_parameters(false,false))); }
        }
        // Misleading Coded feature bit without extended support still captures.
        let socket = Fake { fail: None, calls: Mutex::new(Vec::new()) };
        assert_eq!(configure_scan(&socket, false, true).await.unwrap(), (false, LongRange::Unavailable));
        assert_eq!(socket.calls.lock().unwrap().last(), Some(&scan_enable(false,true)));
        // Loss of the controller is still a real error if legacy also fails.
        let socket = Fake { fail: Some(0x2001), calls: Mutex::new(Vec::new()) };
        assert!(configure_scan(&socket, true, true).await.is_err());
    }
    #[test]
    fn long_range_requires_reports_and_resets() {
        let mut range = LongRange::default();
        assert_eq!(range, LongRange::NotYetObserved);
        range.observe(BleMeta { phy: BlePhy::Le1M, extended: false });
        assert_eq!(range, LongRange::NotYetObserved);
        range.observe(BleMeta { phy: BlePhy::Le1M, extended: true });
        assert_eq!(range, LongRange::NotYetObserved);
        range = LongRange::default();
        range.observe(BleMeta { phy: BlePhy::LeCoded, extended: false });
        assert_eq!(range, LongRange::Observed);
        range = LongRange::Unavailable;
        range.observe(BleMeta { phy: BlePhy::Le1M, extended: false });
        assert_eq!(range, LongRange::Unavailable);
        let inventory = crate::status::BleInventory::default();
        assert_eq!(serde_json::to_value(inventory).unwrap()["long_range"], "not_yet_observed");
        let mut ext = vec![4, 0x3e, 27, 0x0d, 1];
        let mut h = [0u8;24]; h[9]=1; h[10]=1; h[13]=200; h[23]=1;
        ext.extend(h); ext.push(0);
        let (_, ads, observed) = advertising_reports(&ext).unwrap();
        assert!(!observed); assert!(ads.is_empty()); // extended 1M never proves long range
        for (primary,secondary) in [(1,3),(3,1)] {
            ext[14]=primary; ext[15]=secondary;
            assert!(advertising_reports(&ext).unwrap().2);
        }
        let empty = [4, 0x3e, 2, 0x0d, 0];
        assert!(!advertising_reports(&empty).unwrap().2);
        assert!(advertising_reports(&ext[..ext.len()-1]).is_err());
        let sensor = RidBle(RidBleYaml::absent(), Arc::new(arc_swap::ArcSwapOption::empty()), Arc::new(crate::status::FrameActivity::default()));
        let health = Arc::new(ArcSwap::from_pointee(SensorHealth::initializing("rid_ble")));
        LongRange::Observed.publish();
        sensor.inventory(None, &health);
        assert_eq!(LongRange::current(), LongRange::NotYetObserved);
        LongRange::Unavailable.publish();
        sensor.inventory(None, &health);
        assert_eq!(LongRange::current(), LongRange::NotYetObserved);
    }
}
