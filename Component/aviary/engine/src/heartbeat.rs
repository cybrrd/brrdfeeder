// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Heartbeat subject `cybrrd.system.node.heartbeat.<node_id>`.
//!
//! Per Triangulated Architecture consensus 2026-04-29, the heartbeat
//! pairs with the drop-audit subject to give downstream a complete
//! node-health spectrum:
//!
//!   telemetry.frame.rid.<node>          presence  + drone in range
//!   audit.frame.dropped.<node>          partial   degradation
//!   system.node.heartbeat.<node>        liveness  (this subject)
//!   (silence on heartbeat)              full-outage signal
//!
//! Cadence is configurable via `tuning.heartbeat.interval_secs` in
//! `config.yaml` (default 5 s). Payload includes load_avg + radio_status
//! per the reviewer's Wave 6.0e strategic-friction notes — load_avg correlates
//! buffer_full drop events with CPU saturation; radio_status verifies
//! the Wi-Fi/BLE interface is up even when no drones are visible.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use arc_swap::ArcSwap;
use cybrrd_rid_protocol::models::PositionSource;

use crate::hunter::{CoverageClass, HunterState, KITTLER_LINEAGE};
use crate::sensor::{SensorHealth, SensorState};
use crate::sensor_gps::GpsFix;

/// Legacy/test fallback budget. Production Silver v1 uses the configured GPS
/// stale interval and never promotes config_static into current position.
const HEARTBEAT_GPS_FRESH_THRESHOLD_MS: i64 = 30_000;

/// Wire-format radio status. Snake_case strings are the locked contract
/// for globe-frontend's "Master Status" UI mapping.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RadioStatus {
    /// Capture interface is in monitor mode and pcap is open.
    #[serde(rename = "up")]
    Up,
    /// Capture interface is configured but not currently operational.
    #[serde(rename = "down")]
    Down,
    /// Capture failed to initialize (RF silicon error, missing device, etc.).
    #[serde(rename = "error")]
    Error,
    /// Wave 7.1 Inc 8 — the Capture Liveness Watchdog detected a stall
    /// and is actively re-establishing the capture path (re-resolving
    /// ifindex, re-establishing monitor mode, re-opening libpcap). This
    /// is a transient state: it resolves to `up` on successful recovery
    /// or `error` if recovery times out. Its existence is the direct
    /// substrate-honest fix for the 2026-05-13 blind spot — the engine
    /// now has a wire-format word for "I know I'm not capturing and I'm
    /// working on it," which it previously lacked.
    ///
    /// Wire-format note: this variant is a Policy-#2 wire-format
    /// addition. globe-backend's heartbeat consumer must tolerate it
    /// (treat unknown radio_status as non-fatal) before fleet rollout.
    #[serde(rename = "recovering")]
    Recovering,
}

impl From<u8> for RadioStatus {
    fn from(v: u8) -> Self {
        match v {
            1 => RadioStatus::Up,
            2 => RadioStatus::Error,
            3 => RadioStatus::Recovering,
            _ => RadioStatus::Down,
        }
    }
}

impl From<RadioStatus> for u8 {
    fn from(s: RadioStatus) -> u8 {
        match s {
            RadioStatus::Down => 0,
            RadioStatus::Up => 1,
            RadioStatus::Error => 2,
            RadioStatus::Recovering => 3,
        }
    }
}

/// Shared atomic state for the radio status. Capture loop writes; the
/// heartbeat emitter reads. Cloning the wrapper is cheap (Arc).
#[derive(Debug, Clone, Default)]
pub struct RadioState {
    inner: Arc<AtomicU8>,
    forensic_failed: Arc<AtomicBool>,
}

impl RadioState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, status: RadioStatus) {
        self.inner.store(status.into(), Ordering::Relaxed);
    }

    pub fn get(&self) -> RadioStatus {
        if self.forensic_failed.load(Ordering::Relaxed) {
            return RadioStatus::Error;
        }
        RadioStatus::from(self.inner.load(Ordering::Relaxed))
    }
    pub fn set_forensic_failed(&self, failed: bool) {
        self.forensic_failed.store(failed, Ordering::Relaxed);
    }
}

/// Heartbeat payload published every `interval_secs` to
/// `cybrrd.system.node.heartbeat.<node_id>`.
///
/// `timestamp_utc` unit: Unix milliseconds since epoch (Wave 6.4.1
/// substrate-truth lock). Mirrors the `NormalizedTelemetry` wire-
/// format unit so all feeder-emitted timestamps share one semantic.
/// `uptime_seconds` stays in seconds — uptime granularity at second
/// resolution is fine for liveness signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatPayload {
    #[serde(flatten)]
    pub release_currency: crate::release_currency::ReleaseCurrency,
    pub node_id: String,
    pub timestamp_utc: u64,
    pub uptime_seconds: u64,
    /// 1-minute load average from `/proc/loadavg`.
    pub load_avg_1m: f64,
    /// 5-minute load average.
    pub load_avg_5m: f64,
    /// 15-minute load average.
    pub load_avg_15m: f64,
    pub radio_status: RadioStatus,
    /// CPU thermal-zone-0 temperature in degrees C, when readable
    /// (Linux `/sys/class/thermal/thermal_zone0/temp`). Optional because
    /// non-Linux dev hosts and some boards don't expose it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_temp_c: Option<f32>,
    /// Wave 7.1 Hunter Vitals (Kittler Substrate Defense): present
    /// when the Hunter task is active. Pre-Wave-7 bricks emit
    /// heartbeats without this field; downstream consumers must
    /// tolerate its absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunter: Option<HunterVitals>,
    /// Wave 7.3b GPS Vitals: present when the UbloxGps sensor is
    /// spawned. Pre-Wave-7.2 deployments (no sensor) emit heartbeats
    /// without this field. Globe-backend's node-registry UI surfaces
    /// these fields (per the reviewer's 2026-05-26 directive). Self-
    /// Diagnostic path companion to the External-Truth stamping
    /// landed in Wave 7.3a.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gps: Option<GpsVitals>,
    /// Legacy compatibility alias. Silver v1 emits GpsLive only alongside a
    /// current_position; otherwise absent. ConfigStatic remains deserializable
    /// for older heartbeats, but v1 carries configuration separately and makes
    /// no claim that it is a current observation or an attested location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_position_source: Option<PositionSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silver_schema_version: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_position: Option<crate::silver::CurrentPosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_position: Option<crate::silver::ConfiguredPosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_status: Option<crate::silver::PositionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_status: Option<crate::silver::ManagementStatus>,
    // ── #185 Silver fleet-proprioception ──────────────────────────────
    // The node's build/bodily state, reported each heartbeat so System 3
    // (command.cybrrd.com) tracks version convergence without polling.
    /// Cargo's engine package version: the authoritative product version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_version: Option<String>,
    /// Binary-baked source revision only; omitted while no trustworthy source exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_version: Option<String>,
    /// Verified running manifest digest only; omitted pending the runtime handoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_digest: Option<String>,
    /// Trustworthy binary build sequence only; omission is not build zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_seq: Option<u64>,
    /// Release ring: dev | staging | general (default general).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// #183 — is the system clock GPS-disciplined / NTP-trusted? Frames only
    /// publish when true; surfaced here so command can see time-trust state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_clock_trusted: Option<bool>,
    /// Actual policy-application evidence only; never copied from a version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_ack: Option<String>,
    /// D26: explicit update refusal, not convergence. Wrapper proof remains owed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_blocked_reason: Option<String>,
}

