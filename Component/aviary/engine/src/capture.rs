// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use arc_swap::ArcSwap;
use cybrrd_rid_protocol::models::{Node, NodeLocation, NormalizedTelemetry, PositionSource};
use pcap::Capture;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::{self, error::TrySendError};

use crate::audit::DropCounters;
use crate::dedup::DedupGate;
use crate::heartbeat::{RadioState, RadioStatus};
use crate::hunter::HunterState;
use crate::nl80211;
use crate::sensor_gps::GpsFix;
use crate::watchdog::CaptureLiveness;

/// Wave 7.3a (2026-05-26): how recent a GPS fix must be for the
/// capture-loop stamper to use it instead of config-static node.location.
///
/// Matches the GPS sensor's own `stale_after = 30s`: both sides of the
/// boundary agree on "what's stale." Beyond this, the stamper falls
/// back to config-static — a sensor that lost lock 10 minutes ago is
/// not telling the truth about where the witness is RIGHT NOW.
const GPS_FIX_FRESH_THRESHOLD_MS: i64 = 30_000;

/// Wave 7.3a: pick the Node identity to stamp into this frame.
///
/// If a fresh GPS fix is present in the shared swap, build a Node with
/// the GPS-derived lat/lon/alt; otherwise fall back to the config-static
/// Node (the install-time location). Cheap: one ArcSwap load (lock-free
/// atomic) per frame.
///
/// Substrate-truth: the GPS sensor already gates against invalid fixes
/// upstream (`parse_nmea_line` rejects `fix_quality == Invalid`), so
/// anything we read here is at least quality=1. The freshness gate is
/// defense-in-depth: if the sensor task crashed and the swap still
/// holds a stale value, the threshold catches it.
fn stamp_node_with_gps(
    static_node: &Node,
    gps_latest: &ArcSwap<Option<GpsFix>>,
    now_ms: i64,
    fresh_threshold_ms: i64,
) -> Node {
    let guard = gps_latest.load();
    if let Some(fix) = guard.as_ref() {
        if now_ms - fix.fix_at_ms <= fresh_threshold_ms {
            return Node {
                id: static_node.id.clone(),
                version: static_node.version.clone(),
                location: NodeLocation {
                    lat: fix.lat,
                    lon: fix.lon,
                    alt_m: fix.alt_m,
                    // Wave 7.4: tag the position-truth provenance so
                    // every frame in the lake self-attests its source.
                    // Globe-backend Reputation Gravity can then accept
                    // GpsLive frames into trust accumulation and refuse
                    // ConfigStatic ones — closing the Reputation-
                    // Portability Attack vector at the consumer side.
                    position_source: Some(PositionSource::GpsLive),
                },
            };
        }
    }
    // Fall-through: clone the static_node verbatim. Its NodeLocation
    // already carries `position_source = Some(ConfigStatic)` from
    // `node_config::to_node`, which propagates the un-anchored
    // self-disclosure through to the wire.
    static_node.clone()
}

// Wave 7.1 Inc 8 substrate-truth note (2026-05-14):
//
// The Wave 7 (2026-05-07) resolution was: the operator pre-configures
// monitor mode with a manual `iw` dance, and the engine just opens
// libpcap on an already-configured interface. That worked — but it
// assumed monitor mode, once set, *stays* set. The 2026-05-13 Alfa
// cable-bump proved it doesn't: a USB re-enumeration brought the
// interface back in `managed` mode with a new ifindex, and the engine
// ran capture-dark for ~18 hours while still reporting radio_status:up.
//
// Inc 8 retires the manual `iw` dance. The engine now establishes
// monitor mode itself, natively, via nl80211 (`nl80211::
// establish_monitor_mode`) — exactly the "replace shell-outs with
// native crates" direction the Wave 7 note below already anticipated.
// This is the SAME netlink path the Hunter's `set_channel` already
// drives successfully from the engine binary's own cap_net_admin: no
// subprocess cap-propagation tax, no hardened-iproute2 capset(0,0,0)
// problem, no libpcap-rfmon Wireless-Extensions detection gap.
//
// Structure: an OUTER loop owns (re-)establishment — resolve ifindex
// from the shared CaptureLiveness atomic, ensure monitor mode, open
// libpcap. An INNER loop (spawn_blocking) runs the hot capture path,
// bumping `last_packet_unix_ms` on every frame. The watchdog observes
// that atomic; on a stall it re-resolves ifindex, re-establishes
// monitor mode, and sets `restart_requested` — the inner loop notices,
// exits, and the outer loop re-opens cleanly. radio_status now tells
// the truth: up / recovering / error reflect actual capture state.
//
// Channel hopping is the Hunter's job (it drives nl80211::set_channel
// directly). For Hunter-disabled deployments the operator still locks
// a channel once; Inc 8 only takes over the monitor-MODE step, not
// channel selection.

