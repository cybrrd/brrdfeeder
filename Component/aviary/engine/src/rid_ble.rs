//! Passive HCI user-channel source: raw AD only, never ODID decoding.
//! ADR 0005: dedicated controller must already be DOWN and unmanaged.
//! No power-down, advertising, connection, pairing or subprocess commands here.
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use cybrrd_rid_protocol::ble::{BleMeta, BlePhy};
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;

use crate::audit::{SubstrateAuditEvent, KITTLER_LINEAGE};
use crate::node_config::{BleAdapterYaml, RidBleYaml};
use crate::sensor::{now_unix_ms, Sensor, SensorContext, SensorHandle, SensorHealth, SensorState};

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
            if !self.0.enabled {
                println!("[rid_ble] disabled");
                return;
            }
            let result = self.run(&ctx, &tx, &task_health).await;
            if let Err(e) = result {
                transition(
                    &ctx,
                    &task_health,
                    SensorState::Failed,
                    Some(e.to_string()),
                    false,
                );
            }
        });
        SensorHandle { readings, health }
    }
}

impl RidBle {
    async fn run(
        &self,
        ctx: &SensorContext,
        tx: &mpsc::Sender<Advertisement>,
        health: &Arc<ArcSwap<SensorHealth>>,
    ) -> io::Result<()> {
        self.0.validate().map_err(io::Error::other)?;
        let setup = async {
            let (index, address, usb_id) = resolve_adapter(&self.0.adapter).await?;
            let mut inventory = crate::status::BleInventory {
                bd_addr: address
                    .iter()
                    .rev()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(":"),
                usb_id,
                observed_at: crate::status::utc_now(),
                rfkill: None,
                health: Some(Arc::clone(health)),
                rfkill_observer: crate::rfkill::RfkillObserver::for_controller(index).ok().flatten(),
                ..Default::default()
            };
            self.1.store(Some(Arc::new(inventory.clone())));
            let prepared = crate::rfkill::prepare_controller(index, self.0.unblock_rfkill, &mut |soft, hard| {
                inventory.rfkill = Some(crate::status::RfkillSnapshot {
                    soft_blocked: soft,
                    hard_blocked: hard,
                    observed_at: crate::status::utc_now(),
                });
                self.1.store(Some(Arc::new(inventory.clone())));
            });
            // Observe the result, not the fact that an unblock was requested.
            // StatusSource repeats this read on every snapshot/write, even if
            // setup fails or capture is quiet.
            inventory.refresh();
            self.1.store(Some(Arc::new(inventory)));
            prepared?;
            // USER bind refuses UP/busy controllers. Never evict bluetoothd.
            let socket = HciSocket::open(index, 1)?;
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
            if coded && !extended {
                return Err(io::Error::other("Coded RX without extended scanning"));
            }
            println!(
                "[rid_ble] hci{index} detected PHY=1M{}; exclusive passive scan",
                if coded { "+Coded" } else { "" }
            );
            socket.command(0x0c01, &[0xff; 8]).await?; // Event mask includes LE Meta.
            socket
                .command(
                    0x2001,
                    &[2, if extended { 0x10 } else { 0 }, 0, 0, 0, 0, 0, 0],
                )
                .await?;
            let (opcode, params) = scan_parameters(extended, coded);
            socket.command(opcode, &params).await?;
            let (enable_op, enable) = scan_enable(extended, true);
            socket.command(enable_op, &enable).await?;
            Ok::<_, io::Error>((socket, extended))
        };
        let (socket, extended) = tokio::select! {
            _ = ctx.cancel.cancelled() => return Ok(()),
            result = setup => result?,
        };
        let mut last_ad = Instant::now();
        let quiet = Duration::from_secs(self.0.quiet_window_s);
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                _ = ctx.cancel.cancelled() => break,
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
                    let (seen, reports) = advertising_reports(&packet)?;
                    if seen > 0 {
                        self.2.record(seen as u64);
                        last_ad = Instant::now();
                        transition(ctx, health, SensorState::Healthy, None, true);
                    }
                    for report in reports {
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
    let changed = h.state != state;
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

/// Return ALL advertising-report count for health, plus eligible complete ADs.
/// Fragmented/truncated extended data is refused (no guessed reassembly).
fn advertising_reports(packet: &[u8]) -> io::Result<(usize, Vec<Advertisement>)> {
    let mut out = Vec::new();
    if packet.get(..2) != Some(&[4, 0x3e]) {
        return Ok((0, out));
    }
    if packet.len() < 5 || usize::from(packet[2]) + 3 != packet.len() {
        return Err(io::Error::other("malformed LE event length"));
    }
    let kind = packet[3];
    if kind != 2 && kind != 0x0d {
        return Ok((0, out));
    }
    let count = usize::from(packet[4]);
    let mut cursor = 5;
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
            let coded = h[10] == 3;
            (
                &h[3..9],
                h[13] as i8,
                BleMeta {
                    phy: if coded { BlePhy::LeCoded } else { BlePhy::Le1M },
                    extended: !legacy,
                },
                flags & 0x6f == 0 && h[2] <= 1 && ((legacy && h[9] == 1) || (!legacy && coded)),
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
    Ok((count, out))
}

async fn resolve_adapter(identity: &BleAdapterYaml) -> io::Result<(u16, [u8; 6], Option<String>)> {
    let control = HciSocket::open(0xffff, 3)?;
    let mut matches = Vec::new();
    for entry in std::fs::read_dir("/sys/class/bluetooth")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(index) = name
            .to_str()
            .and_then(|n| n.strip_prefix("hci"))
            .and_then(|n| n.parse::<u16>().ok())
        else {
            continue;
        };
        // Read Controller Information only: no management scan or power writes.
        let info = control.controller_info(index).await?;
        let mut addr = [0; 6];
        addr.copy_from_slice(
            info.get(..6)
                .ok_or_else(|| io::Error::other("short controller info"))?,
        );
        let text = addr
            .iter()
            .rev()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        let usb = usb_identity(&entry.path());
        if identity_matches(identity, &text, usb.as_deref()) {
            matches.push((index, addr, usb));
        }
    }
    unique_adapter(matches)
}

fn identity_matches(identity: &BleAdapterYaml, addr: &str, usb: Option<&str>) -> bool {
    match (&identity.bd_addr, &identity.usb_id) {
        (Some(wanted), None) => wanted.eq_ignore_ascii_case(addr),
        (None, Some(wanted)) => usb.is_some_and(|id| wanted.eq_ignore_ascii_case(id)),
        _ => false,
    }
}

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
    let canonical = path.canonicalize().ok()?;
    for ancestor in canonical.ancestors() {
        if let (Ok(vendor), Ok(product)) = (
            std::fs::read_to_string(ancestor.join("idVendor")),
            std::fs::read_to_string(ancestor.join("idProduct")),
        ) {
            return Some(format!("{}:{}", vendor.trim(), product.trim()));
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
    async fn controller_info(&self, index: u16) -> io::Result<Vec<u8>> {
        tokio::time::timeout(Duration::from_secs(2), async {
            let [lo, hi] = index.to_le_bytes();
            self.send(&[4, 0, lo, hi, 0, 0]).await?;
            loop {
                let p = self.recv().await?;
                if p.len() >= 9 && p[2..4] == [lo, hi] && p[6..8] == [4, 0] && p[..2] == [1, 0] {
                    if p[8] != 0 {
                        return Err(io::Error::other(format!(
                            "Read Controller Info status {}",
                            p[8]
                        )));
                    }
                    return Ok(p[9..].to_vec());
                }
            }
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "controller info timeout"))?
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
        let (count, ads) = advertising_reports(&legacy).unwrap();
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
        let (_, ads) = advertising_reports(&ext).unwrap();
        assert_eq!(ads[0].meta.phy, BlePhy::LeCoded);
        ext[5] = 0x20;
        let (count, ads) = advertising_reports(&ext).unwrap();
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
            RidBleYaml::default(),
            Arc::new(arc_swap::ArcSwapOption::empty()),
            Arc::new(crate::status::FrameActivity::default()),
        )
        .start(ctx);
        assert!(disabled.readings.recv().await.is_none());
        assert_eq!(disabled.health.load().state, SensorState::Initializing);
    }
}
