// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
// Wave 7.1 Inc 8 — Capture Liveness Watchdog + Auto-Heal.
//
// ─────────────────────────────────────────────────────────────────────
//   The 2026-05-13 Alfa cable-bump lesson.
//
//   Cy bumped the Alfa's USB cable while clearing the table for CM5
//   programming. The driver hit a `-71 EPROTO` error storm, the
//   device de-registered, the kernel re-enumerated it — and it came
//   back in `managed` mode (the default), not `monitor` mode, with a
//   NEW ifindex (5 → 6). The engine never noticed. It kept reporting
//   `radio_status: "up"` and the Hunter kept reporting
//   `channel_rotation: true` for ~18 hours while capturing NOTHING.
//
//   The udev Tier-1 work (Wave 7.0) gave us naming-stability — the
//   interface came back as `wlx00c0caa697b3`, not stuck as `wlan1`.
//   But naming-stability is not mode-stability or ifindex-stability.
//
//   This watchdog closes the seam. It is a mini-supervisor scoped to
//   the capture path: the Tier-A/Tier-B recovery the full Wave 7.5
//   brrdsupervisor will later subsume. It witnesses the substrate
//   (is capture actually flowing?) rather than assuming it.
// ─────────────────────────────────────────────────────────────────────
//
// Mechanism:
//   - The capture loop bumps `last_packet_unix_ms` on every frame.
//   - The watchdog checks it every WATCHDOG_INTERVAL. In a populated
//     RF environment, monitor mode always sees background Wi-Fi
//     beacons — so a multi-second silence is a genuine stall, not a
//     quiet patch.
//   - On stall: re-resolve ifindex from sysfs, re-verify + re-establish
//     monitor mode, signal the capture loop to re-open its libpcap
//     handle, emit substrate-audit events for the whole sequence,
//     and reflect the truth in radio_status.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::audit::SubstrateAuditEvent;
use crate::heartbeat::{RadioState, RadioStatus};
use crate::nl80211;

/// How often the watchdog checks capture liveness.
pub const WATCHDOG_INTERVAL: Duration = Duration::from_secs(10);

/// Silence threshold. In monitor mode in any populated RF environment
/// there is a continuous background of Wi-Fi beacons from nearby APs.
/// 15 seconds of *zero* packets is a genuine substrate stall, not a
/// quiet patch — confirmed against the 2-day cardinal soak where the
/// pcap had constant non-DJI background traffic.
pub const STALL_THRESHOLD: Duration = Duration::from_secs(15);

/// How long the watchdog waits for capture to resume after a heal
/// attempt before declaring the recovery failed.
pub const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared capture-liveness state. The capture loop, the Hunter, and
/// the watchdog all hold an `Arc<CaptureLiveness>`.
///
/// Substrate-honest design: NOBODY caches the ifindex. The watchdog
/// owns it (re-resolves from sysfs on stall); the capture loop and
/// Hunter READ it from `current_ifindex` every time they need it.
/// This is the direct fix for the 2026-05-13 ifindex-5→6 blind spot.
pub struct CaptureLiveness {
    pub frames: Arc<crate::status::FrameActivity>,
    /// Unix ms of the last frame the capture loop saw. Capture loop
    /// writes on every packet; watchdog reads. Zero = no packet yet.
    pub last_packet_unix_ms: AtomicU64,
    /// Open time is a stall deadline, never evidence that a packet arrived.
    capture_opened_unix_ms: AtomicU64,
    /// Current interface index. main.rs resolves it once at startup;
    /// the watchdog re-resolves on every stall. Capture + Hunter READ
    /// this; they never cache their own copy.
    pub current_ifindex: AtomicU32,
    /// Watchdog sets this true to ask the capture loop to drop its
    /// libpcap handle and re-open (on a possibly-new ifindex, in
    /// freshly-re-established monitor mode). Capture loop clears it
    /// after re-open.
    pub restart_requested: AtomicBool,
    /// Telemetry: total capture stalls detected since engine start.
    pub stall_events: AtomicU64,
    /// Telemetry: total successful recoveries.
    pub recovery_events: AtomicU64,
}