/// #185 — fleet-governance proprioception (Silver). The static identity the
/// node reports every heartbeat, plus the live #183 trusted-time handle.
#[derive(Clone)]
pub struct FleetProprioception {
    pub identity: crate::identity::RunningIdentity,
    pub silver: Option<crate::silver::Context>,
    pub channel: String,
    pub time_trust: std::sync::Arc<crate::clock_discipline::TimeTrust>,
}

/// Wave 7.3b — GPS health snapshot for the heartbeat payload.
///
/// Combines two substrate-truth sources:
///   - SensorHandle.health (the sensor's lifecycle state + error counter)
///   - the latest GpsFix (quality / sat count / HDOP from most recent fix)
///
/// Globe-backend's node-registry surface reads these to render Sentinel
/// health in the UI. Fields beyond `state` + `error_count` are Optional
/// because they're absent until the first fix arrives.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpsVitals {
    /// Lifecycle state: "initializing" / "healthy" / "degraded" / "failed".
    /// Serialized as snake_case via SensorState's serde rename.
    pub state: SensorState,
    /// Unix-ms timestamp of last successful reading; absent until first fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reading_ms: Option<i64>,
    /// Most recent NMEA fix quality (0=invalid, 1=GPS, 2=DGPS, 4=RTK-fixed).
    /// Absent until first fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_quality: Option<u8>,
    /// Satellites used in most recent fix. Absent until first fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sat_count: Option<u8>,
    /// Horizontal Dilution of Precision; lower = more accurate. Absent
    /// until first fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdop: Option<f32>,
    /// Recoverable error count since sensor started (parse fails,
    /// transient serial errors). Monotonic; downstream derives rate
    /// by diffing across heartbeats.
    pub error_count: u64,
    /// Sensor-emitted detail context (last error / lock note). Cleared
    /// on Healthy transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Per-channel survey readings + the lineage tag that traces every
/// substrate-attention artifact back to the Kittler / EGS / Chodove
/// provenance. Wire-format full-snapshot (not delta) per Wave 7.1
/// substrate-truth decision: every quiet observation is itself
/// substrate-truth, not absence of data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HunterVitals {
    /// The channel the Hunter is currently dwelled on (or last
    /// successfully set). Zero before first set_channel succeeds.
    pub current_channel: u32,
    /// Provenance tag carried on every substrate-attention artifact.
    /// Always equals `kittler-substrate-defense-v1` for this engine
    /// revision; locked at compile time via `hunter::KITTLER_LINEAGE`.
    pub lineage: String,
    /// Per-channel snapshots for channels in the configured
    /// channel_set. Sorted by channel number so wire output is
    /// stable for downstream diffing.
    pub channels: Vec<ChannelVitals>,
    /// Per-radio capability attestation. Wave 7.1 ships with one
    /// entry (the Wi-Fi monitor adapter, Alfa AWUS036ACS). Wave 7.2
    /// adds the BLE entry (HolyIoT nRF52840). Wave 7.3 adds the SDR
    /// entry (NESDR / discone). Schema is forward-compatible: the
    /// array grows as engine modules for additional radios land.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub radios: Vec<RadioCapability>,
    /// Wave 7.1 Inc 6 — true when the Hunter is currently dwelled on
    /// a channel because of a recent drone-class frame detection
    /// (protocol-neutral, vendor-blind). The sentinel has its gaze.
    #[serde(default)]
    pub lock_on_active: bool,
    /// Total lock-on triggers since engine start. Monotonic counter;
    /// downstream can derive rate by diffing across heartbeats.
    #[serde(default)]
    pub lock_on_triggers_total: u64,
    /// Wave 7.1b — total lock cycles released early via capture-budget
    /// satisfaction (drone_id + position observed within
    /// `lock_on_min_ms`). Monotonic counter. The ratio
    /// `budget_releases_total / triggers_total` is the substrate
    /// signal for "how often is the budget-release saving cycles vs
    /// hitting the max_lock cap" — useful for ops tuning the
    /// min/max defaults under real-world load.
    #[serde(default)]
    pub lock_on_budget_releases_total: u64,
    /// Channel scheduler — the active preset name (social | park6 |
    /// sweep), so field data can group nodes by schedule family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Channel scheduler S2 — measured set_channel round-trip stats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retune: Option<RetuneVitals>,
}

