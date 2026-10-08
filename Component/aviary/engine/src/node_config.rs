// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Engine-side reader for the `node:` and `capture:` sections of `config.yaml`.
//!
//! Kept small and engine-local so the protocol crate stays pure types and the
//! orchestrator's full config structure doesn't have to be cross-imported.
//! Schema is the same `config.yaml` the orchestrator reads — we just consume
//! the subset relevant to the capture loop and the wire-format `Node` block.

use cybrrd_rid_protocol::models::{Node, NodeLocation, PositionSource};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DEFAULT_NODE_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "-ce");

// REQ-BRRD-003 — config surface honesty: every struct below carries
// `deny_unknown_fields`, so a config.yaml key the engine does not consume is a
// LOAD-TIME ERROR naming the key, never a silent no-op. The config surface is
// a capability claim; the substrate refuses the lie (Executable Contracts §3.4).
// D1 explicitly accepts two installer-legacy keys with warnings; all other
// unknown fields remain errors.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// Derived from the exact bytes parsed by load(); never accepted from YAML.
    #[serde(skip)]
    pub loaded_config_hash: Option<String>,
    pub node: NodeYaml,
    pub capture: CaptureYaml,
    pub backhaul: BackhaulYaml,
    #[serde(default)]
    pub tuning: TuningYaml,
    /// Wave 7.4 (2026-05-26): non-Wi-Fi sensor manifold config. Defaults
    /// preserve pre-Wave-7.2 behavior for any config.yaml that omits the
    /// `sensors:` block entirely.
    #[serde(default)]
    pub sensors: SensorsYaml,
    /// D40: explicit opt-in, grants/streams are separate deployment proposals.
    pub upward: Option<crate::upward::Config>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeYaml {
    pub id: String,
    /// ADR 0007 tier 1: absent/null disables local status; explicit path enables.
    #[serde(default, deserialize_with = "status_path")]
    pub status_file: Option<std::path::PathBuf>,
    #[serde(default = "default_status_interval_secs")]
    pub status_interval_secs: u64,
    /// Installer-only legacy key; accepted with an engine deprecation warning.
    pub storage_class: Option<String>,
    pub location: NodeLocationYaml,
    /// Optional version override; if absent, defaults to crate version + "-ce".
    pub version: Option<String>,
    /// D44 ring: dev | staging | general; absent defaults to general.
    /// The signed release's ring must match this installer-owned assignment.
    pub channel: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeLocationYaml {
    pub latitude: f64,
    pub longitude: f64,
    pub elevation_meters: f32,
}

fn default_status_interval_secs() -> u64 {
    30
}

fn status_path<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<std::path::PathBuf>, D::Error> {
    let value = serde_yaml::Value::deserialize(deserializer)?;
    match value {
        serde_yaml::Value::Null => Ok(None),
        serde_yaml::Value::String(text) => {
            let path = std::path::PathBuf::from(&text);
            if path.is_absolute() && path.file_name().is_some() && !text.contains('\0') {
                Ok(Some(path))
            } else {
                Err(serde::de::Error::custom(
                    "node.status_file must be an absolute file path",
                ))
            }
        }
        _ => Err(serde::de::Error::custom(
            "node.status_file must be a string absolute file path or null",
        )),
    }
}

impl NodeYaml {
    pub fn effective_status_interval_secs(&self, heartbeat_secs: u64) -> u64 {
        self.status_interval_secs.max(heartbeat_secs.max(1))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureYaml {
    pub interface: String,
    /// Disabled by default. When enabled, a private size/age-bounded pcap ring.
    pub savefile: Option<SavefileYaml>,
    /// Wave 7.1 Hunter (Kittler Substrate Defense) — channel rotation
    /// state machine. Disabled by default for backward compat with
    /// pre-Wave-7 deployments where the operator manually pre-locks
    /// the interface to a single channel before engine start.
    #[serde(default)]
    pub hunter: HunterYaml,
}

/// Hunter task config (channel scheduler).
///
/// WHAT: `preset` selects the schedule family; every other field is an
/// OPTIONAL explicit override. `social` (the default) gives channels 6 and
/// 149 long dwells (1200 ms: two NAN discovery-window periods and a ≥1 s
/// `BUR0010` Beacon interval) while every other configured channel still
/// gets a short, fair 220 ms visit each supercycle, all jittered ±15% so
/// no transmitter cadence can phase-lock against us. `park6` parks the
/// first social channel (dense single-band sites). `sweep` reproduces the
/// legacy 200/400 ms rotation values (spectrum-witness missions).
///
/// WHY: the receiver's objective is the SET of aircraft in range. The
/// 200/400 ms rotation gave ch 6 and 149 only ~10% of wall time and turned
/// the ≥1 Hz broadcast cadence into ~4–5 s observation gaps; dwell-to-follow
/// made the node a single-aircraft tracker. See `engine/src/schedule.rs`.
///
/// WHEN-to-tune: raise `dwell_priority_ms` if field heartbeat data
/// (`hunter.channels[].dwell_share_pct`, `rid_hits_total`) shows NAN
/// windows missed at visit edges; lower `dwell_default_ms` only if sweep
/// starvation hurts more than social yield; set `lock_on_duration_ms > 0`
/// ONLY for single-target audit missions — it is off in every preset.
///
/// DEPENDS-ON: `BWF0090` (NAN ch 6/149), `BWFB0030` (Beacon social
/// channels), `BUR0010` (≥1 Hz dynamic), `BWFB0040/0050` (200 TU
/// any-channel Beacon interval). Explicit values always win over preset
/// defaults (back-compat); `HunterPlan::from_yaml` resolves and records
/// which fields were explicit.
///
/// When `enabled: false`, no channel rotation occurs — the engine expects
/// the operator (or a startup systemd unit) to have already locked the
/// interface to its operating channel.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HunterYaml {
    #[serde(default)]
    pub enabled: bool,
    /// Schedule family: `social` (default) | `park6` | `sweep`.
    #[serde(default)]
    pub preset: Option<crate::schedule::HunterPreset>,
    /// Explicit channel set override (default: 12-channel RID set).
    #[serde(default)]
    pub channel_set: Option<Vec<u32>>,
    /// Explicit non-social (sweep) dwell override in ms.
    #[serde(default)]
    pub dwell_default_ms: Option<u64>,
    /// Explicit social/priority dwell override in ms.
    #[serde(default)]
    pub dwell_priority_ms: Option<u64>,
    /// Explicit social/priority channel list override.
    #[serde(default)]
    pub priority_channels: Option<Vec<u32>>,
    /// Opt-in single-target audit lock in ms. WHAT: when > 0, a drone-class
    /// frame defers rotation for at most this long. WHY: single-target
    /// audit tool ONLY — off (0) in every preset because a network receiver
    /// must never let one aircraft starve the others. WHEN-to-tune: audit
    /// missions only. DEPENDS-ON: capture-budget release (lock_on_min_ms).
    #[serde(default)]
    pub lock_on_duration_ms: Option<u64>,
    /// Lock minimum hold floor in ms (audit mode only).
    #[serde(default)]
    pub lock_on_min_ms: Option<u64>,
    /// Dwell jitter fraction in [0, 0.5] (default 0.15).
    #[serde(default)]
    pub jitter_fraction: Option<f64>,
    /// Deterministic jitter seed (default fixed for CI/fleet agreement).
    #[serde(default)]
    pub jitter_seed: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackhaulYaml {
    /// Legacy override is ignored; the engine derives its subject.
    pub target_subject: Option<String>,
    pub broker_urls: Vec<String>,
    pub credentials_path: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuningYaml {
    #[serde(default)]
    pub edge_processing: EdgeProcessingYaml,
    #[serde(default)]
    pub heartbeat: HeartbeatYaml,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeProcessingYaml {
    /// Edge dedup window in milliseconds; default 1000 per Wave 6.0b consensus.
    #[serde(default = "default_dedup_window_ms")]
    pub deduplication_window_ms: u64,
}

impl Default for EdgeProcessingYaml {
    fn default() -> Self {
        EdgeProcessingYaml {
            deduplication_window_ms: default_dedup_window_ms(),
        }
    }
}

fn default_dedup_window_ms() -> u64 {
    1000
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatYaml {
    /// Heartbeat publish cadence in seconds; default 5 per Wave 6.0e consensus.
    /// Lower bound: 1 (we won't spam below 1 Hz). Upper bound: caller's discretion;
    /// commercial Pro deployments may want sub-second for tight ops dashboards.
    #[serde(default = "default_heartbeat_interval_secs")]
    pub interval_secs: u64,
}

impl Default for HeartbeatYaml {
    fn default() -> Self {
        HeartbeatYaml {
            interval_secs: default_heartbeat_interval_secs(),
        }
    }
}

fn default_heartbeat_interval_secs() -> u64 {
    5
}

// ── Wave 7.4 (2026-05-26) — Sensors manifold config ───────────────────
//
// The first non-Wi-Fi sensor (UbloxGps → NmeaGps) is now a TRUST
// primitive, not just a convenience: it anchors the sentinel's
// reported position to satellite consensus. Without GPS, a relocated
// sensor continues to broadcast its config-static install location,
// which means an adversary could earn reputation at site X, physically
// move the sensor to site Y, and spoof captures-from-Y as captures-at-X.
// This is the "Reputation-Portability Attack" the release approver identified 2026-05-26.
//
// Default policy (Wave 7.4): required=true. Operators who genuinely
// want dev/test mode without GPS must explicitly set required=false,
// which AUTOMATICALLY tags the heartbeat with node_position_source =
// "config_static" — self-disclosing untrusted-mode signal that
// downstream consumers (globe-backend Reputation Gravity, future
// Task #81) can refuse to accumulate trust against.

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorsYaml {
    #[serde(default)]
    pub gps: GpsYaml,
    #[serde(default)]
    pub rid_ble: RidBleYaml,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RidBleYaml {
    pub enabled: bool,
    pub unblock_rfkill: bool,
    pub adapter: BleAdapterYaml,
    pub quiet_window_s: u64,
}

impl Default for RidBleYaml {
    fn default() -> Self {
        Self {
            enabled: false,
            unblock_rfkill: false,
            adapter: BleAdapterYaml::default(),
            quiet_window_s: 30,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BleAdapterYaml {
    pub bd_addr: Option<String>,
    pub usb_id: Option<String>,
}

impl RidBleYaml {
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self.quiet_window_s == 0 {
            return Err("rid_ble quiet_window_s must be positive".into());
        }
        match (&self.adapter.bd_addr, &self.adapter.usb_id) {
            (Some(addr), None) if valid_hex_identity(addr, 6, 2) => Ok(()),
            (None, Some(id)) if valid_hex_identity(id, 2, 4) => Ok(()),
            _ => Err(
                "rid_ble needs exactly one adapter identity: BD_ADDR or USB vendor:product".into(),
            ),
        }
    }
}

fn valid_hex_identity(value: &str, count: usize, width: usize) -> bool {
    let parts: Vec<_> = value.split(':').collect();
    parts.len() == count
        && parts
            .iter()
            .all(|p| p.len() == width && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Optional PMTK initialization for MTK-family receivers. `pmtk: false`
/// (the default) sends nothing — the module's power-on defaults stand.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GpsInitYaml {
    /// Master gate: nothing is sent unless this is true.
    #[serde(default)]
    pub pmtk: bool,
    /// PMTK_API_SET_NMEA_OUTPUT frequency, 1..=10 Hz. None = leave default.
    #[serde(default)]
    pub update_rate_hz: Option<u32>,
    /// Comma list of NMEA sentence types to emit (e.g. "gga,rmc,gsa").
    /// None = leave default set.
    #[serde(default)]
    pub nmea_set: Option<String>,
    /// PMK_API_SET_SBAS_ENABLE (1=on, 0=off). None = leave default.
    #[serde(default)]
    pub sbas: Option<bool>,
}

impl GpsInitYaml {
    /// Render the enabled PMTK sentences (checksummed). Empty when off.
    pub fn sentences(&self) -> Vec<String> {
        if !self.pmtk {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Some(hz) = self.update_rate_hz {
            out.push(format!(
                "$PMTK220,{}",
                (1000 / hz.clamp(1, 10))
            ));
        }
        if let Some(set) = &self.nmea_set {
            let cleaned: String = set
                .split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(",");
            if !cleaned.is_empty() {
                out.push(format!("$PMTK314,{}", cleaned));
            }
        }
        if let Some(sbas) = self.sbas {
            out.push(format!("$PMTK313,{}", if sbas { 1 } else { 0 }));
        }
        out.into_iter().map(|s| pmtk_checksum(&s)).collect()
    }
}

/// Compute + append the NMEA XOR checksum (`$PMTK220,100*1F`).
fn pmtk_checksum(sentence: &str) -> String {
    let body: &str = sentence.strip_prefix('$').unwrap_or(sentence);
    let body = body.split('*').next().unwrap_or(body);
    let mut v: u8 = 0;
    for ch in body.bytes() {
        v ^= ch;
    }
    format!("{}*{:02X}", sentence, v)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GpsYaml {
    #[serde(default)]
    pub clock: ClockYaml,
    /// Optional receiver initialization (Adafruit Ultimate GPS #746 and other
    /// GlobalTop/MTK-family modules). Everything here is OFF by default: the
    /// stock 9600-baud 1 Hz GGA/RMC/GSA output already satisfies the reader.
    /// Sentences are sent once per serial open with correct NMEA checksums;
    /// a deliberate NO-OP default means an absent block never touches the
    /// port. PMTK baud-change commands are EXCLUDED by design — a wrong
    /// command bricks the port until power cycle (operator ruling).
    #[serde(default)]
    pub init: GpsInitYaml,
    /// CDC-ACM device path. Default `/dev/ttyACM1` matches the test-node-2
    /// hardware layout (u-blox 7 on the Anker hub Port 1, 2026-05-26).
    #[serde(default = "default_gps_device")]
    pub device: String,
    /// Serial baud. Default 9600 = u-blox 7 default. Other NMEA 0183
    /// receivers may use 4800, 38400, or 115200.
    #[serde(default = "default_gps_baud")]
    pub baud: u32,
    /// **Trust primitive.** When true (default), the engine refuses to
    /// start until the GPS sensor reaches Healthy within
    /// startup_grace_secs. Operators must explicitly set false to run
    /// in development/test mode — and that opt-out is wire-disclosed
    /// in the heartbeat (`node_position_source: "config_static"`) so
    /// downstream consumers can refuse to accumulate trust against
    /// non-anchored sentinels.
    #[serde(default = "default_gps_required")]
    pub required: bool,
    /// Cold-start grace window when `required: true`. Outdoor cold-start
    /// is typically 30s for u-blox 7; indoors with partial sky view can
    /// exceed 60s. Default 120 to accommodate the worst plausible
    /// outdoor-permanent-install case before declaring sensor failed.
    #[serde(default = "default_gps_startup_grace_secs")]
    pub startup_grace_secs: u64,
    /// Mark sensor Degraded when no fresh fix arrives within this many
    /// seconds. Matches the stamp-side freshness threshold used by the
    /// capture-loop's GPS substitution (also 30s) — both sides of the
    /// boundary agree on "what's stale."
    #[serde(default = "default_gps_stale_after_secs")]
    pub stale_after_secs: u64,
}

impl Default for GpsYaml {
    fn default() -> Self {
        GpsYaml {
            clock: ClockYaml::default(),
            init: GpsInitYaml::default(),
            device: default_gps_device(),
            baud: default_gps_baud(),
            required: default_gps_required(),
            startup_grace_secs: default_gps_startup_grace_secs(),
            stale_after_secs: default_gps_stale_after_secs(),
        }
    }
}

/// REQ-BRRD-001: bind the GPS via the udev-stable symlink, never a raw `/dev/ttyACM*`.
///
/// `/dev/ttyACM*` enumeration order is non-deterministic across power-cycles (the
/// 2026-05/06 "port jumpiness" incidents). On test-node-1 the symlink currently resolves to
/// `ttyACM1` — which is what this default used to hardcode, so it worked *by luck*. Had
/// enumeration shifted, the symlink would have followed the real GPS while this default
/// silently bound whatever landed on ACM1 (on test-node-1: the BLE adapter). Deterministic
/// failure with a clear device name beats lucky success.
///
/// The udev rule that creates this symlink lives at `deploy/udev/99-cybrrd-brrdfeeder.rules`
/// (harvested from a live feeder, not invented).
fn default_gps_device() -> String {
    "/dev/cybrrd_gps".to_string()
}
fn default_gps_baud() -> u32 {
    9600
}
fn default_gps_required() -> bool {
    true
} // Wave 7.4 trust-tier default
fn default_gps_startup_grace_secs() -> u64 {
    120
}
fn default_gps_stale_after_secs() -> u64 {
    30
}

impl EngineConfig {
    pub fn load_with_fallback(
        primary: &str,
        fallback: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        match Self::load(primary) {
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                Self::load(fallback)
            }
            result => result,
        }
    }

    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let mut cfg: EngineConfig = serde_yaml::from_str(&content)?;
        let heartbeat_secs = cfg.tuning.heartbeat.interval_secs.max(1);
        if cfg.node.status_file.is_some() && cfg.node.status_interval_secs < heartbeat_secs {
            eprintln!("[config] node.status_interval_secs={} is below heartbeat interval {}; using {} seconds", cfg.node.status_interval_secs, heartbeat_secs, heartbeat_secs);
        }
        cfg.sensors.rid_ble.validate()?;
        cfg.sensors.gps.clock.validate()?;
        if let Some(upward) = &cfg.upward {
            upward.validate(&cfg.node.id)?;
        }
        if let Some(savefile) = &cfg.capture.savefile {
            savefile.validate()?;
        }
        let loc = &cfg.node.location;
        if !loc.latitude.is_finite()
            || !(-90.0..=90.0).contains(&loc.latitude)
            || !loc.longitude.is_finite()
            || !(-180.0..=180.0).contains(&loc.longitude)
            || !loc.elevation_meters.is_finite()
        {
            return Err("node.location coordinates must be finite and in range".into());
        }
        if !crate::silver::valid_coordinates(loc.latitude, loc.longitude) {
            return Err("node.location: (0,0) is declared unknown, not a valid configured installation position".into());
        }
        cfg.loaded_config_hash = Some(format!("sha256:{}", hex::encode(Sha256::digest(content.as_bytes()))));
        for key in cfg.deprecated_keys() {
            eprintln!("[config] deprecated {key}: accepted for installer compatibility, ignored by engine");
        }
        Ok(cfg)
    }

    pub fn silver_context(&self) -> crate::silver::Context {
        crate::silver::Context {
            configured_position: crate::silver::ConfiguredPosition {
                latitude: self.node.location.latitude,
                longitude: self.node.location.longitude,
                elevation_meters: self.node.location.elevation_meters,
                source: crate::silver::ConfigSource::ConfigStatic,
            },
            config_hash: self.loaded_config_hash.clone(),
            fresh_for_ms: self
                .sensors
                .gps
                .stale_after_secs
                .saturating_mul(1000)
                .clamp(1, i64::MAX as u64) as i64,
        }
    }

    fn deprecated_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.node.storage_class.is_some() {
            keys.push("node.storage_class");
        }
        if self.backhaul.target_subject.is_some() {
            keys.push("backhaul.target_subject");
        }
        keys
    }

    /// Project the YAML into the wire-format `Node` block.
    ///
    /// Wave 7.4 (2026-05-26): the initial Node carries
    /// `position_source = Some(ConfigStatic)` — the install-time
    /// install location is by definition un-anchored. The capture
    /// loop's stamper (`capture::stamp_node_with_gps`) substitutes
    /// in `Some(GpsLive)` per-frame when a fresh GPS fix is
    /// available; when GPS is stale or absent, the static value
    /// (ConfigStatic) flows through unchanged.
    pub fn to_node(&self) -> Node {
        Node {
            id: self.node.id.clone(),
            location: NodeLocation {
                lat: self.node.location.latitude,
                lon: self.node.location.longitude,
                alt_m: self.node.location.elevation_meters,
                position_source: Some(PositionSource::ConfigStatic),
            },
            version: self
                .node
                .version
                .clone()
                .unwrap_or_else(|| DEFAULT_NODE_VERSION.to_string()),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SavefileYaml {
    pub directory: std::path::PathBuf,
    pub max_bytes: u64,
    pub max_files: usize,
    pub max_age_secs: u64,
}
impl Default for SavefileYaml {
    fn default() -> Self {
        Self {
            directory: "/var/lib/brrdfeeder/capture".into(),
            max_bytes: 8 * 1024 * 1024,
            max_files: 4,
            max_age_secs: 86400,
        }
    }
}
impl SavefileYaml {
    pub fn validate(&self) -> Result<(), String> {
        if !self.directory.is_absolute()
            || !(65536..=64 * 1024 * 1024).contains(&self.max_bytes)
            || !(1..=16).contains(&self.max_files)
            || !(1..=7 * 86400).contains(&self.max_age_secs)
        {
            return Err("capture.savefile requires absolute directory, 64KiB..64MiB/file, 1..16 files, 1s..7d age".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClockYaml {
    pub consistent_fixes: u32,
    pub max_step_secs: u64,
    pub allow_large_step: bool,
}
impl Default for ClockYaml {
    fn default() -> Self {
        Self {
            consistent_fixes: 3,
            max_step_secs: 7 * 86400,
            allow_large_step: false,
        }
    }
}
impl ClockYaml {
    pub fn validate(&self) -> Result<(), String> {
        if !(2..=10).contains(&self.consistent_fixes)
            || !(1..=400 * 86400).contains(&self.max_step_secs)
        {
            return Err(
                "GPS clock requires 2..10 consistent fixes and max_step_secs in 1s..400d".into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Channel-scheduler acceptance C1 (red on the pre-scheduler default):
    /// the DEFAULT hunter configuration must give the social channels
    /// (6 and 149) a long-dwell share — at least 24% of scheduled dwell
    /// time each — so a ≥1 Hz Beacon and the 512 TU NAN discovery-window
    /// cadence are caught with high probability per visit. The pre-scheduler
    /// default (400 ms priority dwell inside a 3800 ms cycle ≈ 10.5%) fails
    /// this assertion.
    #[test]
    fn hunter_defaults_give_social_channels_long_dwell_share() {
        let plan = crate::schedule::HunterPlan::from_yaml(&HunterYaml::default());
        assert!(
            !plan.channel_set.is_empty(),
            "default channel set must exist"
        );
        let total = plan.cycle_ms();
        assert!(total > 0);
        for social in [6u32, 149] {
            assert!(
                plan.channel_set.contains(&social),
                "social channel {social} must be in the default channel set"
            );
            let share_pct = plan.share_pct(social);
            assert!(
                share_pct >= 24,
                "social channel {social} must hold >=24% of scheduled dwell (got {share_pct}%)"
            );
            let d = plan.dwell_for(social);
            assert!(
                d >= 1000,
                "social dwell {d} ms must be >=1000 ms (>=1 s Beacon interval)"
            );
            // One NAN discovery window period is 512 TU = 524.288 ms; the
            // dwell must cover at least one full period plus margin.
            assert!(
                d as f64 >= crate::schedule::NAN_DW_PERIOD_MS * 1.15,
                "social dwell {d} ms must cover a NAN discovery window plus margin"
            );
        }
    }

    /// Channel-scheduler acceptance C2 (red on the pre-scheduler default):
    /// aircraft-following lock-on must be OFF by default. A network receiver
    /// hears ALL aircraft in range; a drone being heard must never defer
    /// channel rotation. The pre-scheduler default (2000 ms) fails both the
    /// config assertion and the behavioral hunter-state assertion.
    #[test]
    fn hunter_default_disables_aircraft_following_lock_on() {
        let plan = crate::schedule::HunterPlan::from_yaml(&HunterYaml::default());
        assert_eq!(
            plan.lock_on_duration_ms, 0,
            "lock-on must default to 0 (opt-in single-target audit tool only)"
        );
        let hs = crate::hunter::HunterState::new();
        hs.record_drone_observation("ACCEPTANCE-AIR-1", true, plan.lock_on_duration_ms);
        assert!(
            !hs.lock_on_active(),
            "a drone observation under the default config must not defer rotation"
        );
    }

    #[test]
    fn status_file_is_explicitly_opt_in() {
        let base = "id: test\nlocation: {latitude: 0, longitude: 0, elevation_meters: 0}\n";
        for suffix in ["", "status_file: null\n"] {
            let node: NodeYaml = serde_yaml::from_str(&format!("{base}{suffix}")).unwrap();
            assert!(node.status_file.is_none());
        }
        let node: NodeYaml = serde_yaml::from_str(&format!(
            "{base}status_file: /var/lib/brrdfeeder/status.json\n"
        ))
        .unwrap();
        assert_eq!(
            node.status_file.unwrap(),
            std::path::Path::new("/var/lib/brrdfeeder/status.json")
        );
        let node: NodeYaml =
            serde_yaml::from_str(&format!("{base}status_file: /tmp/custom-status.json\n")).unwrap();
        assert_eq!(
            node.status_file.unwrap(),
            std::path::Path::new("/tmp/custom-status.json")
        );
    }

    #[test]
    fn status_interval_defaults_and_clamps_to_heartbeat() {
        let base = "id: test\nlocation: {latitude: 0, longitude: 0, elevation_meters: 0}\n";
        let node: NodeYaml = serde_yaml::from_str(base).unwrap();
        assert_eq!(node.status_interval_secs, 30);
        assert_eq!(node.effective_status_interval_secs(5), 30);
        assert_eq!(node.effective_status_interval_secs(60), 60);
        for requested in [0, 1, 5, 30, 60] {
            let node: NodeYaml =
                serde_yaml::from_str(&format!("{base}status_interval_secs: {requested}\n"))
                    .unwrap();
            assert_eq!(node.effective_status_interval_secs(5), requested.max(5));
        }
    }

    #[test]
    fn status_file_rejects_nonstring_or_nonabsolute_at_config_load() {
        let path = std::env::temp_dir().join(format!("d19-config-{}.yaml", std::process::id()));
        let base = include_str!("../../config.yaml")
            .replace("latitude: 0.0", "latitude: 40.0")
            .replace("longitude: 0.0", "longitude: -95.0");
        for invalid in [
            "123",
            "true",
            "[]",
            "{}",
            "relative.json",
            "'/'",
            "''",
            "\"/tmp/a\\0b\"",
        ] {
            let yaml = base.replacen("node:", &format!("node:\n  status_file: {invalid}"), 1);
            std::fs::write(&path, yaml).unwrap();
            let err = EngineConfig::load(path.to_str().unwrap())
                .unwrap_err()
                .to_string();
            assert!(err.contains("node.status_file"), "{invalid}: {err}");
        }
        let yaml = base.replacen(
            "node:",
            "node:\n  status_file: /var/lib/brrdfeeder/status.json\n  status_interval_secs: 1",
            1,
        );
        std::fs::write(&path, yaml).unwrap();
        let cfg = EngineConfig::load(path.to_str().unwrap()).unwrap();
        assert_eq!(
            cfg.node
                .effective_status_interval_secs(cfg.tuning.heartbeat.interval_secs),
            5
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn verify_req_brrd_014_config_refuses_unknown_and_defaults_disabled() {
        let cfg: RidBleYaml = serde_yaml::from_str("{}").unwrap();
        assert!(!cfg.enabled);
        assert!(!cfg.unblock_rfkill);
        assert!(cfg.validate().is_ok());
        for yaml in [
            "enabled: true",
            "enabled: true\nadapter: {bd_addr: hci0}",
            "enabled: true\nadapter: {bd_addr: '00:E0:4C:31:5E:C6', usb_id: '0bda:876e'}",
            "enabled: true\nquiet_window_s: 0\nadapter: {usb_id: '0bda:876e'}",
        ] {
            assert!(serde_yaml::from_str::<RidBleYaml>(yaml)
                .unwrap()
                .validate()
                .is_err());
        }
        for yaml in ["mystery: true", "adapter: {hci: 0}"] {
            assert!(serde_yaml::from_str::<RidBleYaml>(yaml).is_err());
        }
        let cfg: RidBleYaml =
            serde_yaml::from_str("enabled: true\nadapter: {bd_addr: '00:E0:4C:31:5E:C6'}").unwrap();
        assert!(cfg.validate().is_ok());
    }

    /// REQ-BRRD-003 — the substrate refuses the lie: a config carrying a key
    /// the engine does not consume must fail to load, and the error must name
    /// the unknown key so the operator can act on it.
    #[test]
    fn unknown_config_key_is_refused() {
        let yaml = r#"
node:
  id: "test-node-001"
  not_a_supported_key: "ephemeral"
  location: { latitude: 0.0, longitude: 0.0, elevation_meters: 0 }
capture: { interface: "wlan1" }
backhaul: { broker_urls: ["tls://example.invalid:4222"], credentials_path: "/tmp/x" }
"#;
        let err = serde_yaml::from_str::<EngineConfig>(yaml)
            .expect_err("unknown key must refuse to parse")
            .to_string();
        assert!(
            err.contains("not_a_supported_key"),
            "error must name the unknown key, got: {err}"
        );
    }

    /// REQ-BRRD-003 golden — the SHIPPED reference config.yaml must always
    /// parse against the real engine schema. If this test fails, the reference
    /// config claims a key the engine does not wire (or the schema moved
    /// without the reference following) — fix the drift, not the test.
    #[test]
    fn installer_legacy_keys_warn_and_primary_parse_errors_survive() {
        let raw = include_str!("../../deploy/bootstrap/config.yaml.mobile.template");
        let cfg: EngineConfig = serde_yaml::from_str(raw).unwrap();
        assert_eq!(
            cfg.deprecated_keys(),
            ["node.storage_class", "backhaul.target_subject"]
        );
        let root = std::env::temp_dir().join(format!("cybrrd-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let primary = root.join("config.yaml");
        let fallback = root.join("fallback.yaml");
        // D32: the shipped mobile template still has (0,0) and must now be
        // refused until configured. Do not edit installer assets in this packet.
        std::fs::write(&fallback, raw).unwrap();
        assert!(EngineConfig::load(fallback.to_str().unwrap())
            .unwrap_err()
            .to_string()
            .contains("(0,0)"));
        let valid = raw
            .replace("latitude: 0.0", "latitude: 40.0")
            .replace("longitude: 0.0", "longitude: -95.0");
        std::fs::write(&fallback, valid).unwrap();
        let load = || {
            EngineConfig::load_with_fallback(primary.to_str().unwrap(), fallback.to_str().unwrap())
        };
        assert!(load().is_ok()); // only NotFound permits fallback
        std::fs::write(&primary, "node: [broken").unwrap();
        assert!(load().is_err()); // valid fallback must not hide malformed primary
        std::fs::remove_file(&fallback).unwrap();
        let error = load().unwrap_err();
        assert!(error.downcast_ref::<serde_yaml::Error>().is_some());
        std::fs::remove_file(&primary).unwrap();
        std::fs::create_dir(&primary).unwrap();
        assert_ne!(
            load()
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::NotFound
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shipped_reference_config_parses() {
        let raw = include_str!("../../config.yaml");
        let cfg: EngineConfig =
            serde_yaml::from_str(raw).expect("reference config.yaml must match the engine schema");
        assert!(
            cfg.sensors.gps.required,
            "reference config must not silently opt out of the GPS trust tier (REQ-BRRD-002)"
        );
    }

    #[test]
    fn d32_config_validation_and_loaded_hash_are_not_disk_assertions() {
        let root = std::env::temp_dir().join(format!("d32-config-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("fixture.yaml");
        let yaml = |lat: &str, lon: &str| {
            format!("node:\n  id: fixture\n  location: {{latitude: {lat}, longitude: {lon}, elevation_meters: 0}}\ncapture: {{interface: fixture}}\nbackhaul: {{broker_urls: [], credentials_path: unused}}\n")
        };
        for (lat, lon) in [
            ("0", "0"),
            ("-0.0", "0"),
            ("91", "1"),
            ("1", "181"),
            (".nan", "1"),
        ] {
            std::fs::write(&path, yaml(lat, lon)).unwrap();
            let err = EngineConfig::load(path.to_str().unwrap())
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("node.location"),
                "classification must name the invalid config: {err}"
            );
        }
        for (lat, lon) in [("0", "1"), ("1", "0"), ("40", "-95")] {
            std::fs::write(&path, yaml(lat, lon)).unwrap();
            assert!(EngineConfig::load(path.to_str().unwrap()).is_ok());
        }
        let loaded = EngineConfig::load(path.to_str().unwrap()).unwrap();
        let hash = loaded.loaded_config_hash.clone().unwrap();
        // Independent sha256sum golden: preserve the wire format across digest API upgrades.
        assert_eq!(hash, "sha256:93d35e0eb3ffc08474e7beccee891788f641daa5032f1dc3a18abb6e6add81fb");
        std::fs::write(&path, yaml("41", "-96")).unwrap();
        assert_eq!(
            loaded.silver_context().config_hash.as_deref(),
            Some(hash.as_str()),
            "disk replacement is not a reload"
        );
        let reloaded = EngineConfig::load(path.to_str().unwrap()).unwrap();
        assert_ne!(
            reloaded.loaded_config_hash, loaded.loaded_config_hash,
            "new loaded bytes must change hash"
        );
        assert_eq!(loaded.silver_context().configured_position.latitude, 40.0);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    /// REQ-BRRD-001 golden — the COMPILED default GPS device must be the
    /// udev-stable symlink, never a raw `/dev/ttyACM*` path.
    ///
    /// This is the contract's teeth. `/dev/ttyACM*` enumeration is
    /// non-deterministic across power-cycles; a raw-path default silently binds
    /// whatever enumerated into that slot. On test-node-1 the symlink resolves to
    /// `ttyACM1` — exactly what this default used to hardcode — so the old
    /// default worked by luck, and `ttyACM0` there is the BLE adapter. If this
    /// test fails because someone "fixed" the default back to a raw path, the
    /// regression is the default, not the test.
    #[test]
    fn gps_default_device_is_the_udev_symlink() {
        let dev = default_gps_device();
        assert_eq!(
            dev, "/dev/cybrrd_gps",
            "REQ-BRRD-001: compiled GPS default must be the udev-stable symlink"
        );
        assert!(
            !dev.contains("ttyACM"),
            "REQ-BRRD-001: compiled default must never be a raw /dev/ttyACM* path, got: {dev}"
        );
    }

    /// REQ-BRRD-001 golden — a config that omits `sensors.gps.device` must fall
    /// back to the udev symlink, not to a raw port. Guards the serde default
    /// wiring itself, not just the helper function.
    #[test]
    fn gps_device_omitted_falls_back_to_symlink() {
        let yaml = r#"
node:
  id: "test-node-001"
  location: { latitude: 0.0, longitude: 0.0, elevation_meters: 0 }
capture: { interface: "wlan1" }
backhaul: { broker_urls: ["tls://example.invalid:4222"], credentials_path: "/tmp/x" }
"#;
        let cfg: EngineConfig = serde_yaml::from_str(yaml).expect("minimal config must parse");
        assert_eq!(
            cfg.sensors.gps.device, "/dev/cybrrd_gps",
            "REQ-BRRD-001: omitted gps.device must default to the udev symlink"
        );
    }
}

#[cfg(test)]
mod gps_init_tests {
    use super::*;

    #[test]
    fn init_off_by_default_sends_nothing() {
        let y: GpsYaml = serde_yaml::from_str("").unwrap();
        assert!(y.init.sentences().is_empty());
        let y2: GpsYaml =
            serde_yaml::from_str("init: {update_rate_hz: 10}").unwrap();
        // pmtk master gate false => nothing sent even with fields set.
        assert!(y2.init.sentences().is_empty());
    }

    #[test]
    fn init_renders_checksummed_sentences() {
        let y: GpsYaml = serde_yaml::from_str(
            "init: {pmtk: true, update_rate_hz: 10, nmea_set: \"GGA, RMC ,gsa\", sbas: true}",
        )
        .unwrap();
        let s = y.init.sentences();
        assert_eq!(s.len(), 3, "{:?}", s);
        assert_eq!(s[0], "$PMTK220,100*2F");
        assert_eq!(s[1], "$PMTK314,gga,rmc,gsa*70");
        assert_eq!(s[2], "$PMTK313,1*2E");
        for sent in &s {
            let body = &sent[1..sent.len() - 3];
            let v = body.bytes().fold(0u8, |a, b| a ^ b);
            assert_eq!(format!("*{:02X}", v), &sent[sent.len() - 3..]);
        }
    }

    #[test]
    fn init_update_rate_clamps_to_receiver_limits() {
        let y: GpsYaml = serde_yaml::from_str("init: {pmtk: true, update_rate_hz: 50}")
            .unwrap();
        assert_eq!(y.init.sentences(), vec!["$PMTK220,100*2F".to_string()]);
    }
}