impl CaptureLiveness {
    pub fn new(initial_ifindex: u32) -> Arc<Self> {
        Arc::new(CaptureLiveness {
            frames: Arc::new(crate::status::FrameActivity::default()),
            last_packet_unix_ms: AtomicU64::new(0),
            capture_opened_unix_ms: AtomicU64::new(0),
            current_ifindex: AtomicU32::new(initial_ifindex),
            restart_requested: AtomicBool::new(false),
            stall_events: AtomicU64::new(0),
            recovery_events: AtomicU64::new(0),
        })
    }

    /// Only genuine packets on the currently opened path establish Up. Ignore
    /// an old ifindex or a handle already scheduled for replacement.
    pub fn mark_packet(&self, ifindex: u32, radio: &RadioState) {
        if self.restart_requested.load(Ordering::Relaxed)
            || ifindex != self.current_ifindex.load(Ordering::Relaxed) {
            return;
        }
        self.last_packet_unix_ms
            .store(now_unix_ms(), Ordering::Relaxed);
        self.frames.record(1);
        radio.set(RadioStatus::Up);
    }

    /// Every successful open starts Recovering, not Up. A separate deadline
    /// allows the watchdog to detect an open-but-dark interface at startup.
    pub fn mark_capture_open(&self, radio: &RadioState) {
        self.last_packet_unix_ms.store(0, Ordering::Relaxed);
        self.capture_opened_unix_ms.store(now_unix_ms(), Ordering::Relaxed);
        radio.set(RadioStatus::Recovering);
    }
}

/// Resolve a network-interface name to its kernel ifindex by reading
/// the canonical sysfs attribute. Pure-safe-Rust; works for any
/// interface the kernel can see. Substrate-honest: this is the
/// single source of ifindex truth — never cache its result, always
/// re-read.
pub fn iface_to_ifindex(iface: &str) -> Result<u32, std::io::Error> {
    let path = format!("/sys/class/net/{}/ifindex", iface);
    let s = std::fs::read_to_string(&path)?;
    s.trim().parse::<u32>().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("ifindex parse from {}: {}", path, e),
        )
    })
}