/// Per-radio empirical capability attestation (Wave 7.1 Inc 5.5).
/// Each entry describes one sensor radio's observed coverage based
/// on substrate-truth behavior (set_channel success, survey emptiness,
/// etc.) — NOT on a vendor blocklist. Future-proof against the next
/// chipset that advertises features it doesn't implement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RadioCapability {
    /// What this radio does in the Anastomotic Reticulum. Three
    /// canonical roles in Wave 7.1–7.3: `wifi_monitor`, `ble_sniffer`,
    /// `sdr_wideband`. Free-form string for forward-extensibility.
    pub role: String,
    /// Stable device identifier (interface name, /dev/ alias, or
    /// brrdsupervisor-issued logical ID in Wave 7.5+).
    pub device_alias: String,
    /// Empirically-derived coverage class. Substrate-honest:
    /// observed behavior is the source of truth, not advertised
    /// capability.
    pub coverage_class: CoverageClass,
    /// Optional driver name from /sys/class/net/<iface>/device/driver.
    /// Informational metadata for the Hardware Reliability Index;
    /// not load-bearing for any control logic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_name: Option<String>,
    /// Per-feature observed availability. Each is None when not yet
    /// evaluated (within the sample window) or when the radio does
    /// not implement that feature class. Substrate-truth: derived
    /// from observation, not advertisement.
    pub telemetry_features: TelemetryFeatures,
}

/// Per-radio feature observation map. One field per substrate-truth
/// capability the engine cares about. Each is `Option<bool>` because
/// "not yet evaluated" is itself a substrate-honest answer that
/// downstream consumers must not conflate with "false."
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TelemetryFeatures {
    /// True when set_channel calls succeed. False when channel
    /// rotation has been observed to fail repeatedly. None when not
    /// yet evaluated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_rotation: Option<bool>,
    /// True when get_survey returns ≥1 populated entry during the
    /// sample window. False if consistently empty (rtw88_8812au
    /// substrate-truth lesson 2026-05-10). None during pending
    /// evaluation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noise_floor: Option<bool>,
    /// True when busy_pct computation has produced any non-None
    /// per-channel reading. Currently dependent on noise_floor
    /// substrate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_busy_pct: Option<bool>,
}

/// One channel's noise floor + busy-percent + staleness, as observed
/// at the most recent end-of-dwell survey sample, plus the channel
/// scheduler's dwell accounting (C4): accumulated actual dwell
/// milliseconds, decoded RID observations received on the channel, and
/// the dwell share of total recorded dwell (so field data can prove the
/// schedule improvement and expose any lock-on inflation).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelVitals {
    pub channel: u32,
    /// Last-measured noise floor in dBm. Typical quiet 5GHz UNII-3
    /// is around -97 dBm; values approaching -65 dBm suggest RF
    /// saturation (drone control link, adjacent emitter, or
    /// jamming).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noise_dbm: Option<i8>,
    /// Channel busy percent computed as a delta across consecutive
    /// dwell samples (None until at least two samples exist for
    /// this channel).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy_pct: Option<u8>,
    /// Wall-clock milliseconds elapsed since the kernel survey
    /// that produced these readings was sampled. Carries
    /// substrate-truth about how fresh this data is — downstream
    /// consumers should weight stale samples accordingly.
    pub sample_age_ms: u64,
    /// Accumulated actual dwell milliseconds on this channel
    /// (lock-on extensions included — substrate truth).
    #[serde(default)]
    pub dwell_ms_total: u64,
    /// Decoded RID observations received while dwelling on this channel.
    #[serde(default)]
    pub rid_hits_total: u64,
    /// Dwell share of total recorded dwell across all channels, in
    /// percent. None until any dwell has been recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dwell_share_pct: Option<u8>,
}

/// Measured `set_channel` round-trip statistics (S2): the modelled
/// retune dead time starts as a cited same-silicon proxy and is
/// replaced by these fleet-measured counters.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct RetuneVitals {
    #[serde(default)]
    pub last_ms: u64,
    #[serde(default)]
    pub max_ms: u64,
    #[serde(default)]
    pub count: u64,
}

/// Pure-function loadavg parser, separated from the filesystem read
/// so it's unit-testable without /proc.
pub fn parse_loadavg(s: &str) -> Option<(f64, f64, f64)> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

/// Read /proc/loadavg. Returns None on any read or parse error so the
/// heartbeat emitter can fall back to (0, 0, 0) without panicking.
pub fn read_loadavg() -> Option<(f64, f64, f64)> {
    let s = std::fs::read_to_string("/proc/loadavg").ok()?;
    parse_loadavg(&s)
}

/// Read CPU temperature in degrees C from
/// `/sys/class/thermal/thermal_zone0/temp` (Linux convention,
/// millidegrees Celsius). Returns None on any error.
pub fn read_cpu_temp_c() -> Option<f32> {
    let s = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok()?;
    let mc: i32 = s.trim().parse().ok()?;
    Some(mc as f32 / 1000.0)
}