/// Why the inner (hot-path) capture loop exited. The outer loop uses
/// this to decide whether to re-open the capture path or shut down.
enum InnerExit {
    /// The watchdog set `restart_requested` — re-resolve, re-establish
    /// monitor mode, re-open libpcap.
    RestartRequested,
    /// libpcap returned a hard (non-timeout) error — the device is
    /// likely gone. Re-open after a brief backoff; the watchdog will
    /// have re-resolved the ifindex by then.
    CaptureError(String),
    /// The backhaul mpsc channel is closed — the engine is shutting
    /// down. Do NOT re-open; let the capture loop return.
    ChannelClosed,
}

/// Wall-clock at the moment of feeder-side frame emission, in Unix
/// milliseconds.
///
/// Wave 6.4.1 wire-format lock 2026-05-03 (substrate-truth): the
/// 10 Hz Virilio Spool cadence is incompatible with seconds-resolution
/// timestamps — same-second frames produce Δt=0 at the Bohr Spool's
/// spacetime gate, generating spurious TEMPORAL_INVERSION rejections.
/// Milliseconds align the wire-format with the actual emission cadence.
///
/// Note: this is the FEEDER's emit timestamp. ASTM auth-page timestamps
/// (cybrrd_rid_protocol::AuthInfo.timestamp_utc) remain in seconds —
/// they're drone-sourced data, not feeder-emit time, so the unit
/// stays whatever the wire-from-drone supplies.
fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Ensure the capture interface is in monitor mode, establishing it
/// natively via nl80211 if it is not. Substrate-honest: probe first
/// (`interface_is_monitor`), then only act if needed. Best-effort —
/// logs and returns even on failure so the caller can still attempt
/// the libpcap open (and the watchdog can retry).
async fn ensure_monitor_mode(interface: &str, ifindex: u32) {
    match tokio::task::spawn_blocking(move || nl80211::interface_is_monitor(ifindex)).await {
        Ok(Ok(true)) => {
            println!(
                "[capture] {} (ifindex {}) confirmed in monitor mode",
                interface, ifindex
            );
        }
        Ok(Ok(false)) => {
            println!(
                "[capture] {} (ifindex {}) NOT in monitor mode — auto-establishing (Wave 7.1 Inc 8)",
                interface, ifindex
            );
            match tokio::task::spawn_blocking(move || nl80211::establish_monitor_mode(ifindex))
                .await
            {
                Ok(Ok(())) => {
                    println!(
                        "[capture] monitor mode established natively on ifindex {}",
                        ifindex
                    );
                    // Brief settle window: the kernel needs a moment
                    // after the down→set-type→up sequence before
                    // libpcap can open a flowing capture.
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                Ok(Err(e)) => eprintln!(
                    "[capture] establish_monitor_mode failed: {} — \
                     attempting libpcap open anyway",
                    e
                ),
                Err(join_err) => eprintln!(
                    "[capture] establish_monitor_mode join error: {} — \
                     attempting libpcap open anyway",
                    join_err
                ),
            }
        }
        Ok(Err(e)) => eprintln!(
            "[capture] interface_is_monitor probe failed: {} — \
             attempting libpcap open anyway",
            e
        ),
        Err(join_err) => eprintln!(
            "[capture] interface_is_monitor join error: {} — \
             attempting libpcap open anyway",
            join_err
        ),
    }
}

/// Capture loop. Per Wave 6.0b consensus:
///   - dedup on the (drone_id, mac) tuple within `dedup_window_ms`
///   - non-blocking try_send so a saturated backhaul doesn't stall the
///     radio capture thread; on `Full`, increment the `buffer_full` drop
///     counter that the audit emitter folds into 1Hz aggregate events.
///
/// Wave 7.1 Inc 8: the loop is now restartable. It establishes monitor
/// mode natively before each libpcap open, bumps the shared
/// `CaptureLiveness` clock on every frame, and re-opens cleanly when
/// the watchdog requests it.
pub async fn start_capture_loop(
    tx: mpsc::Sender<NormalizedTelemetry>,
    node: Node,
    interface: String,
    dedup_window_ms: u64,
    counters: DropCounters,
    radio: RadioState,
    hunter_state: Option<Arc<HunterState>>,
    lock_on_duration_ms: u64,
    liveness: Arc<CaptureLiveness>,
    // Wave 7.3a (2026-05-26): shared GPS-fix swap. When the stamper
    // finds a fresh fix here, frame.node.location is substituted with
    // the GPS-derived position. When None or stale, falls back to the
    // config-static `node` parameter — the install-time identity.
    gps_latest: Arc<ArcSwap<Option<GpsFix>>>,
    // Wave 6.5 (2026-06-04): per-drone observation cache feeding the
    // Green Protocol tick_publisher. When `Some`, each successful
    // payload is also recorded into the store (parallel-path tap to
    // the existing backhaul send). When `None`, this is a no-op —
    // legacy single-path mode. Design decision 2026-06-04.
    observation_store: Option<Arc<crate::tick_publisher::ObservationStore>>,
    forensic_config: Option<crate::node_config::SavefileYaml>,
    forensic_audit: mpsc::Sender<crate::audit::SubstrateAuditEvent>,
) {
    println!("[*] Target interface: {}", interface);
    println!(
        "[*] Wave 7.1 Inc 8: engine establishes monitor mode natively via nl80211 — \
         no manual `iw` dance required."
    );

    let mut first_open = true;

    // ── OUTER loop: owns capture-path (re-)establishment ──
    loop {
        // Re-resolve the ifindex from sysfs every outer iteration —
        // never trust a cached value across a re-open. The 2026-05-13
        // lesson: a USB re-enumeration changes the ifindex (5 → 6), and
        // the capture loop must pick that up the instant it re-opens,
        // not wait for the watchdog's next 10s tick. The watchdog also
        // re-resolves on stall; both writing the same atomic is
        // idempotent.
        match crate::watchdog::iface_to_ifindex(&interface) {
            Ok(idx) => liveness.current_ifindex.store(idx, Ordering::Relaxed),
            Err(e) => eprintln!(
                "[capture] could not resolve ifindex for {}: {} — using last-known {}",
                interface,
                e,
                liveness.current_ifindex.load(Ordering::Relaxed)
            ),
        }
        let ifindex = liveness.current_ifindex.load(Ordering::Relaxed);

        // Auto-establish monitor mode (startup heal + watchdog-triggered
        // re-heal both flow through here).
        ensure_monitor_mode(&interface, ifindex).await;

        let radio_init_started = Instant::now();

        // Open libpcap on the (now monitor-mode) interface. We do NOT
        // request rfmon at the libpcap layer — libpcap's pcap_set_rfmon()
        // probes via Wireless-Extensions IOCTLs the modern rtw88_8812au
        // driver doesn't expose. Monitor mode is established above via
        // nl80211; here we just open a packet socket on it.
        let cap = match Capture::from_device(interface.as_str())
            .and_then(|c| c.promisc(true).timeout(100).open())
        {
            Ok(cap) => cap,
            Err(e) => {
                eprintln!("[-] open capture on {}: {}", interface, e);
                radio.set(RadioStatus::Error);
                // Backoff before retry; the watchdog re-resolves the
                // ifindex independently, so a re-enumerated device will
                // be reachable on the next outer iteration.
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        let radio_init_ms = radio_init_started.elapsed().as_millis();
        println!(
            "[+] {} active for capture (radio_init_ms={}).",
            interface, radio_init_ms
        );

        let mut recorder = crate::forensic::Recorder::new(forensic_config.clone(), cap.get_datalink().0 as u32, node.id.clone(), radio.clone(), forensic_audit.clone());

        // Opening is not packet evidence. This also covers autonomous reopens
        // after a hard error, when the watchdog never entered its waiter.
        liveness.mark_capture_open(&radio);

        // Clear the restart flag now that we've re-opened — the
        // watchdog's signal has been consumed.
        liveness.restart_requested.store(false, Ordering::Relaxed);

        println!(
            "[+] Capture loop active. Dedup window={}ms; backhaul try_send buffer=mpsc(1024). \
             first_open={}",
            dedup_window_ms, first_open
        );
        first_open = false;

        // Per-iteration clones for the move into spawn_blocking.
        let inner_tx = tx.clone();
        let inner_counters = counters.clone();
        let inner_node = node.clone();
        let inner_hunter = hunter_state.clone();
        let inner_liveness = Arc::clone(&liveness);
        let inner_radio = radio.clone();
        let inner_observation_store = observation_store.as_ref().map(Arc::clone);
        // Wave 7.3a — GPS fix swap + one-shot "first GPS-stamped frame"
        // observability flag (logged exactly once per outer iteration when
        // the stamper first transitions from static-fallback to GPS-derived).
        let inner_gps = Arc::clone(&gps_latest);

        // ── INNER loop: the hot capture path ──
        let handle = tokio::task::spawn_blocking(move || -> InnerExit {
            let mut cap = cap;
            let mut pipeline = PostParse::new(ForwardContext {
                tx: inner_tx, node: inner_node, counters: inner_counters,
                hunter: inner_hunter, lock_on_duration_ms, gps: inner_gps,
                observation_store: inner_observation_store,
            }, dedup_window_ms);

            loop {
                let packet = match cap.next_packet() {
                    Ok(p) => p,
                    Err(pcap::Error::TimeoutExpired) => {
                        recorder.poll();
                        // Normal: no packet within the 100ms libpcap
                        // timeout. Check whether the watchdog wants us
                        // to re-open, then keep waiting.
                        if inner_liveness.restart_requested.load(Ordering::Relaxed) {
                            return InnerExit::RestartRequested;
                        }
                        continue;
                    }
                    Err(e) => {
                        // Hard capture error — the device is likely gone
                        // (USB re-enumeration, driver crash). Exit so the
                        // outer loop can re-establish.
                        return InnerExit::CaptureError(e.to_string());
                    }
                };

                // Substrate-truth liveness: a frame arrived. Bump the
                // shared clock the watchdog watches. This is the single
                // honest signal that capture is actually flowing.
                inner_liveness.mark_packet(ifindex, &inner_radio);

                recorder.write(&packet);

                let Some(data) = ingest_monitor_frame(packet.data) else {
                    // Not a drone-class frame. Still check the restart
                    // flag so a stall during a quiet-DJI / busy-Wi-Fi
                    // window is still responsive.
                    if inner_liveness.restart_requested.load(Ordering::Relaxed) {
                        return InnerExit::RestartRequested;
                    }
                    continue;
                };

                if !pipeline.forward(data) {
                    return InnerExit::ChannelClosed;
                }

                if inner_liveness.restart_requested.load(Ordering::Relaxed) {
                    return InnerExit::RestartRequested;
                }
            }
        });

        // Wait for the inner loop to exit and decide what to do next.
        match handle.await {
            Ok(InnerExit::RestartRequested) => {
                println!(
                    "[capture] watchdog requested restart — re-establishing capture path"
                );
                // Outer loop iterates: re-resolve ifindex, re-establish
                // monitor mode, re-open libpcap.
            }
            Ok(InnerExit::CaptureError(msg)) => {
                eprintln!(
                    "[capture] hard capture error: {} — re-opening after brief backoff",
                    msg
                );
                radio.set(RadioStatus::Error);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok(InnerExit::ChannelClosed) => {
                eprintln!("[capture] backhaul channel closed — capture loop shutting down");
                return;
            }
            Err(join_err) => {
                eprintln!(
                    "[capture] inner-loop join error: {} — re-opening after brief backoff",
                    join_err
                );
                radio.set(RadioStatus::Error);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}


/// Shared post-decode path for Wi-Fi and BLE. State stays local to each reader;
/// processing order and non-blocking backhaul semantics are identical.
pub struct ForwardContext {
    pub tx: mpsc::Sender<NormalizedTelemetry>,
    pub node: Node,
    pub counters: DropCounters,
    pub hunter: Option<Arc<HunterState>>,
    pub lock_on_duration_ms: u64,
    pub gps: Arc<ArcSwap<Option<GpsFix>>>,
    pub observation_store: Option<Arc<crate::tick_publisher::ObservationStore>>,
}

pub struct PostParse {
    context: ForwardContext,
    dedup: DedupGate,
    first_gps_stamp: AtomicBool,
    frames_since_gc: u32,
}

/// Preserve the existing Beacon radiotap/RSSI behavior, and normalize NAN's
/// optional capture FCS trailer before its exact-length attribute walk.
fn ingest_monitor_frame(raw: &[u8]) -> Option<cybrrd_rid_protocol::models::TelemetryData> {
    if raw.len() < 8 {
        return None;
    }
    let radiotap_len = u16::from_le_bytes([raw[2], raw[3]]) as usize;
    let radiotap_bytes = raw.get(..radiotap_len)?;
    let mut dot11 = raw.get(radiotap_len..)?;
    let rtap = radiotap::Radiotap::from_bytes(radiotap_bytes).ok();
    let rssi_dbm = rtap.as_ref().and_then(|r| r.antenna_signal)
        .map(|s| s.value as i32).unwrap_or(-100);
    if dot11.first() == Some(&0xd0) {
        if let Some(flags) = rtap.as_ref()?.flags {
            if flags.bad_fcs {
                return None;
            }
            if flags.fcs {
                dot11 = dot11.get(..dot11.len().checked_sub(4)?)?;
            }
        }
    }
    cybrrd_rid_protocol::router::ingest_frame(dot11, rssi_dbm)
}

impl PostParse {
    pub fn new(context: ForwardContext, window_ms: u64) -> Self {
        Self {
            context,
            dedup: DedupGate::new(window_ms),
            first_gps_stamp: AtomicBool::new(false),
            frames_since_gc: 0,
        }
    }

    /// False means the backhaul receiver is closed; caller must stop.
    pub fn forward(&mut self, data: cybrrd_rid_protocol::models::TelemetryData) -> bool {
        // Wave 7.1 Inc 6 + 7.1b — protocol-neutral lock-on
        // signal with capture-budget tracking. ANY frame that
        // parses as a valid drone RID broadcast (vendor-blind:
        // DJI, Skydio, Autel, ELRS, custom builds — all
        // converge on the cybrrd_rid_protocol detector) tells
        // the Hunter to glue the radio for at MOST
        // self.context.lock_on_duration_ms, releasing earlier (after
        // lock_on_min_ms minimum) once we've observed both
        // drone_id + known position. The dedup gate is
        // intentionally downstream of this signal: duplicate
        // frames within the dedup window still refresh the
        // lock window because they're evidence the drone is
        // still broadcasting on this channel.
        if let Some(hs) = self.context.hunter.as_ref() {
            hs.record_drone_observation(
                &data.drone_id,
                data.pos.is_some(),
                self.context.lock_on_duration_ms,
            );
        }

        // Dedup gate (Wave 6.0b): suppress duplicates within window.
        if !self.dedup.should_emit_observation(&data) {
            return true;
        }

        // Wave 7.3a: dynamic node identity. If GPS has a fresh
        // fix, the stamper substitutes its lat/lon/alt into the
        // Node we put on the wire. Otherwise we fall back to the
        // config-static `self.context.node` (install-time identity).
        //
        // `now_unix_ms()` here returns u64 (capture.rs-local
        // helper, Wave 6.4.1); GpsFix.fix_at_ms is i64 (sensor
        // module convention). Cast at the freshness-math seam
        // so the stamp helper has one consistent signed type.
        let frame_ts_ms = now_unix_ms();
        let stamped_node = stamp_node_with_gps(
            &self.context.node,
            &self.context.gps,
            frame_ts_ms as i64,
            GPS_FIX_FRESH_THRESHOLD_MS,
        );
        // One-shot observability log the first time we substitute
        // GPS data — confirms in journalctl that the Witness-Truth
        // path is live (vs the Config-Static fallback).
        if stamped_node.location.lat != self.context.node.location.lat
            || stamped_node.location.lon != self.context.node.location.lon
        {
            if !self.first_gps_stamp.swap(true, Ordering::Relaxed) {
                println!(
                    "[+] capture: stamping frames with live GPS fix \
                     (was config-static {:.6},{:.6}; now {:.6},{:.6})",
                    self.context.node.location.lat,
                    self.context.node.location.lon,
                    stamped_node.location.lat,
                    stamped_node.location.lon,
                );
            }
        }
        let payload = NormalizedTelemetry {
            wire_format_version: Some(cybrrd_rid_protocol::models::WIRE_FORMAT_VERSION),
            node: stamped_node,
            timestamp_utc: frame_ts_ms,
            data,
        };

        // Wave 6.5 Green Protocol tap (design decision 2026-06-04):
        // record the observation into the shared store before
        // handing the payload off to backhaul. The tick_publisher
        // reads from this store once per second and emits the
        // batched AirspaceState envelope. No-op when the store
        // is None (legacy single-path mode).
        if let Some(ref store) = self.context.observation_store {
            store.record(&payload);
        }

        // Non-blocking send: backhaul saturation must not stall radio capture.
        match self.context.tx.try_send(payload) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.context.counters.buffer_full.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Closed(_)) => {
                eprintln!("[-] Backhaul channel closed; halting capture.");
                return false;
            }
        }

        self.frames_since_gc = self.frames_since_gc.wrapping_add(1);
        if self.frames_since_gc >= 1024 {
            self.dedup.gc();
            self.frames_since_gc = 0;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cybrrd_rid_protocol::models::NodeLocation;

    #[test]
    fn unknown_position_reaches_backhaul_and_tick_store_without_fabrication() {
        let mut bytes = [0_u8; 53];
        bytes[..3].copy_from_slice(&[0xF2, 25, 2]);
        bytes[4] = 0x10;
        bytes[5..9].copy_from_slice(b"D27E");
        bytes[28] = 0x12;
        bytes[29] = 2 << 4; // contradictory airborne claim, raw coordinates zero
        let data = cybrrd_rid_protocol::astm::parse_message_pack(&bytes, [1; 6], -40).unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        let counters = DropCounters::new();
        let store = crate::tick_publisher::ObservationStore::new();
        let mut pipe = PostParse::new(ForwardContext {
            tx, node: static_node(), counters: counters.clone(), hunter: None,
            lock_on_duration_ms: 0, gps: Arc::new(ArcSwap::from_pointee(None)),
            observation_store: Some(Arc::clone(&store)),
        }, 1000);
        assert!(pipe.forward(data));
        let frame = rx.try_recv().expect("unknown entity must enter the backhaul queue");
        let value = serde_json::to_value(frame).unwrap();
        assert_eq!(value["wire_format_version"], 4);
        assert_eq!(value["data"]["drone_id"], "D27E");
        assert!(value["data"].get("pos").is_none());
        assert_eq!(value["data"]["operational_status"], 2);
        assert_eq!(value["data"]["position_unknown_reason"], "zero_pair");
        assert_eq!(store.len(), 1);
        assert_eq!(counters.buffer_full.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn verify_req_brrd_014_shared_pipeline_gps_dedup_store_backpressure() {
        use cybrrd_rid_protocol::models::{RidTransport, TelemetryData};
        for transport in [None, Some(RidTransport::Bt4Legacy), Some(RidTransport::Bt5LongRange)] {
            let mut data: TelemetryData = serde_json::from_str(r#"{"protocol":"ASTM_F3411_22a","mac_address":"01:02:03:04:05:06","drone_id":"TEST-LOOP-20260915","pos":{"lat":40.8817,"lon":-95.6901,"alt_m":120},"signal_rssi_dbm":-45}"#).unwrap();
            data.transport = transport;
            let (tx, mut rx) = mpsc::channel(1);
            let counters = DropCounters::new();
            let hunter = HunterState::new();
            let store = crate::tick_publisher::ObservationStore::new();
            let gps = fresh_fix(now_unix_ms() as i64);
            let mut pipe = PostParse::new(ForwardContext {
                tx, node: static_node(), counters: counters.clone(), hunter: Some(Arc::clone(&hunter)),
                lock_on_duration_ms: 2000, gps: Arc::new(ArcSwap::from_pointee(Some(gps.clone()))),
                observation_store: Some(Arc::clone(&store)),
            }, 1000);
            assert!(pipe.forward(data.clone()));
            assert!(hunter.lock_on_active());
            assert_eq!(store.len(), 1);
            let frame = rx.try_recv().unwrap();
            assert_eq!(frame.node.location.lat, gps.lat);
            assert_eq!(frame.node.location.position_source, Some(PositionSource::GpsLive));
            assert_eq!(frame.data.transport, transport);
            assert!(pipe.forward(data.clone()));
            assert!(rx.try_recv().is_err());
            data.drone_id = "SECOND".into();
            assert!(pipe.forward(data.clone())); // fills the queue
            data.drone_id = "THIRD".into();
            assert!(pipe.forward(data.clone())); // store tap survives full backhaul
            assert_eq!(counters.buffer_full.load(Ordering::Relaxed), 1);
            assert_eq!(store.len(), 3);
            drop(rx);
            data.drone_id = "FOURTH".into();
            assert!(!pipe.forward(data));
        }
    }

    fn static_node() -> Node {
        Node {
            id: "brrdfeeder-test-001".into(),
            version: "0.2.0-test".into(),
            location: NodeLocation {
                lat: 40.882590, lon: -95.690920, alt_m: 320.0,
                position_source: Some(PositionSource::ConfigStatic),
            },
        }
    }

    #[test]
    fn nan_rotating_mac_deduplicates_by_identity() {
        use cybrrd_rid_protocol::models::TelemetryData;
        let mut data: TelemetryData = serde_json::from_str(r#"{"protocol":"ASTM_F3411_22a","transport":"wifi_nan","mac_address":"02:01:02:03:04:05","drone_id":"NAN-TEST-01","hardware_serial":"NAN-TEST-01","signal_rssi_dbm":-49}"#).unwrap();
        let (tx, mut rx) = mpsc::channel(8);
        let store = crate::tick_publisher::ObservationStore::new();
        let mut pipe = PostParse::new(ForwardContext {
            tx, node: static_node(), counters: DropCounters::new(), hunter: None,
            lock_on_duration_ms: 0, gps: Arc::new(ArcSwap::from_pointee(None)),
            observation_store: Some(Arc::clone(&store)),
        }, 1000);
        assert!(pipe.forward(data.clone()));
        assert_eq!(rx.try_recv().unwrap().data.mac_address, data.mac_address);
        data.mac_address = [2, 9, 8, 7, 6, 5];
        assert!(pipe.forward(data.clone()));
        assert!(rx.try_recv().is_err(), "NAN address rotation split one pack identity");
        assert_eq!(store.len(), 1);
        data.drone_id = "NAN-TEST-02".into();
        data.hardware_serial = Some(data.drone_id.clone());
        assert!(pipe.forward(data.clone()));
        assert_eq!(rx.try_recv().unwrap().data.drone_id, "NAN-TEST-02");
        assert_eq!(store.len(), 2);
        data.drone_id = "UNKNOWN".into();
        data.hardware_serial = None;
        for mac in [[2; 6], [4; 6]] {
            data.mac_address = mac;
            assert!(pipe.forward(data.clone()));
            assert_eq!(rx.try_recv().unwrap().data.mac_address, mac);
        }
        assert_eq!(store.len(), 2, "identity-less NAN must not create a shared UNKNOWN track");
    }

    #[test]
    fn nan_monitor_fcs_and_backhaul_preserve_provenance() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../cybrrd-rid-protocol/tests/fixtures/nan-oracle.json"
        )).unwrap();
        let frame = hex::decode(fixture["cases"][8]["frame_hex"].as_str().unwrap()).unwrap();
        let mut raw = vec![0, 0, 10, 0, 0x22, 0, 0, 0, 0, (-49_i8) as u8];
        raw.extend_from_slice(&frame);
        let no_fcs = ingest_monitor_frame(&raw).unwrap();
        raw[8] = 0x10;
        raw.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);
        let with_fcs = ingest_monitor_frame(&raw).unwrap();
        assert_eq!(serde_json::to_value(&no_fcs).unwrap(), serde_json::to_value(&with_fcs).unwrap());
        raw[8] = 0x50;
        assert!(ingest_monitor_frame(&raw).is_none(), "driver-reported bad FCS admitted");
        raw[8] = 0;
        assert!(ingest_monitor_frame(&raw).is_none(), "unannounced trailer mistaken for attributes");

        let (tx, mut rx) = mpsc::channel(1);
        let mut pipe = PostParse::new(ForwardContext {
            tx, node: static_node(), counters: DropCounters::new(), hunter: None,
            lock_on_duration_ms: 0, gps: Arc::new(ArcSwap::from_pointee(None)),
            observation_store: None,
        }, 1000);
        assert!(pipe.forward(with_fcs));
        let wire = serde_json::to_value(rx.try_recv().unwrap()).unwrap();
        assert_eq!(wire["wire_format_version"], 4);
        assert_eq!(wire["data"]["transport"], "wifi_nan");
        assert_eq!(wire["data"]["message_counter"], 0);
        assert_eq!(wire["data"]["mac_address"], "02:01:02:03:04:05");
        assert_eq!(wire["data"]["wifi_bssid"], "50:6f:9a:01:00:ff");
        assert_eq!(wire["data"]["signal_rssi_dbm"], -49);
    }

    fn fresh_fix(now_ms: i64) -> GpsFix {
        GpsFix {
            lat: 40.999_999,
            lon: -95.111_111,
            alt_m: 345.5,
            fix_quality: 1,
            sat_count: 5,
            hdop: 1.2,
            fix_at_ms: now_ms - 1_000, // 1s ago, well inside threshold
            gps_utc_ms: Some(now_ms - 1_000),
        }
    }

    #[test]
    fn stamp_falls_back_to_static_when_swap_is_none() {
        let s = static_node();
        let swap = ArcSwap::from_pointee(None::<GpsFix>);
        let out = stamp_node_with_gps(&s, &swap, 1_000_000, 30_000);
        assert_eq!(out.location.lat, s.location.lat);
        assert_eq!(out.location.lon, s.location.lon);
        assert_eq!(out.location.alt_m, s.location.alt_m);
        assert_eq!(out.id, s.id);
        assert_eq!(out.version, s.version);
    }

    #[test]
    fn stamp_substitutes_when_fix_is_fresh() {
        let s = static_node();
        let now = 1_000_000_i64;
        let fix = fresh_fix(now);
        let swap = ArcSwap::from_pointee(Some(fix.clone()));
        let out = stamp_node_with_gps(&s, &swap, now, 30_000);
        assert!((out.location.lat - fix.lat).abs() < 1e-9);
        assert!((out.location.lon - fix.lon).abs() < 1e-9);
        assert!((out.location.alt_m - fix.alt_m).abs() < 1e-3);
        // Identity preserved across the substitution.
        assert_eq!(out.id, s.id);
        assert_eq!(out.version, s.version);
    }

    /// Substrate-truth: a stale fix (older than threshold) must NOT be
    /// stamped — the witness has lost lock and may have moved since.
    /// Fall back to config-static rather than claim a position that's
    /// silently aged out.
    #[test]
    fn stamp_falls_back_when_fix_is_stale() {
        let s = static_node();
        let now = 1_000_000_i64;
        let mut fix = fresh_fix(now);
        fix.fix_at_ms = now - 60_000; // 60s old — past 30s default threshold
        let swap = ArcSwap::from_pointee(Some(fix));
        let out = stamp_node_with_gps(&s, &swap, now, 30_000);
        // Should be static_node values, NOT GPS values.
        assert_eq!(out.location.lat, s.location.lat);
        assert_eq!(out.location.lon, s.location.lon);
    }

    /// Boundary: a fix exactly at the threshold age is still considered
    /// fresh (`now - fix_at_ms <= threshold`).
    #[test]
    fn stamp_substitutes_at_exact_threshold_boundary() {
        let s = static_node();
        let now = 1_000_000_i64;
        let threshold = 30_000_i64;
        let mut fix = fresh_fix(now);
        fix.fix_at_ms = now - threshold; // exactly threshold ago
        let swap = ArcSwap::from_pointee(Some(fix.clone()));
        let out = stamp_node_with_gps(&s, &swap, now, threshold);
        // Inclusive boundary: should substitute.
        assert!((out.location.lat - fix.lat).abs() < 1e-9);
        assert!((out.location.lon - fix.lon).abs() < 1e-9);
    }

    // ── Wave 7.4 — PositionSource attestation ──────────────────────────

    /// When stamping with a fresh GPS fix, the returned Node MUST carry
    /// `position_source = Some(GpsLive)`. This is the trust-anchor signal
    /// that closes the Reputation-Portability Attack vector — downstream
    /// Reputation Gravity uses this field to decide whether the frame
    /// participates in trust accumulation.
    #[test]
    fn stamp_with_fresh_fix_attests_gps_live() {
        let s = static_node();
        let now = 1_000_000_i64;
        let fix = fresh_fix(now);
        let swap = ArcSwap::from_pointee(Some(fix));
        let out = stamp_node_with_gps(&s, &swap, now, 30_000);
        assert_eq!(
            out.location.position_source,
            Some(PositionSource::GpsLive),
            "fresh GPS fix must tag node.location with GpsLive provenance"
        );
    }

    /// When falling back to config-static (no fix, stale fix, or no GPS),
    /// the returned Node MUST carry the static's position_source — which
    /// `node_config::to_node` sets to `Some(ConfigStatic)`. This is the
    /// self-disclosure: even fallback frames declare their provenance.
    #[test]
    fn stamp_fallback_preserves_config_static_attestation() {
        // Static node carries ConfigStatic (matches what to_node() produces).
        let mut s = static_node();
        s.location.position_source = Some(PositionSource::ConfigStatic);
        // Empty swap → fallback path
        let swap = ArcSwap::from_pointee(None::<GpsFix>);
        let out = stamp_node_with_gps(&s, &swap, 1_000_000, 30_000);
        assert_eq!(
            out.location.position_source,
            Some(PositionSource::ConfigStatic),
            "fallback to config-static must propagate ConfigStatic provenance"
        );
        // Stale-fix path also returns config-static identity
        let mut fix = fresh_fix(1_000_000);
        fix.fix_at_ms = 1_000_000 - 60_000; // stale
        let stale_swap = ArcSwap::from_pointee(Some(fix));
        let out_stale = stamp_node_with_gps(&s, &stale_swap, 1_000_000, 30_000);
        assert_eq!(
            out_stale.location.position_source,
            Some(PositionSource::ConfigStatic),
            "stale GPS fix must fall back and propagate ConfigStatic provenance"
        );
    }
}