/// Run the Capture Liveness Watchdog. Pass to `tokio::spawn`. Never
/// returns under normal operation.
pub async fn run_watchdog(
    iface: String,
    liveness: Arc<CaptureLiveness>,
    radio: RadioState,
    node_id: String,
    audit_tx: Option<tokio::sync::mpsc::Sender<SubstrateAuditEvent>>,
) {
    println!(
        "[watchdog] Capture Liveness Watchdog active: iface={} \
         check_interval={}s stall_threshold={}s",
        iface,
        WATCHDOG_INTERVAL.as_secs(),
        STALL_THRESHOLD.as_secs()
    );

    let mut ticker = tokio::time::interval(WATCHDOG_INTERVAL);
    ticker.tick().await; // first tick fires immediately; skip it

    loop {
        ticker.tick().await;

        let last = liveness.last_packet_unix_ms.load(Ordering::Relaxed)
            .max(liveness.capture_opened_unix_ms.load(Ordering::Relaxed));
        // No successful open yet. Opening starts the stall deadline but does
        // not fake a packet timestamp or mark a dark radio Up.
        if last == 0 {
            continue;
        }

        let silence_ms = now_unix_ms().saturating_sub(last);
        if silence_ms < STALL_THRESHOLD.as_millis() as u64 {
            continue; // healthy: packets are flowing
        }

        // ── STALL DETECTED ──
        eprintln!(
            "[watchdog] CAPTURE STALL: {}ms since last packet (threshold {}ms)",
            silence_ms,
            STALL_THRESHOLD.as_millis()
        );
        liveness.stall_events.fetch_add(1, Ordering::Relaxed);
        radio.set(RadioStatus::Recovering);
        let recovery_start = Instant::now();

        // ── 1. Re-resolve ifindex ──
        let old_ifindex = liveness.current_ifindex.load(Ordering::Relaxed);
        let ifindex_changed = match iface_to_ifindex(&iface) {
            Ok(new_idx) => {
                if new_idx != old_ifindex {
                    eprintln!(
                        "[watchdog] ifindex changed: {} → {} (interface re-enumerated)",
                        old_ifindex, new_idx
                    );
                    liveness
                        .current_ifindex
                        .store(new_idx, Ordering::Relaxed);
                    true
                } else {
                    false
                }
            }
            Err(e) => {
                eprintln!(
                    "[watchdog] could not re-resolve ifindex for {}: {} \
                     (interface may be fully gone)",
                    iface, e
                );
                false
            }
        };
        let ifindex = liveness.current_ifindex.load(Ordering::Relaxed);

        // ── 2. Check + re-establish monitor mode ──
        // nl80211 calls are synchronous netlink round-trips; wrap in
        // spawn_blocking so they don't stall the async runtime (same
        // discipline the Hunter uses for set_channel).
        let was_monitor =
            match tokio::task::spawn_blocking(move || nl80211::interface_is_monitor(ifindex)).await {
                Ok(Ok(m)) => m,
                Ok(Err(e)) => {
                    eprintln!("[watchdog] interface_is_monitor probe failed: {}", e);
                    false // assume not-monitor; the re-establish is idempotent enough
                }
                Err(join_err) => {
                    eprintln!("[watchdog] interface_is_monitor join error: {}", join_err);
                    false
                }
            };
        let monitor_mode_lost = !was_monitor;
        if monitor_mode_lost {
            eprintln!(
                "[watchdog] interface not in monitor mode — re-establishing on ifindex {}",
                ifindex
            );
            match tokio::task::spawn_blocking(move || nl80211::establish_monitor_mode(ifindex)).await
            {
                Ok(Ok(())) => {
                    eprintln!("[watchdog] monitor mode re-established on ifindex {}", ifindex)
                }
                Ok(Err(e)) => eprintln!("[watchdog] establish_monitor_mode failed: {}", e),
                Err(join_err) => {
                    eprintln!("[watchdog] establish_monitor_mode join error: {}", join_err)
                }
            }
        }

        // ── 3. Capture the dmesg root-cause (best-effort) ──
        let dmesg_cause = capture_dmesg_cause(&iface).await;

        // ── 4. Emit the capture_stall audit event ──
        if let Some(tx) = audit_tx.as_ref() {
            let _ = tx.try_send(SubstrateAuditEvent::capture_stall(
                node_id.clone(),
                silence_ms,
                ifindex_changed,
                monitor_mode_lost,
                dmesg_cause.clone(),
            ));
        }

        // ── 5. Signal the capture loop to re-open libpcap ──
        liveness.restart_requested.store(true, Ordering::Relaxed);

        // ── 6. Wait for capture to resume ──
        let recovered = wait_for_recovery(&liveness, RECOVERY_TIMEOUT).await;
        let elapsed_ms = recovery_start.elapsed().as_millis() as u64;

        if recovered {
            // The capture path already set Up on a genuine packet. Do not
            // overwrite a newer capture error using the waiter's old result.
            liveness.recovery_events.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "[watchdog] capture RECOVERED in {}ms \
                 (ifindex_changed={}, monitor_mode_lost={})",
                elapsed_ms, ifindex_changed, monitor_mode_lost
            );
            if let Some(tx) = audit_tx.as_ref() {
                let _ = tx.try_send(SubstrateAuditEvent::capture_recovered(
                    node_id.clone(),
                    elapsed_ms,
                    ifindex_changed,
                    monitor_mode_lost,
                ));
            }
        } else {
            radio.set(RadioStatus::Error);
            eprintln!(
                "[watchdog] capture did NOT recover within {}s — \
                 radio_status: error. Operator intervention required.",
                RECOVERY_TIMEOUT.as_secs()
            );
            if let Some(tx) = audit_tx.as_ref() {
                let _ = tx.try_send(SubstrateAuditEvent::capture_recovery_failed(
                    node_id.clone(),
                    elapsed_ms,
                    dmesg_cause,
                ));
            }
        }
    }
}

/// Poll `last_packet_unix_ms` until a fresh packet arrives (capture
/// has resumed) or the timeout elapses. "Fresh" means a packet
/// timestamped after this function started waiting.
async fn wait_for_recovery(liveness: &CaptureLiveness, timeout: Duration) -> bool {
    let wait_start = now_unix_ms();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let last = liveness.last_packet_unix_ms.load(Ordering::Relaxed);
        if !liveness.restart_requested.load(Ordering::Relaxed) && last > wait_start {
            return true; // a packet arrived after we started waiting
        }
    }
    false
}