/// Wall-clock for heartbeat emission timestamps (Wave 6.4.1: ms
/// resolution). See engine/src/capture.rs::now_unix_ms for the
/// substrate-truth rationale on the s→ms unit change.
pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Snapshot the current process state into a HeartbeatPayload. Pure-ish
/// (calls into /proc + /sys) so the emitter loop stays trivial.
///
/// `hunter_state` is optional: when `Some`, the payload includes the
/// Wave 7.1 Hunter Vitals block (current channel + per-channel noise
/// floor + busy %); when `None`, the field is omitted entirely (pre-
/// Wave-7 compatibility).
pub fn build_payload(
    node_id: &str,
    start: Instant,
    radio: &RadioState,
    hunter_state: Option<&Arc<HunterState>>,
    wifi_iface: Option<&str>,
    // Channel scheduler: the active preset name (social | park6 | sweep),
    // surfaced in the hunter block so field data groups nodes by family.
    hunter_preset: Option<String>,
    // Wave 7.3b: GPS health + latest-fix surfaces. Both Optional so
    // pre-Wave-7.2 deployments without a UbloxGps sensor still
    // produce heartbeats unchanged.
    gps_health: Option<&Arc<ArcSwap<SensorHealth>>>,
    gps_latest: Option<&Arc<ArcSwap<Option<GpsFix>>>>,
    // #185 Silver fleet-proprioception. None in unit tests / pre-fleet builds.
    fleet: Option<&FleetProprioception>,
) -> HeartbeatPayload {
    let (load_1m, load_5m, load_15m) = read_loadavg().unwrap_or((0.0, 0.0, 0.0));
    let now = now_unix_ms();
    let hunter = hunter_state.map(|state| {
        let (current_channel, channel_entries) = state.snapshot();
        let coverage_class = state.coverage_class();
        let total_dwell: u64 = channel_entries.iter().map(|(_, s)| s.dwell_ms_total).sum();
        let radios = if let Some(iface) = wifi_iface {
            vec![RadioCapability {
                role: "wifi_monitor".to_string(),
                device_alias: iface.to_string(),
                coverage_class,
                driver_name: read_driver_name(iface),
                telemetry_features: telemetry_features_for(coverage_class),
            }]
        } else {
            Vec::new()
        };
        HunterVitals {
            current_channel,
            lineage: KITTLER_LINEAGE.to_string(),
            channels: channel_entries
                .iter()
                .map(|(channel, snap)| ChannelVitals {
                    channel: *channel,
                    noise_dbm: snap.noise_dbm,
                    busy_pct: snap.busy_pct,
                    sample_age_ms: now.saturating_sub(snap.last_sample_unix_ms),
                    dwell_ms_total: snap.dwell_ms_total,
                    rid_hits_total: snap.rid_hits_total,
                    dwell_share_pct: if total_dwell > 0 {
                        Some((snap.dwell_ms_total * 100 / total_dwell).min(100) as u8)
                    } else {
                        None
                    },
                })
                .collect(),
            radios,
            lock_on_active: state.lock_on_active(),
            lock_on_triggers_total: state
                .lock_on_triggers
                .load(std::sync::atomic::Ordering::Relaxed),
            lock_on_budget_releases_total: state
                .lock_on_budget_releases
                .load(std::sync::atomic::Ordering::Relaxed),
            preset: hunter_preset.clone(),
            retune: Some(RetuneVitals {
                last_ms: state
                    .retune_last_ms
                    .load(std::sync::atomic::Ordering::Relaxed),
                max_ms: state
                    .retune_max_ms
                    .load(std::sync::atomic::Ordering::Relaxed),
                count: state
                    .retune_count
                    .load(std::sync::atomic::Ordering::Relaxed),
            }),
        }
    });
    let mut gps = build_gps_vitals(gps_health, gps_latest);
    // One GPS snapshot supplies position and source; never substitute installation data.
    let snapshot = gps_latest.map(|latest| latest.load_full());
    let fix = snapshot.as_ref().and_then(|f| f.as_ref().as_ref());
    if let Some(vitals) = gps.as_mut() {
        // Keep health metadata on the same fix snapshot used for coordinates.
        vitals.fix_quality = fix.map(|f| f.fix_quality);
        vitals.sat_count = fix.map(|f| f.sat_count);
        vitals.hdop = fix.map(|f| f.hdop);
    }
    let trusted = fleet.is_some_and(|f| f.time_trust.is_trusted());
    let context = fleet.and_then(|f| f.silver.as_ref());
    let (current_position, position_status) = crate::silver::position(
        fix,
        gps.as_ref().map(|g| g.state),
        trusted,
        now as i64,
        context.map_or(HEARTBEAT_GPS_FRESH_THRESHOLD_MS, |c| c.fresh_for_ms),
    );
    let node_position_source = current_position.as_ref().map(|_| PositionSource::GpsLive);
    let blocked_reason = fleet
        .and_then(|f| f.identity.blocked_reason())
        .map(str::to_owned);
    let management_status = context.map(|_| {
        crate::silver::management(
            blocked_reason.as_deref(),
            position_status.state,
            radio.get() == RadioStatus::Up,
            trusted,
        )
    });
    let (engine_version, image_digest, build_seq, channel, os_clock_trusted, policy_ack) =
        match fleet {
            Some(f) => (
                f.identity.engine_version.clone(),
                f.identity.image_digest.clone(),
                f.identity.build_seq,
                Some(f.channel.clone()),
                Some(f.time_trust.is_trusted()),
                // Version equality is not a policy-application receipt (PRV 5.4).
                None,
            ),
            None => (None, None, None, None, None, None),
        };
    HeartbeatPayload {
        release_currency: crate::release_currency::ReleaseCurrency::read(
            std::path::Path::new("/var/lib/brrdfeeder/release_currency.json"),
            node_id,
            image_digest.as_deref(),
        ),
        node_id: node_id.to_string(),
        timestamp_utc: now,
        uptime_seconds: start.elapsed().as_secs(),
        load_avg_1m: load_1m,
        load_avg_5m: load_5m,
        load_avg_15m: load_15m,
        radio_status: radio.get(),
        cpu_temp_c: read_cpu_temp_c(),
        hunter,
        gps,
        node_position_source,
        silver_schema_version: context.map(|_| 1),
        current_position,
        configured_position: context.map(|c| c.configured_position.clone()),
        position_status: context.map(|_| position_status),
        config_hash: context.and_then(|c| c.config_hash.clone()),
        management_status,
        product_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        engine_version,
        image_digest,
        build_seq,
        channel,
        os_clock_trusted,
        policy_ack,
        update_blocked_reason: blocked_reason,
    }
}

/// Wave 7.3b helper: build a GpsVitals snapshot from the sensor's two
/// shared-state handles. Returns None when no GPS sensor is configured
/// (both handles are None) — produces backward-compatible heartbeats.
fn build_gps_vitals(
    gps_health: Option<&Arc<ArcSwap<SensorHealth>>>,
    gps_latest: Option<&Arc<ArcSwap<Option<GpsFix>>>>,
) -> Option<GpsVitals> {
    let health_handle = gps_health?;
    let health = health_handle.load();
    // Pull fix-quality metadata IF a fix is present. The sensor's
    // health snapshot tells us the lifecycle state; the latest-fix
    // swap tells us the most recent quality numbers. Both are
    // lock-free atomic reads.
    //
    // ArcSwap deref pattern: `&**guard` walks Guard → &Arc<Option<GpsFix>>
    // → &Option<GpsFix>, which we then pattern-match without moving.
    let (fix_quality, sat_count, hdop) = if let Some(arc) = gps_latest {
        let guard = arc.load();
        let opt_ref: &Option<GpsFix> = &**guard;
        if let Some(fix) = opt_ref {
            (Some(fix.fix_quality), Some(fix.sat_count), Some(fix.hdop))
        } else {
            (None, None, None)
        }
    } else {
        (None, None, None)
    };
    Some(GpsVitals {
        state: health.state,
        last_reading_ms: health.last_reading_ms,
        fix_quality,
        sat_count,
        hdop,
        error_count: health.error_count,
        detail: health.detail.clone(),
    })
}

/// Resolve the driver name backing a Wi-Fi interface by reading the
/// /sys/class/net/<iface>/device/driver symlink target's basename.
/// Pure-safe-Rust; returns None on any I/O error or non-Linux host.
fn read_driver_name(iface: &str) -> Option<String> {
    let path = format!("/sys/class/net/{}/device/driver", iface);
    let target = std::fs::read_link(&path).ok()?;
    target.file_name().map(|n| n.to_string_lossy().to_string())
}

/// Derive the wire-format TelemetryFeatures triple from a CoverageClass.
/// Substrate-honest: Pending leaves all three as None ("not yet
/// evaluated"); LogicOnly means channel_rotation=true but
/// noise_floor/busy_pct=false; FullSigint means all three true;
/// RadioError means channel_rotation=false (downstream cascades).
fn telemetry_features_for(c: CoverageClass) -> TelemetryFeatures {
    match c {
        CoverageClass::Pending => TelemetryFeatures::default(),
        CoverageClass::LogicOnly => TelemetryFeatures {
            channel_rotation: Some(true),
            noise_floor: Some(false),
            channel_busy_pct: Some(false),
        },
        CoverageClass::FullSigint => TelemetryFeatures {
            channel_rotation: Some(true),
            noise_floor: Some(true),
            channel_busy_pct: Some(true),
        },
        CoverageClass::RadioError => TelemetryFeatures {
            channel_rotation: Some(false),
            noise_floor: Some(false),
            channel_busy_pct: Some(false),
        },
    }
}