/// Best-effort dmesg root-cause capture. Greps the kernel ring buffer
/// for lines mentioning the interface or its driver. Returns None if
/// dmesg is unreadable (the engine has cap_net_admin + cap_net_raw
/// but NOT cap_syslog — on hardened kernels with dmesg_restrict=1
/// this will return None, which is substrate-honest: we report what
/// we can observe).
async fn capture_dmesg_cause(iface: &str) -> Option<String> {
    let output = tokio::process::Command::new("dmesg")
        .arg("--time-format")
        .arg("iso")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // Grab the last few lines mentioning the interface name or any
    // rtw88/mt76/ath driver — that's where the re-enumeration story
    // lives.
    let relevant: Vec<&str> = text
        .lines()
        .filter(|l| {
            l.contains(iface)
                || l.contains("rtw88")
                || l.contains("mt76")
                || l.contains("ath9k")
                || l.contains("ath10k")
        })
        .collect();
    if relevant.is_empty() {
        None
    } else {
        // Last 6 relevant lines, joined.
        let tail: Vec<&str> = relevant.iter().rev().take(6).rev().copied().collect();
        Some(tail.join(" | "))
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_liveness_starts_with_zero_last_packet() {
        let l = CaptureLiveness::new(5);
        assert_eq!(l.last_packet_unix_ms.load(Ordering::Relaxed), 0);
        assert_eq!(l.current_ifindex.load(Ordering::Relaxed), 5);
        assert!(!l.restart_requested.load(Ordering::Relaxed));
    }

    #[test]
    fn mark_packet_sets_recent_timestamp() {
        let l = CaptureLiveness::new(5);
        let before = now_unix_ms();
        l.mark_packet(5, &RadioState::new());
        let stamped = l.last_packet_unix_ms.load(Ordering::Relaxed);
        assert!(stamped >= before, "mark_packet must store a current timestamp");
    }

    #[test]
    fn disappear_reopen_new_ifindex_packet_restores_up_not_open_alone() {
        let l = CaptureLiveness::new(5);
        let radio = RadioState::new();
        l.mark_capture_open(&radio);
        assert_eq!(radio.get(), RadioStatus::Recovering);
        assert_eq!(l.last_packet_unix_ms.load(Ordering::Relaxed), 0);
        l.mark_packet(5, &radio);
        assert_eq!(radio.get(), RadioStatus::Up);
        radio.set(RadioStatus::Error); // device disappeared / pcap hard error
        l.current_ifindex.store(6, Ordering::Relaxed);
        l.mark_capture_open(&radio); // autonomous reopen, no watchdog waiter
        assert_eq!(radio.get(), RadioStatus::Recovering);
        l.mark_packet(5, &radio); // stale handle is not evidence
        assert_eq!(radio.get(), RadioStatus::Recovering);
        assert_eq!(l.last_packet_unix_ms.load(Ordering::Relaxed), 0);
        l.restart_requested.store(true, Ordering::Relaxed);
        l.mark_packet(6, &radio);
        assert_eq!(radio.get(), RadioStatus::Recovering);
        l.restart_requested.store(false, Ordering::Relaxed);
        l.mark_packet(6, &radio);
        assert_eq!(radio.get(), RadioStatus::Up);
    }

    #[test]
    fn iface_to_ifindex_reads_loopback() {
        // Substrate-truth check against a real interface that always
        // exists: loopback. ifindex of `lo` is conventionally 1, but
        // we only assert it parses to *something* — the substrate
        // truth is "the sysfs read + parse path works."
        let result = iface_to_ifindex("lo");
        assert!(result.is_ok(), "loopback ifindex must resolve: {:?}", result);
        assert!(result.unwrap() >= 1);
    }

    #[test]
    fn iface_to_ifindex_errors_on_nonexistent() {
        let result = iface_to_ifindex("definitely-not-a-real-iface-xyz");
        assert!(result.is_err());
    }
}