/// Emitter task: tick every `interval_secs`, publish a heartbeat payload.
/// `publish` is injected so unit tests can substitute a sink — and so the
/// emitter doesn't have to know whether NATS is reachable.
///
/// `hunter_state` is `None` when the Hunter is disabled in config; the
/// emitter then omits the `hunter` block from heartbeats entirely.
pub async fn run_heartbeat_emitter<P, Fut>(
    node_id: String,
    radio: RadioState,
    hunter_state: Option<Arc<HunterState>>,
    wifi_iface: Option<String>,
    // Channel scheduler: active preset name (social | park6 | sweep).
    hunter_preset: Option<String>,
    // Wave 7.3b: GPS surfaces — health swap + latest-fix swap. None
    // when no UbloxGps sensor was spawned (pre-Wave-7.2 deployments).
    gps_health: Option<Arc<ArcSwap<SensorHealth>>>,
    gps_latest: Option<Arc<ArcSwap<Option<GpsFix>>>>,
    // #185 Silver fleet-proprioception (engine_version / channel / clock-trust).
    fleet: FleetProprioception,
    interval_secs: u64,
    status: Option<crate::status::StatusSource>,
    mut publish: P,
) where
    P: FnMut(String, Vec<u8>) -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    let subject = format!("cybrrd.system.node.heartbeat.{}", node_id);
    let start = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs.max(1)));
    // First tick fires immediately; we want the first heartbeat at +interval, not 0.
    ticker.tick().await;
    // Observe meaningful changes at 1 Hz without changing NATS heartbeat cadence.
    // Disabled status has no additional polling branch or payload construction.
    let mut status_ticker = tokio::time::interval(Duration::from_secs(1));
    status_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    status_ticker.tick().await;
    loop {
        let publish_due = tokio::select! {
            biased;
            _ = ticker.tick() => true,
            _ = status_ticker.tick(), if status.is_some() => false,
        };
        let payload = build_payload(
            &node_id,
            start,
            &radio,
            hunter_state.as_ref(),
            wifi_iface.as_deref(),
            hunter_preset.clone(),
            gps_health.as_ref(),
            gps_latest.as_ref(),
            Some(&fleet),
        );
        if publish_due {
            if let Ok(bytes) = serde_json::to_vec(&payload) {
                publish(subject.clone(), bytes).await;
            }
        }
        if let Some(source) = &status {
            if let Err(e) = source.write(&payload).await {
                eprintln!("[status] local snapshot write failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Channel-scheduler acceptance C4 (red on the pre-scheduler heartbeat):
    /// the hunter block's per-channel vitals must report dwell accounting
    /// (accumulated dwell milliseconds and RID frames heard) so field data
    /// can prove the schedule improvement. The pre-scheduler ChannelVitals
    /// carries neither field and fails this assertion.
    #[test]
    fn channel_vitals_report_dwell_and_hits() {
        let v = ChannelVitals {
            channel: 6,
            noise_dbm: None,
            busy_pct: None,
            sample_age_ms: 42,
            dwell_ms_total: 0,
            rid_hits_total: 0,
            dwell_share_pct: None,
        };
        let json = serde_json::to_value(&v).unwrap();
        assert!(
            json.get("dwell_ms_total").is_some(),
            "ChannelVitals must carry dwell_ms_total (accumulated dwell)"
        );
        assert!(
            json.get("rid_hits_total").is_some(),
            "ChannelVitals must carry rid_hits_total (frames heard on channel)"
        );
    }

    #[test]
    fn parses_canonical_proc_loadavg_line() {
        let s = "0.42 0.31 0.22 1/256 12345\n";
        let (a, b, c) = parse_loadavg(s).unwrap();
        assert!((a - 0.42).abs() < 1e-9);
        assert!((b - 0.31).abs() < 1e-9);
        assert!((c - 0.22).abs() < 1e-9);
    }

    #[test]
    fn parse_loadavg_handles_malformed_input() {
        assert!(parse_loadavg("").is_none());
        assert!(parse_loadavg("not a number").is_none());
        assert!(parse_loadavg("0.1 0.2").is_none()); // only 2 fields
        assert!(parse_loadavg("a b c").is_none());
    }

    #[test]
    fn radio_status_round_trips_through_atomic() {
        let s = RadioState::new();
        assert_eq!(s.get(), RadioStatus::Down);
        s.set(RadioStatus::Up);
        assert_eq!(s.get(), RadioStatus::Up);
        s.set(RadioStatus::Error);
        assert_eq!(s.get(), RadioStatus::Error);
        // Wave 7.1 Inc 8 — the transient watchdog state must survive
        // the AtomicU8 round-trip too.
        s.set(RadioStatus::Recovering);
        assert_eq!(s.get(), RadioStatus::Recovering);
    }

    #[test]
    fn radio_status_serializes_to_canonical_wire_names() {
        // Front-end "Master Status" UI is contracted on these strings —
        // renaming any of them is a breaking wire change.
        for (status, expected) in [
            (RadioStatus::Up, "\"up\""),
            (RadioStatus::Down, "\"down\""),
            (RadioStatus::Error, "\"error\""),
            (RadioStatus::Recovering, "\"recovering\""),
        ] {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(
                json, expected,
                "{:?} should serialize to {}",
                status, expected
            );
        }
    }

    #[test]
    fn heartbeat_payload_serializes_with_all_canonical_fields() {
        // Wave 6.4.1: timestamp_utc is now Unix milliseconds.
        // 1714342400 (s) → 1714342400000 (ms).
        let payload = HeartbeatPayload {
            release_currency: Default::default(),
            node_id: "bf-test-001".into(),
            timestamp_utc: 1714342400000,
            uptime_seconds: 3600,
            load_avg_1m: 0.42,
            load_avg_5m: 0.31,
            load_avg_15m: 0.22,
            radio_status: RadioStatus::Up,
            cpu_temp_c: Some(48.5),
            hunter: None,
            gps: None,
            node_position_source: None,
            silver_schema_version: None,
            current_position: None,
            configured_position: None,
            position_status: None,
            config_hash: None,
            management_status: None,
            product_version: None,
            engine_version: None,
            image_digest: None,
            build_seq: None,
            channel: None,
            os_clock_trusted: None,
            policy_ack: None,
            update_blocked_reason: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        for field in &[
            "\"node_id\":\"bf-test-001\"",
            "\"timestamp_utc\":1714342400000",
            "\"uptime_seconds\":3600",
            "\"load_avg_1m\":0.42",
            "\"load_avg_5m\":0.31",
            "\"load_avg_15m\":0.22",
            "\"radio_status\":\"up\"",
            "\"cpu_temp_c\":48.5",
        ] {
            assert!(json.contains(field), "missing {} in {}", field, json);
        }
    }

    #[test]
    fn heartbeat_payload_omits_cpu_temp_when_absent() {
        let payload = HeartbeatPayload {
            release_currency: Default::default(),
            node_id: "bf-test-001".into(),
            timestamp_utc: 0,
            uptime_seconds: 0,
            load_avg_1m: 0.0,
            load_avg_5m: 0.0,
            load_avg_15m: 0.0,
            radio_status: RadioStatus::Down,
            cpu_temp_c: None,
            hunter: None,
            gps: None,
            node_position_source: None,
            silver_schema_version: None,
            current_position: None,
            configured_position: None,
            position_status: None,
            config_hash: None,
            management_status: None,
            product_version: None,
            engine_version: None,
            image_digest: None,
            build_seq: None,
            channel: None,
            os_clock_trusted: None,
            policy_ack: None,
            update_blocked_reason: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("cpu_temp_c"), "should be skipped: {}", json);
    }

    #[test]
    fn heartbeat_payload_omits_hunter_when_disabled() {
        // Backward-compat invariant: pre-Wave-7 deployments emit
        // heartbeats with no `hunter` field; downstream parsers
        // (globe-backend Wave 6.2g+) must continue to accept that.
        let payload = HeartbeatPayload {
            release_currency: Default::default(),
            node_id: "bf-pre-w7".into(),
            timestamp_utc: 0,
            uptime_seconds: 0,
            load_avg_1m: 0.0,
            load_avg_5m: 0.0,
            load_avg_15m: 0.0,
            radio_status: RadioStatus::Up,
            cpu_temp_c: None,
            hunter: None,
            gps: None,
            node_position_source: None,
            silver_schema_version: None,
            current_position: None,
            configured_position: None,
            position_status: None,
            config_hash: None,
            management_status: None,
            product_version: None,
            engine_version: None,
            image_digest: None,
            build_seq: None,
            channel: None,
            os_clock_trusted: None,
            policy_ack: None,
            update_blocked_reason: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(
            !json.contains("hunter"),
            "hunter field should be absent: {}",
            json
        );
    }

    #[test]
    fn heartbeat_payload_includes_hunter_when_enabled() {
        // Wave 7.1 wire-format contract: when hunter is Some, the
        // emitted JSON carries current_channel + lineage + per-channel
        // noise_dbm + busy_pct + sample_age_ms.
        let payload = HeartbeatPayload {
            release_currency: Default::default(),
            node_id: "bf-00000003".into(),
            timestamp_utc: 1714342400000,
            uptime_seconds: 60,
            load_avg_1m: 0.0,
            load_avg_5m: 0.0,
            load_avg_15m: 0.0,
            radio_status: RadioStatus::Up,
            cpu_temp_c: None,
            hunter: Some(HunterVitals {
                current_channel: 149,
                lineage: "kittler-substrate-defense-v1".to_string(),
                channels: vec![
                    ChannelVitals {
                        channel: 6,
                        noise_dbm: Some(-91),
                        busy_pct: Some(23),
                        sample_age_ms: 2400,
                        dwell_ms_total: 0,
                        rid_hits_total: 0,
                        dwell_share_pct: None,
                    },
                    ChannelVitals {
                        channel: 149,
                        noise_dbm: Some(-97),
                        busy_pct: Some(4),
                        sample_age_ms: 80,
                        dwell_ms_total: 0,
                        rid_hits_total: 0,
                        dwell_share_pct: None,
                    },
                ],
                radios: vec![RadioCapability {
                    role: "wifi_monitor".to_string(),
                    device_alias: "wlx00c0caa697b3".to_string(),
                    coverage_class: CoverageClass::FullSigint,
                    driver_name: Some("rtw88_8812au".to_string()),
                    telemetry_features: TelemetryFeatures {
                        channel_rotation: Some(true),
                        noise_floor: Some(true),
                        channel_busy_pct: Some(true),
                    },
                }],
                lock_on_active: false,
                lock_on_triggers_total: 0,
                lock_on_budget_releases_total: 0,
                preset: None,
                retune: None,
            }),
            gps: None,
            node_position_source: None,
            silver_schema_version: None,
            current_position: None,
            configured_position: None,
            position_status: None,
            config_hash: None,
            management_status: None,
            product_version: None,
            engine_version: None,
            image_digest: None,
            build_seq: None,
            channel: None,
            os_clock_trusted: None,
            policy_ack: None,
            update_blocked_reason: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        for field in &[
            "\"current_channel\":149",
            "\"lineage\":\"kittler-substrate-defense-v1\"",
            "\"channel\":6",
            "\"channel\":149",
            "\"noise_dbm\":-91",
            "\"noise_dbm\":-97",
            "\"busy_pct\":23",
            "\"busy_pct\":4",
            "\"sample_age_ms\":2400",
            "\"sample_age_ms\":80",
            // Wave 7.1 Inc 5.5 — RadioCapability attestation wire fields
            "\"role\":\"wifi_monitor\"",
            "\"device_alias\":\"wlx00c0caa697b3\"",
            "\"coverage_class\":\"full_sigint\"",
            "\"driver_name\":\"rtw88_8812au\"",
            "\"channel_rotation\":true",
            "\"noise_floor\":true",
            "\"channel_busy_pct\":true",
        ] {
            assert!(json.contains(field), "missing {} in {}", field, json);
        }
    }

    #[test]
    fn coverage_class_wire_serialization_is_snake_case() {
        // Locks the wire-format string contract for downstream
        // consumers (Bluejay TUI chip labels, Hardware Reliability
        // Index queries).
        assert_eq!(
            serde_json::to_string(&CoverageClass::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&CoverageClass::LogicOnly).unwrap(),
            "\"logic_only\""
        );
        assert_eq!(
            serde_json::to_string(&CoverageClass::FullSigint).unwrap(),
            "\"full_sigint\""
        );
        assert_eq!(
            serde_json::to_string(&CoverageClass::RadioError).unwrap(),
            "\"radio_error\""
        );
    }

    #[test]
    fn telemetry_features_derived_from_coverage_class() {
        // Substrate-truth: Pending leaves all None (not yet evaluated).
        let pending = telemetry_features_for(CoverageClass::Pending);
        assert_eq!(pending, TelemetryFeatures::default());
        assert!(pending.channel_rotation.is_none());

        // LogicOnly: hops work, no noise floor.
        let logic = telemetry_features_for(CoverageClass::LogicOnly);
        assert_eq!(logic.channel_rotation, Some(true));
        assert_eq!(logic.noise_floor, Some(false));
        assert_eq!(logic.channel_busy_pct, Some(false));

        // FullSigint: everything works.
        let full = telemetry_features_for(CoverageClass::FullSigint);
        assert_eq!(full.channel_rotation, Some(true));
        assert_eq!(full.noise_floor, Some(true));
        assert_eq!(full.channel_busy_pct, Some(true));
    }

    // ── Wave 7.3b — GpsVitals + build_gps_vitals ───────────────────────

    #[test]
    fn build_gps_vitals_returns_none_when_no_gps_sensor() {
        // Pre-Wave-7.2 deployments don't spawn a UbloxGps — heartbeat
        // emitter passes None for both handles. Result must be None so
        // the wire payload omits the gps field entirely (backward compat).
        let v = build_gps_vitals(None, None);
        assert!(v.is_none());
    }

    #[test]
    fn build_gps_vitals_carries_state_with_no_fix_metadata_when_initializing() {
        // Wave 7.2 sensor spawned but no fix yet (cold start) — health
        // is Initializing, latest-fix swap holds None. Wire payload
        // should carry state + error_count but omit fix_quality/sats/hdop.
        let health = Arc::new(ArcSwap::from_pointee(SensorHealth::initializing(
            "gps:u-blox-7",
        )));
        let latest: Arc<ArcSwap<Option<GpsFix>>> = Arc::new(ArcSwap::from_pointee(None));
        let v = build_gps_vitals(Some(&health), Some(&latest)).expect("expected Some");
        assert_eq!(v.state, SensorState::Initializing);
        assert!(v.last_reading_ms.is_none());
        assert!(v.fix_quality.is_none());
        assert!(v.sat_count.is_none());
        assert!(v.hdop.is_none());
        assert_eq!(v.error_count, 0);
        assert!(v.detail.is_none());
    }

    #[test]
    fn build_gps_vitals_carries_full_payload_when_healthy_with_fix() {
        // Sensor is Healthy and a fix is in the swap — heartbeat surface
        // includes state + fix quality fields. Substrate-truth shape
        // for what globe-backend's node-registry UI will render.
        let mut health_data = SensorHealth::initializing("gps:u-blox-7");
        health_data.state = SensorState::Healthy;
        health_data.last_reading_ms = Some(1_700_000_000_000);
        health_data.error_count = 3;
        let health = Arc::new(ArcSwap::from_pointee(health_data));
        let fix = GpsFix {
            lat: 40.882625,
            lon: -95.691057,
            alt_m: 352.1,
            fix_quality: 1,
            sat_count: 6,
            hdop: 1.27,
            fix_at_ms: 1_700_000_000_000,
            gps_utc_ms: Some(1_700_000_000_000),
        };
        let latest = Arc::new(ArcSwap::from_pointee(Some(fix)));
        let v = build_gps_vitals(Some(&health), Some(&latest)).expect("expected Some");
        assert_eq!(v.state, SensorState::Healthy);
        assert_eq!(v.last_reading_ms, Some(1_700_000_000_000));
        assert_eq!(v.fix_quality, Some(1));
        assert_eq!(v.sat_count, Some(6));
        assert!((v.hdop.unwrap() - 1.27).abs() < 1e-3);
        assert_eq!(v.error_count, 3);
    }

    /// Wave 7.3b wire-format contract: GpsVitals serializes with
    /// snake_case state strings (so globe-backend's UI mapping is
    /// stable), and Optional fields are skipped when None.
    #[test]
    fn heartbeat_payload_with_gps_serializes_to_wire_contract() {
        let payload = HeartbeatPayload {
            release_currency: Default::default(),
            node_id: "bf-00000003".into(),
            timestamp_utc: 1714342400000,
            uptime_seconds: 60,
            load_avg_1m: 0.0,
            load_avg_5m: 0.0,
            load_avg_15m: 0.0,
            radio_status: RadioStatus::Up,
            cpu_temp_c: None,
            hunter: None,
            gps: Some(GpsVitals {
                state: SensorState::Healthy,
                last_reading_ms: Some(1714342395000),
                fix_quality: Some(1),
                sat_count: Some(6),
                hdop: Some(1.27),
                error_count: 0,
                detail: None,
            }),
            node_position_source: Some(PositionSource::GpsLive),
            silver_schema_version: None,
            current_position: None,
            configured_position: None,
            position_status: None,
            config_hash: None,
            management_status: None,
            product_version: None,
            engine_version: None,
            image_digest: None,
            build_seq: None,
            channel: None,
            os_clock_trusted: None,
            policy_ack: None,
            update_blocked_reason: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        // Substrate-truth contract checks (locked):
        for field in &[
            "\"gps\":",
            "\"state\":\"healthy\"", // snake_case via SensorState rename
            "\"last_reading_ms\":1714342395000",
            "\"fix_quality\":1",
            "\"sat_count\":6",
            "\"hdop\":1.27",
            "\"error_count\":0",
        ] {
            assert!(json.contains(field), "missing {} in {}", field, json);
        }
        // Optional `detail` was None — must be absent from wire.
        assert!(
            !json.contains("\"detail\""),
            "detail should be skipped when None"
        );
    }

    /// Backward compatibility: payload with gps=None must NOT carry
    /// the gps field on the wire (skip_serializing_if). Globe-backend's
    /// pre-Wave-7.2 heartbeat consumer would parse a payload identical
    /// to what it already accepts.
    #[test]
    fn heartbeat_payload_without_gps_omits_field_entirely() {
        let payload = HeartbeatPayload {
            release_currency: Default::default(),
            node_id: "bf-pre-w72".into(),
            timestamp_utc: 1714342400000,
            uptime_seconds: 1,
            load_avg_1m: 0.0,
            load_avg_5m: 0.0,
            load_avg_15m: 0.0,
            radio_status: RadioStatus::Up,
            cpu_temp_c: None,
            hunter: None,
            gps: None,
            node_position_source: None,
            silver_schema_version: None,
            current_position: None,
            configured_position: None,
            position_status: None,
            config_hash: None,
            management_status: None,
            product_version: None,
            engine_version: None,
            image_digest: None,
            build_seq: None,
            channel: None,
            os_clock_trusted: None,
            policy_ack: None,
            update_blocked_reason: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(
            !json.contains("\"gps\""),
            "gps field must be absent when None"
        );
    }
}

#[cfg(test)]
mod wave_7_4_tests {
    use super::*;

    /// Wire-format contract: snake_case serialization for the trust gate.
    /// This is the literal string globe-backend Reputation Gravity matches on.
    #[test]
    fn heartbeat_payload_node_position_source_serializes_snake_case() {
        let payload = HeartbeatPayload {
            release_currency: Default::default(),
            node_id: "bf-00000003".into(),
            timestamp_utc: 1714342400000,
            uptime_seconds: 60,
            load_avg_1m: 0.0,
            load_avg_5m: 0.0,
            load_avg_15m: 0.0,
            radio_status: RadioStatus::Up,
            cpu_temp_c: None,
            hunter: None,
            gps: None,
            node_position_source: Some(PositionSource::GpsLive),
            silver_schema_version: None,
            current_position: None,
            configured_position: None,
            position_status: None,
            config_hash: None,
            management_status: None,
            product_version: None,
            engine_version: None,
            image_digest: None,
            build_seq: None,
            channel: None,
            os_clock_trusted: None,
            policy_ack: None,
            update_blocked_reason: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(
            json.contains("\"node_position_source\":\"gps_live\""),
            "expected node_position_source=\"gps_live\" in {}",
            json
        );

        let payload2 = HeartbeatPayload {
            release_currency: Default::default(),
            node_position_source: Some(PositionSource::ConfigStatic),
            ..payload
        };
        let json2 = serde_json::to_string(&payload2).unwrap();
        assert!(
            json2.contains("\"node_position_source\":\"config_static\""),
            "expected node_position_source=\"config_static\" in {}",
            json2
        );
    }

    /// #185 — lock the Silver fleet-proprioception wire-format. The
    /// command.cybrrd.com daemon (System 3) keys off these EXACT snake_case
    /// names to track version convergence; renaming any is a breaking change.
    #[test]
    fn silver_fleet_fields_serialize_with_locked_names() {
        let fleet = FleetProprioception {
            identity: crate::identity::RunningIdentity::unverified(),
            silver: None,
            channel: "rc".to_string(),
            // A fresh TimeTrust is untrusted — proves os_clock_trusted rides the wire.
            time_trust: std::sync::Arc::new(crate::clock_discipline::TimeTrust::new()),
        };
        let payload = build_payload(
            "bf-silver-001",
            Instant::now(),
            &RadioState::new(),
            None,
            None,
            None,
            None,
            None,
            Some(&fleet),
        );
        let json = serde_json::to_string(&payload).expect("serialize");
        assert!(
            json.contains(&format!(
                "\"product_version\":\"{}\"",
                env!("CARGO_PKG_VERSION")
            )),
            "product version did not come from the engine package: {json}"
        );
        for key in ["engine_version", "image_digest", "build_seq", "policy_ack"] {
            assert!(!json.contains(key), "unverified field published: {json}");
        }
        assert!(
            json.contains("\"update_blocked_reason\":\"running_identity_unverified\""),
            "Silver refusal missing: {json}"
        );
        assert!(json.contains("\"channel\":\"rc\""), "got: {}", json);
        assert!(json.contains("\"os_clock_trusted\":false"), "got: {}", json);
    }
}
