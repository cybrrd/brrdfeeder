// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! ADR 0007 tier 1: local, read-only status. Read-only kernel rfkill observations;
//! no radio control, network probes or credentials here.
//! The destination directory is operator-owned; failures never stop capture.
//!
//! Consumers MUST treat missing status, or a `written_at` older than three
//! times `status_interval_secs` (the effective interval), as stale/offline and
//! MUST NOT render it healthy. Invalid/future timestamps or an untrusted clock
//! cannot establish freshness. Default freshness budget: 3 x 30 s = 90 s.
//! Graceful shutdown unlinks best-effort; SIGKILL/power loss can leave a file.
//! Meaningful changes are sampled at 1 Hz; all writes are limited to 1/second.
//! The configured path must belong exclusively to this engine instance.
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use crate::heartbeat::{GpsVitals, HeartbeatPayload, RadioStatus};
use crate::nats_publisher::NatsStatus;

#[derive(Debug, Serialize)]
pub struct StatusPayload<'a> {
    schema_version: u8,
    written_at: String,
    status_interval_secs: u64,
    heartbeat: &'a HeartbeatPayload,
    inventory: Inventory,
    links: Links,
}

#[derive(Debug, Serialize)]
struct Inventory {
    capture: Vec<CaptureInterface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rid_ble: Option<BleInventory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gps: Option<GpsInventory>,
}

#[derive(Debug, Serialize)]
struct CaptureInterface {
    interface: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    driver: Option<String>,
    // Up proves a working monitor capture path; other states do not prove
    // managed mode. Unknown is omitted, never misrepresented as false.
    #[serde(skip_serializing_if = "Option::is_none")]
    monitor_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_channel: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BleInventory {
    pub bd_addr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usb_id: Option<String>,
    pub observed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfkill: Option<RfkillSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::sensor::SensorState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_rfkill: Option<RfkillSnapshot>,
    #[serde(skip)]
    pub health: Option<Arc<arc_swap::ArcSwap<crate::sensor::SensorHealth>>>,
    #[serde(skip)]
    pub rfkill_observer: Option<crate::rfkill::RfkillObserver>,
}

impl BleInventory {
    pub fn refresh(&mut self) {
        self.state = self.health.as_ref().map(|h| h.load().state);
        // A missing/unreadable/replaced switch is UNKNOWN, never a retained
        // green or blocked bit. The original preflight remains history only.
        self.current_rfkill = self.rfkill_observer.as_ref().and_then(|o| o.read()).map(
            |(soft_blocked, hard_blocked)| RfkillSnapshot {
                soft_blocked,
                hard_blocked,
                observed_at: utc_now(),
            },
        );
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RfkillSnapshot {
    pub soft_blocked: bool,
    pub hard_blocked: bool,
    pub observed_at: String,
}

#[derive(Debug, Serialize)]
struct GpsInventory {
    device: String,
    #[serde(flatten)]
    fix: GpsVitals,
}

#[derive(Debug, Serialize)]
struct Links {
    nats_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_successful_publish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_frame_observed: Option<String>,
    frames_last_hour: u64,
}

pub fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn timestamp(ms: u64) -> Option<String> {
    if ms == 0 {
        return None;
    }
    DateTime::from_timestamp_millis(i64::try_from(ms).ok()?)
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// Fixed-memory, monotonic one-second buckets. Counts raw capture packets and
/// BLE advertising reports, not decoded/deduplicated RID or published frames.
/// The oldest partial second is excluded (at most one second of undercount).
pub struct FrameActivity {
    start: Instant,
    counts: Mutex<Vec<(u64, u64)>>,
    last_frame_ms: AtomicU64,
}

impl Default for FrameActivity {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            counts: Mutex::new(vec![(0, 0); 3600]),
            last_frame_ms: AtomicU64::new(0),
        }
    }
}

impl FrameActivity {
    pub fn record(&self, count: u64) {
        if count == 0 {
            return;
        }
        self.record_at(self.start.elapsed().as_secs(), count);
        self.last_frame_ms
            .store(crate::heartbeat::now_unix_ms(), Ordering::Relaxed);
    }

    fn record_at(&self, second: u64, count: u64) {
        let mut buckets = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        let bucket = &mut buckets[(second % 3600) as usize];
        if bucket.0 != second {
            *bucket = (second, 0);
        }
        bucket.1 = bucket.1.saturating_add(count);
    }

    fn count_at(&self, second: u64) -> u64 {
        self.counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(s, _)| *s <= second && second - s < 3600)
            .fold(0u64, |sum, (_, n)| sum.saturating_add(*n))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Meaningful {
    radio: RadioStatus,
    nats: String,
    // GPS is currently the only error counter in the serialized schema.
    gps: Option<(crate::sensor::SensorState, Option<u8>, u64)>,
    capture: Vec<(String, Option<bool>)>,
    rfkill: Option<(bool, bool)>,
    ble_state: Option<crate::sensor::SensorState>,
    current_rfkill: Option<(bool, bool)>,
    policy_ack: Option<String>,
    update_blocked_reason: Option<String>,
    position_state: Option<crate::silver::PositionState>,
    management_status: Option<crate::silver::ManagementStatus>,
    engine_version: Option<String>,
    image_digest: Option<String>,
}

impl StatusPayload<'_> {
    fn meaningful(&self) -> Meaningful {
        Meaningful {
            radio: self.heartbeat.radio_status,
            nats: self.links.nats_state.clone(),
            gps: self
                .heartbeat
                .gps
                .as_ref()
                .map(|g| (g.state, g.fix_quality, g.error_count)),
            capture: self
                .inventory
                .capture
                .iter()
                .map(|c| (c.interface.clone(), c.monitor_mode))
                .collect(),
            rfkill: self
                .inventory
                .rid_ble
                .as_ref()
                .and_then(|b| b.rfkill.as_ref())
                .map(|r| (r.soft_blocked, r.hard_blocked)),
            ble_state: self.inventory.rid_ble.as_ref().and_then(|b| b.state),
            current_rfkill: self
                .inventory
                .rid_ble
                .as_ref()
                .and_then(|b| b.current_rfkill.as_ref())
                .map(|r| (r.soft_blocked, r.hard_blocked)),
            policy_ack: self.heartbeat.policy_ack.clone(),
            update_blocked_reason: self.heartbeat.update_blocked_reason.clone(),
            position_state: self.heartbeat.position_status.as_ref().map(|s| s.state),
            management_status: self.heartbeat.management_status,
            engine_version: self.heartbeat.engine_version.clone(),
            image_digest: self.heartbeat.image_digest.clone(),
        }
    }
}

#[derive(Default)]
struct WriterState {
    stopped: bool,
    initialized: bool,
    last_attempt: Option<Instant>,
    last_success: Option<(Instant, Meaningful)>,
}

impl WriterState {
    fn due(&self, now: Instant, interval_secs: u64, meaningful: &Meaningful) -> bool {
        !self.stopped
            && self
                .last_attempt
                .is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(1))
            && self.last_success.as_ref().is_none_or(|(t, old)| {
                now.saturating_duration_since(*t) >= Duration::from_secs(interval_secs)
                    || meaningful != old
            })
    }
}

#[derive(Clone)]
pub struct StatusSource {
    pub path: PathBuf,
    pub interface: String,
    pub gps_device: String,
    pub ble: Arc<ArcSwapOption<BleInventory>>,
    pub nats: Arc<NatsStatus>,
    pub frames: Arc<FrameActivity>,
    interval_secs: u64,
    writer: Arc<Mutex<WriterState>>,
}

impl StatusSource {
    pub fn new(
        path: PathBuf,
        interface: String,
        gps_device: String,
        ble: Arc<ArcSwapOption<BleInventory>>,
        nats: Arc<NatsStatus>,
        frames: Arc<FrameActivity>,
        interval_secs: u64,
    ) -> Self {
        Self {
            path,
            interface,
            gps_device,
            ble,
            nats,
            frames,
            interval_secs: interval_secs.max(1),
            writer: Arc::new(Mutex::new(WriterState::default())),
        }
    }

    pub fn snapshot<'a>(&self, heartbeat: &'a HeartbeatPayload) -> StatusPayload<'a> {
        let ble = self.ble.load_full().map(|b| {
            let mut current = (*b).clone();
            current.refresh();
            current
        });
        let hunter = heartbeat.hunter.as_ref();
        let driver = hunter
            .and_then(|h| h.radios.iter().find(|r| r.device_alias == self.interface))
            .and_then(|r| r.driver_name.clone());
        StatusPayload {
            schema_version: 1,
            written_at: utc_now(),
            status_interval_secs: self.interval_secs,
            heartbeat,
            inventory: Inventory {
                capture: vec![CaptureInterface {
                    interface: self.interface.clone(),
                    driver,
                    monitor_mode: (heartbeat.radio_status == RadioStatus::Up).then_some(true),
                    current_channel: hunter.map(|h| h.current_channel).filter(|c| *c != 0),
                }],
                rid_ble: ble,
                gps: heartbeat.gps.clone().map(|fix| GpsInventory {
                    device: self.gps_device.clone(),
                    fix,
                }),
            },
            links: Links {
                nats_state: self.nats.connection_state(),
                last_successful_publish: timestamp(
                    self.nats.last_publish_ms.load(Ordering::Relaxed),
                ),
                last_frame_observed: timestamp(self.frames.last_frame_ms.load(Ordering::Relaxed)),
                frames_last_hour: self.frames.count_at(self.frames.start.elapsed().as_secs()),
            },
        }
    }

    pub async fn write(&self, heartbeat: &HeartbeatPayload) -> io::Result<()> {
        let snapshot = self.snapshot(heartbeat);
        let meaningful = snapshot.meaningful();
        // Byte equality is deliberately NOT used: clock/uptime fields advance.
        let bytes = serde_json::to_vec(&snapshot).map_err(io::Error::other)?;
        let source = self.clone();
        tokio::task::spawn_blocking(move || {
            source
                .commit(&bytes, meaningful, Instant::now())
                .map(|_| ())
        })
        .await
        .map_err(io::Error::other)?
    }

    fn commit(&self, bytes: &[u8], meaningful: Meaningful, now: Instant) -> io::Result<bool> {
        // Held only on blocking workers. Serializes rename against shutdown.
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if !writer.due(now, self.interval_secs, &meaningful) {
            return Ok(false);
        }
        writer.last_attempt = Some(now);
        if !writer.initialized {
            sweep_orphans(&self.path)?;
            writer.initialized = true;
        }
        write_atomic(&self.path, bytes)?;
        writer.last_success = Some((now, meaningful));
        Ok(true)
    }

    pub async fn shutdown(&self) -> io::Result<()> {
        let source = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut writer = source.writer.lock().unwrap_or_else(|e| e.into_inner());
            writer.stopped = true;
            trusted_parent(&source.path)?;
            match fs::remove_file(&source.path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        })
        .await
        .map_err(io::Error::other)?
    }
}

fn trusted_parent(path: &Path) -> io::Result<&Path> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("status file requires a parent directory"))?;
    let metadata = fs::metadata(parent)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o022 != 0 {
        return Err(io::Error::other(
            "status parent must be a directory without group/world write permission",
        ));
    }
    Ok(parent)
}

fn sweep_orphans(path: &Path) -> io::Result<()> {
    let parent = trusted_parent(path)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("status file requires a filename"))?;
    let mut prefix = name.to_os_string();
    prefix.push(".tmp.");
    use std::os::unix::ffi::OsStrExt;
    // Exclusive configured path; run once before this instance's first write.
    // Bound directory work, skip symlinks/subdirectories/unrelated names, and
    // match only our numeric PID.sequence suffix (including reused PIDs).
    for entry in fs::read_dir(parent)?.take(1024) {
        let entry = entry?;
        let filename = entry.file_name();
        let Some(suffix) = filename.as_bytes().strip_prefix(prefix.as_bytes()) else {
            continue;
        };
        let mut parts = suffix.split(|b| *b == b'.');
        let valid = |part: Option<&[u8]>| {
            part.is_some_and(|p| !p.is_empty() && p.iter().all(u8::is_ascii_digit))
        };
        if valid(parts.next())
            && valid(parts.next())
            && parts.next().is_none()
            && entry.file_type()?.is_file()
        {
            match fs::remove_file(entry.path()) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Unique sibling + create_new (never follow a pre-existing temp symlink).
/// Rename replaces the destination inode without following a target symlink.
/// No mkdir/chown: the operator must supply a writable, trusted directory.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let parent = trusted_parent(path)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("status file requires a filename"))?;
    let mut collisions = 0;
    let (temp, mut file) = loop {
        let mut temporary_name = name.to_os_string();
        temporary_name.push(format!(
            ".tmp.{}.{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let temp = parent.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
        {
            Ok(file) => break (temp, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && collisions < 128 => {
                collisions += 1;
            }
            Err(e) => return Err(e),
        }
    };
    let result = (|| {
        file.write_all(bytes)?;
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Barrier;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            loop {
                let path = std::env::temp_dir().join(format!(
                    "cybrrd-d17-{}-{}",
                    std::process::id(),
                    SEQ.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => {
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                        return Self(path);
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("create test directory: {e}"),
                }
            }
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn heartbeat() -> HeartbeatPayload {
        serde_json::from_value(json!({
            "node_id": "redacted-node", "timestamp_utc": 1789776000000u64,
            "uptime_seconds": 90, "load_avg_1m": 0.1, "load_avg_5m": 0.2,
            "load_avg_15m": 0.3, "radio_status": "up", "cpu_temp_c": 42.5,
            "hunter": {
                "current_channel": 6, "lineage": "kittler-substrate-defense-v1",
                "channels": [{"channel":6,"noise_dbm":-90,"busy_pct":5,"sample_age_ms":10,
                    "dwell_ms_total":1200,"rid_hits_total":3,"dwell_share_pct":26}], "radios": [{
                    "role": "wifi_monitor", "device_alias": "wlan1",
                    "coverage_class": "logic_only", "driver_name": "fixture-driver",
                    "telemetry_features": {"channel_rotation":true,"noise_floor":true,"channel_busy_pct":true}
                }], "lock_on_active": false, "lock_on_triggers_total": 0,
                "lock_on_budget_releases_total": 0,
                "preset": "social",
                "retune": {"last_ms": 8, "max_ms": 11, "count": 24}
            },
            "gps": { "state": "healthy", "last_reading_ms": 1789776000000i64,
                "fix_quality": 1, "sat_count": 8, "hdop": 1.2, "error_count": 0, "detail":"fixture health note" },
            "node_position_source": "gps_live", "product_version": env!("CARGO_PKG_VERSION"),
            "engine_version": "fixture-sha",
            "image_digest": "sha256:fixture", "build_seq": 287, "channel": "stable",
            "os_clock_trusted": true, "policy_ack": "fixture-sha",
            "update_blocked_reason": "running_identity_unverified",
            "silver_schema_version": 1,
            "current_position": {"latitude":40.0,"longitude":-95.0,"source":"gps_live",
                "observed_at_ms":1789776000000i64,"received_at_ms":1789776000000i64,"time_source":"gps_utc"},
            "configured_position": {"latitude":41.0,"longitude":-96.0,"elevation_meters":300.0,"source":"config_static"},
            "position_status": {"state":"current","fresh_for_ms":30000,"observed_at_ms":1789776000000i64,
                "received_at_ms":1789776000000i64,"time_source":"gps_utc"},
            "config_hash":format!("sha256:{}", "a".repeat(64)),
            "management_status":"blocked"
        }))
        .unwrap()
    }

    fn source(path: PathBuf) -> StatusSource {
        StatusSource::new(
            path,
            "wlan1".into(),
            "/dev/cybrrd_gps".into(),
            Arc::new(ArcSwapOption::empty()),
            Arc::new(NatsStatus::default()),
            Arc::new(FrameActivity::default()),
            30,
        )
    }

    fn serialized_keys(value: &Value) -> std::collections::BTreeSet<String> {
        fn walk(value: &Value, path: &str, out: &mut std::collections::BTreeSet<String>) {
            match value {
                Value::Object(fields) => {
                    for (key, value) in fields {
                        let child = if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        };
                        out.insert(child.clone());
                        walk(value, &child, out);
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        walk(item, &format!("{path}[]"), out);
                    }
                }
                _ => {}
            }
        }
        let mut keys = std::collections::BTreeSet::new();
        walk(value, "", &mut keys);
        keys
    }

    fn assert_public_key_set(value: &Value) {
        // Deliberate disclosure review boundary, NOT derived from the payload.
        // The full fixture fills every current optional field and nested array.
        // Review any new path for disclosure AND meaningful-change semantics.
        let expected = [
            "schema_version",
            "written_at",
            "status_interval_secs",
            "heartbeat",
            "heartbeat.node_id",
            "heartbeat.timestamp_utc",
            "heartbeat.uptime_seconds",
            "heartbeat.load_avg_1m",
            "heartbeat.load_avg_5m",
            "heartbeat.load_avg_15m",
            "heartbeat.radio_status",
            "heartbeat.cpu_temp_c",
            "heartbeat.hunter",
            "heartbeat.hunter.current_channel",
            "heartbeat.hunter.lineage",
            "heartbeat.hunter.channels",
            "heartbeat.hunter.channels[].channel",
            "heartbeat.hunter.channels[].noise_dbm",
            "heartbeat.hunter.channels[].busy_pct",
            "heartbeat.hunter.channels[].sample_age_ms",
            "heartbeat.hunter.channels[].dwell_ms_total",
            "heartbeat.hunter.channels[].rid_hits_total",
            "heartbeat.hunter.channels[].dwell_share_pct",
            "heartbeat.hunter.preset",
            "heartbeat.hunter.retune",
            "heartbeat.hunter.retune.last_ms",
            "heartbeat.hunter.retune.max_ms",
            "heartbeat.hunter.retune.count",
            "heartbeat.hunter.radios",
            "heartbeat.hunter.radios[].role",
            "heartbeat.hunter.radios[].device_alias",
            "heartbeat.hunter.radios[].coverage_class",
            "heartbeat.hunter.radios[].driver_name",
            "heartbeat.hunter.radios[].telemetry_features",
            "heartbeat.hunter.radios[].telemetry_features.channel_rotation",
            "heartbeat.hunter.radios[].telemetry_features.noise_floor",
            "heartbeat.hunter.radios[].telemetry_features.channel_busy_pct",
            "heartbeat.hunter.lock_on_active",
            "heartbeat.hunter.lock_on_triggers_total",
            "heartbeat.hunter.lock_on_budget_releases_total",
            "heartbeat.gps",
            "heartbeat.gps.state",
            "heartbeat.gps.last_reading_ms",
            "heartbeat.gps.fix_quality",
            "heartbeat.gps.sat_count",
            "heartbeat.gps.hdop",
            "heartbeat.gps.error_count",
            "heartbeat.gps.detail",
            "heartbeat.node_position_source",
            "heartbeat.product_version",
            "heartbeat.engine_version",
            "heartbeat.image_digest",
            "heartbeat.build_seq",
            "heartbeat.channel",
            "heartbeat.os_clock_trusted",
            "heartbeat.policy_ack",
            "heartbeat.update_blocked_reason",
            "heartbeat.silver_schema_version",
            "heartbeat.current_position",
            "heartbeat.current_position.latitude",
            "heartbeat.current_position.longitude",
            "heartbeat.current_position.source",
            "heartbeat.current_position.observed_at_ms",
            "heartbeat.current_position.received_at_ms",
            "heartbeat.current_position.time_source",
            "heartbeat.configured_position",
            "heartbeat.configured_position.latitude",
            "heartbeat.configured_position.longitude",
            "heartbeat.configured_position.elevation_meters",
            "heartbeat.configured_position.source",
            "heartbeat.position_status",
            "heartbeat.position_status.state",
            "heartbeat.position_status.fresh_for_ms",
            "heartbeat.position_status.observed_at_ms",
            "heartbeat.position_status.received_at_ms",
            "heartbeat.position_status.time_source",
            "heartbeat.config_hash",
            "heartbeat.management_status",
            "inventory",
            "inventory.capture",
            "inventory.capture[].interface",
            "inventory.capture[].driver",
            "inventory.capture[].monitor_mode",
            "inventory.capture[].current_channel",
            "inventory.gps",
            "inventory.gps.device",
            "inventory.gps.state",
            "inventory.gps.last_reading_ms",
            "inventory.gps.fix_quality",
            "inventory.gps.sat_count",
            "inventory.gps.hdop",
            "inventory.gps.error_count",
            "inventory.gps.detail",
            "inventory.rid_ble",
            "inventory.rid_ble.bd_addr",
            "inventory.rid_ble.usb_id",
            "inventory.rid_ble.observed_at",
            "inventory.rid_ble.rfkill",
            "inventory.rid_ble.rfkill.soft_blocked",
            "inventory.rid_ble.rfkill.hard_blocked",
            "inventory.rid_ble.rfkill.observed_at",
            "inventory.rid_ble.state",
            "inventory.rid_ble.current_rfkill",
            "inventory.rid_ble.current_rfkill.soft_blocked",
            "inventory.rid_ble.current_rfkill.hard_blocked",
            "inventory.rid_ble.current_rfkill.observed_at",
            "links",
            "links.nats_state",
            "links.last_successful_publish",
            "links.last_frame_observed",
            "links.frames_last_hour",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(
            serialized_keys(value),
            expected,
            "public status key-set changed: disclosure review required"
        );
    }

    fn ble_fixture(dir: &Path) -> BleInventory {
        let root = dir.join("switches");
        let switch = root.join("rfkill17");
        fs::create_dir_all(&switch).unwrap();
        for (name, value) in [
            ("name", "hci6"),
            ("type", "bluetooth"),
            ("soft", "0"),
            ("hard", "0"),
        ] {
            fs::write(switch.join(name), value).unwrap();
        }
        BleInventory {
            bd_addr: "00:00:00:00:00:01".into(),
            usb_id: Some("0000:0001".into()),
            observed_at: "2026-09-19T00:00:00.000Z".into(),
            rfkill: Some(RfkillSnapshot {
                soft_blocked: false,
                hard_blocked: false,
                observed_at: "2026-09-19T00:00:00.000Z".into(),
            }),
            health: Some(Arc::new(arc_swap::ArcSwap::from_pointee(
                crate::sensor::SensorHealth {
                    state: crate::sensor::SensorState::Healthy,
                    ..crate::sensor::SensorHealth::initializing("rid_ble")
                },
            ))),
            rfkill_observer: crate::rfkill::RfkillObserver::at(&root, 6).unwrap(),
            ..Default::default()
        }
    }

    #[test]
    fn ble_status_reobserves_unblock_reblock_and_lost_switch() {
        let dir = TempDir::new();
        let source = source(dir.0.join("status.json"));
        let mut inventory = ble_fixture(&dir.0);
        inventory.rfkill.as_mut().unwrap().soft_blocked = true; // history before unblock
        let switch = dir.0.join("switches/rfkill17");
        let health = inventory.health.as_ref().unwrap().clone();
        source.ble.store(Some(Arc::new(inventory)));
        let h = heartbeat();
        let now = Instant::now();
        for (tick, soft, hard, state) in [
            (0, "1", "0", crate::sensor::SensorState::Failed),
            (1, "0", "0", crate::sensor::SensorState::Healthy),
            (2, "1", "0", crate::sensor::SensorState::Healthy),
            (3, "0", "1", crate::sensor::SensorState::Failed),
        ] {
            fs::write(switch.join("soft"), soft).unwrap();
            fs::write(switch.join("hard"), hard).unwrap();
            health.store(Arc::new(crate::sensor::SensorHealth {
                state,
                ..crate::sensor::SensorHealth::initializing("rid_ble")
            }));
            let snapshot = source.snapshot(&h);
            let current = snapshot.inventory.rid_ble.as_ref().unwrap();
            assert!(
                current.rfkill.as_ref().unwrap().soft_blocked,
                "history is retained"
            );
            assert_eq!(current.state, Some(state));
            assert_eq!(
                current.current_rfkill.as_ref().unwrap().soft_blocked,
                soft == "1"
            );
            assert_eq!(
                current.current_rfkill.as_ref().unwrap().hard_blocked,
                hard == "1"
            );
            let bytes = serde_json::to_vec(&snapshot).unwrap();
            assert!(source
                .commit(
                    &bytes,
                    snapshot.meaningful(),
                    now + Duration::from_secs(tick)
                )
                .unwrap());
            let v: Value = serde_json::from_slice(&fs::read(&source.path).unwrap()).unwrap();
            assert_eq!(
                v["inventory"]["rid_ble"]["current_rfkill"]["soft_blocked"],
                soft == "1"
            );
            assert!(
                v["inventory"]["rid_ble"]["current_rfkill"]["observed_at"]
                    .as_str()
                    .unwrap()
                    <= v["written_at"].as_str().unwrap()
            );
        }
        // An unrelated adapter is not a substitute; invalid reads do not retain green.
        fs::write(switch.join("name"), "hci7").unwrap();
        assert!(source
            .snapshot(&h)
            .inventory
            .rid_ble
            .unwrap()
            .current_rfkill
            .is_none());
        fs::write(switch.join("name"), "hci6").unwrap();
        fs::write(switch.join("soft"), "unknown").unwrap();
        assert!(source
            .snapshot(&h)
            .inventory
            .rid_ble
            .unwrap()
            .current_rfkill
            .is_none());
        fs::rename(&switch, dir.0.join("removed-switch")).unwrap();
        fs::create_dir(&switch).unwrap();
        for (name, value) in [
            ("name", "hci6"),
            ("type", "bluetooth"),
            ("soft", "0"),
            ("hard", "0"),
        ] {
            fs::write(switch.join(name), value).unwrap();
        }
        assert!(
            source
                .snapshot(&h)
                .inventory
                .rid_ble
                .unwrap()
                .current_rfkill
                .is_none(),
            "replacement cannot borrow the old controller's identity"
        );
    }

    #[test]
    fn full_payload_reuses_heartbeat_and_serializes_known_inventory() {
        let source = source(PathBuf::new());
        let dir = TempDir::new();
        source.ble.store(Some(Arc::new(ble_fixture(&dir.0))));
        source
            .nats
            .last_publish_ms
            .store(1789776000000, Ordering::Relaxed);
        source.frames.record(3);
        let heartbeat = heartbeat();
        let v: Value =
            serde_json::from_slice(&serde_json::to_vec(&source.snapshot(&heartbeat)).unwrap())
                .unwrap();
        let expected: Value =
            serde_json::from_slice(&serde_json::to_vec(&heartbeat).unwrap()).unwrap();
        assert_eq!(v["heartbeat"], expected);
        assert_eq!(v["schema_version"], 1);
        assert!(v["written_at"].as_str().unwrap().ends_with('Z'));
        DateTime::parse_from_rfc3339(v["written_at"].as_str().unwrap()).unwrap();
        assert_eq!(
            v["inventory"]["capture"][0],
            json!({
                "interface":"wlan1", "driver":"fixture-driver", "monitor_mode":true, "current_channel":6
            })
        );
        assert_eq!(v["inventory"]["gps"]["sat_count"], 8);
        assert_eq!(v["inventory"]["rid_ble"]["usb_id"], "0000:0001");
        assert_eq!(v["links"]["frames_last_hour"], 3);
        assert_eq!(v["links"]["nats_state"], "disconnected");
        assert!(v["links"]["last_successful_publish"].is_string());
        assert!(v["links"]["last_frame_observed"].is_string());
        let text = serde_json::to_string_pretty(&v).unwrap();
        assert_public_key_set(&v);
        println!(
            "D19 disclosure keys={} full_fixture_bytes={}",
            serialized_keys(&v).len(),
            serde_json::to_vec(&v).unwrap().len()
        );
        println!("{text}");
    }

    #[test]
    fn gps_absent_is_omitted_from_both_surfaces() {
        let source = source(PathBuf::new());
        let mut heartbeat = heartbeat();
        heartbeat.gps = None;
        let v = serde_json::to_value(source.snapshot(&heartbeat)).unwrap();
        assert!(v["heartbeat"].get("gps").is_none());
        assert!(v["inventory"].get("gps").is_none());
    }

    #[test]
    fn ble_absent_and_unknown_capture_fields_are_omitted() {
        let source = source(PathBuf::new());
        let mut heartbeat = heartbeat();
        heartbeat.hunter = None;
        heartbeat.radio_status = RadioStatus::Down;
        let v = serde_json::to_value(source.snapshot(&heartbeat)).unwrap();
        assert!(v["inventory"].get("rid_ble").is_none());
        assert_eq!(v["inventory"]["capture"][0], json!({"interface":"wlan1"}));
        assert_eq!(
            v["links"],
            json!({"nats_state":"disconnected", "frames_last_hour":0})
        );
    }

    #[test]
    fn frame_window_expires_and_reuses_buckets_without_accumulating() {
        let activity = FrameActivity::default();
        activity.record_at(0, 2);
        activity.record_at(1, 3);
        assert_eq!(activity.count_at(3599), 5);
        assert_eq!(activity.count_at(3600), 3);
        activity.record_at(3600, 7);
        assert_eq!(activity.count_at(3600), 10);
        assert_eq!(activity.count_at(7200), 0);
        assert_eq!(activity.counts.lock().unwrap().len(), 3600);
        assert!(timestamp(0).is_none());
        assert!(timestamp(u64::MAX).is_none());
    }

    #[test]
    fn atomic_replace_under_concurrent_readers_and_writers() {
        let dir = TempDir::new();
        let path = dir.0.join("status.json");
        let payloads: Vec<Vec<u8>> = [32, 65536]
            .iter()
            .enumerate()
            .map(|(generation, size)| {
                serde_json::to_vec(&json!({"generation":generation,"body":"x".repeat(*size)}))
                    .unwrap()
            })
            .collect();
        write_atomic(&path, &payloads[0]).unwrap();
        let stop = AtomicBool::new(false);
        let reads = AtomicUsize::new(0);
        let barrier = Barrier::new(6);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let path = &path;
                let payloads = &payloads;
                let stop = &stop;
                let reads = &reads;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    loop {
                        let bytes = fs::read(path).unwrap();
                        serde_json::from_slice::<Value>(&bytes).unwrap();
                        assert!(payloads.contains(&bytes), "reader saw a torn generation");
                        reads.fetch_add(1, Ordering::Relaxed);
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                    }
                });
            }
            let writers: Vec<_> = (0..2)
                .map(|offset| {
                    let path = &path;
                    let payloads = &payloads;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        for i in 0..100 {
                            write_atomic(path, &payloads[(i + offset) % 2]).unwrap();
                        }
                    })
                })
                .collect();
            let results: Vec<_> = writers.into_iter().map(|w| w.join()).collect();
            stop.store(true, Ordering::Release);
            for result in results {
                result.unwrap();
            }
        });
        assert!(reads.load(Ordering::Relaxed) >= 4);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        println!(
            "atomicity: 200 replacements, {} complete reads, 4 readers",
            reads.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn failed_rename_cleans_temp_and_missing_parent_is_not_created() {
        let dir = TempDir::new();
        let path = dir.0.join("status.json");
        fs::create_dir(&path).unwrap();
        assert!(write_atomic(&path, b"{}").is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        assert!(write_atomic(&dir.0.join("absent/status.json"), b"{}").is_err());
        assert!(!dir.0.join("absent").exists());
    }

    #[test]
    fn destination_symlink_is_replaced_not_followed() {
        let dir = TempDir::new();
        let victim = dir.0.join("unrelated");
        fs::write(&victim, b"unchanged").unwrap();
        let path = dir.0.join("status.json");
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        write_atomic(&path, b"{}").unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"unchanged");
        assert!(!fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[tokio::test]
    async fn async_writer_outputs_full_status() {
        let dir = TempDir::new();
        let source = source(dir.0.join("status.json"));
        let heartbeat = heartbeat();
        source.write(&heartbeat).await.unwrap();
        let v: Value = serde_json::from_slice(&fs::read(&source.path).unwrap()).unwrap();
        let expected: Value =
            serde_json::from_slice(&serde_json::to_vec(&heartbeat).unwrap()).unwrap();
        assert_eq!(v["heartbeat"], expected);
    }

    #[test]
    fn meaningful_fields_trigger_but_continuous_values_do_not() {
        let source = source(PathBuf::new());
        let original = heartbeat();
        let expected = source.snapshot(&original).meaningful();
        let changes: &[fn(&mut HeartbeatPayload)] = &[
            |h| h.radio_status = RadioStatus::Error,
            |h| h.gps.as_mut().unwrap().state = crate::sensor::SensorState::Degraded,
            |h| h.gps.as_mut().unwrap().fix_quality = Some(0),
            |h| h.gps.as_mut().unwrap().error_count += 1,
            |h| h.gps = None,
            |h| h.policy_ack = Some("changed".into()),
            |h| h.update_blocked_reason = None,
            |h| h.position_status.as_mut().unwrap().state = crate::silver::PositionState::Stale,
            |h| h.management_status = Some(crate::silver::ManagementStatus::Attention),
            |h| h.engine_version = Some("changed".into()),
            |h| h.image_digest = Some("changed".into()),
        ];
        for change in changes {
            let mut h = original.clone();
            change(&mut h);
            assert_ne!(source.snapshot(&h).meaningful(), expected);
        }
        let changes: &[fn(&mut StatusPayload<'_>)] = &[
            |s| s.links.nats_state = "connected".into(),
            |s| s.inventory.capture[0].interface = "wlan2".into(),
            |s| s.inventory.capture[0].monitor_mode = None,
            |s| {
                s.inventory.rid_ble.as_mut().unwrap().state =
                    Some(crate::sensor::SensorState::Failed)
            },
            |s| {
                s.inventory
                    .rid_ble
                    .as_mut()
                    .unwrap()
                    .current_rfkill
                    .as_mut()
                    .unwrap()
                    .soft_blocked = true
            },
            |s| s.inventory.rid_ble.as_mut().unwrap().current_rfkill = None,
            |s| {
                s.inventory
                    .rid_ble
                    .as_mut()
                    .unwrap()
                    .rfkill
                    .as_mut()
                    .unwrap()
                    .soft_blocked = true
            },
            |s| {
                s.inventory
                    .rid_ble
                    .as_mut()
                    .unwrap()
                    .rfkill
                    .as_mut()
                    .unwrap()
                    .hard_blocked = true
            },
        ];
        let dir = TempDir::new();
        source.ble.store(Some(Arc::new(ble_fixture(&dir.0))));
        let expected = source.snapshot(&original).meaningful();
        for change in changes {
            let mut s = source.snapshot(&original);
            change(&mut s);
            assert_ne!(s.meaningful(), expected);
        }
        let mut noisy = original;
        noisy.timestamp_utc += 5000;
        noisy.uptime_seconds += 5;
        noisy.load_avg_1m += 1.0;
        noisy.load_avg_5m += 1.0;
        noisy.load_avg_15m += 1.0;
        noisy.cpu_temp_c = Some(50.0);
        let hunter = noisy.hunter.as_mut().unwrap();
        hunter.current_channel = 11;
        hunter.lock_on_triggers_total += 1;
        hunter.lock_on_budget_releases_total += 1;
        hunter.channels[0].busy_pct = Some(70);
        hunter.channels[0].sample_age_ms += 100;
        let gps = noisy.gps.as_mut().unwrap();
        gps.last_reading_ms = Some(123);
        gps.sat_count = Some(12);
        gps.hdop = Some(2.0);
        let mut snapshot = source.snapshot(&noisy);
        snapshot.written_at = "changed".into();
        snapshot.links.frames_last_hour += 99;
        snapshot.links.last_frame_observed = Some("changed".into());
        snapshot.links.last_successful_publish = Some("changed".into());
        snapshot
            .inventory
            .rid_ble
            .as_mut()
            .unwrap()
            .rfkill
            .as_mut()
            .unwrap()
            .observed_at = "changed".into();
        assert_eq!(snapshot.meaningful(), expected);
    }

    #[test]
    fn periodic_refresh_events_and_one_second_rate_limit() {
        let dir = TempDir::new();
        let source = source(dir.0.join("status.json"));
        let h = heartbeat();
        let key = source.snapshot(&h).meaningful();
        let now = Instant::now();
        let first = br#"{"written_at":"2026-09-19T00:00:00Z"}"#;
        let refreshed = br#"{"written_at":"2026-09-19T00:00:30Z"}"#;
        assert!(source.commit(first, key.clone(), now).unwrap());
        assert!(!source
            .commit(refreshed, key.clone(), now + Duration::from_secs(29))
            .unwrap());
        assert_eq!(fs::read(&source.path).unwrap(), first);
        assert!(source
            .commit(refreshed, key.clone(), now + Duration::from_secs(30))
            .unwrap());
        assert_eq!(fs::read(&source.path).unwrap(), refreshed);
        let mut changed = key;
        changed.radio = RadioStatus::Down;
        assert!(!source
            .commit(b"{}", changed.clone(), now + Duration::from_millis(30_999))
            .unwrap());
        assert!(source
            .commit(b"{}", changed.clone(), now + Duration::from_secs(31))
            .unwrap());
        assert!(!source
            .commit(b"{}", changed.clone(), now + Duration::from_secs(32))
            .unwrap());
        assert!(source
            .commit(b"{}", changed, now + Duration::from_secs(61))
            .unwrap());
    }

    #[test]
    fn failed_write_does_not_record_success_or_suppress_retry() {
        let dir = TempDir::new();
        let parent = dir.0.join("later");
        let source = source(parent.join("status.json"));
        let h = heartbeat();
        let key = source.snapshot(&h).meaningful();
        let now = Instant::now();
        assert!(source.commit(b"{}", key.clone(), now).is_err());
        assert!(source.writer.lock().unwrap().last_success.is_none());
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!source
            .commit(b"{}", key.clone(), now + Duration::from_millis(999))
            .unwrap());
        assert!(source
            .commit(b"{}", key, now + Duration::from_secs(1))
            .unwrap());
    }

    #[test]
    fn parent_permission_guard_refuses_group_or_world_writes() {
        let dir = TempDir::new();
        let path = dir.0.join("status.json");
        for mode in [0o775, 0o757, 0o777] {
            fs::set_permissions(&dir.0, fs::Permissions::from_mode(mode)).unwrap();
            assert!(write_atomic(&path, b"{}")
                .unwrap_err()
                .to_string()
                .contains("group/world"));
            assert!(!path.exists());
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
        }
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
        write_atomic(&path, b"{}").unwrap();
    }

    #[test]
    fn startup_sweep_only_removes_our_regular_numeric_temps() {
        let dir = TempDir::new();
        let source = source(dir.0.join("status.json"));
        let stale = dir.0.join("status.json.tmp.99999.0");
        let reused_pid = dir
            .0
            .join(format!("status.json.tmp.{}.0", std::process::id()));
        fs::write(&stale, b"orphan").unwrap();
        fs::write(&reused_pid, b"orphan").unwrap();
        for name in [
            "other.json.tmp.1.0",
            "status.json.tmp.nope.0",
            "status.json.tmp.1.0.extra",
        ] {
            fs::write(dir.0.join(name), b"unrelated").unwrap();
        }
        let victim = dir.0.join("unrelated");
        fs::write(&victim, b"untouched").unwrap();
        let link = dir.0.join("status.json.tmp.777.0");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let subdir = dir.0.join("status.json.tmp.888.0");
        fs::create_dir(&subdir).unwrap();
        let h = heartbeat();
        assert!(source
            .commit(b"{}", source.snapshot(&h).meaningful(), Instant::now())
            .unwrap());
        assert!(!stale.exists());
        assert!(!reused_pid.exists());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(subdir.is_dir());
        assert_eq!(fs::read(&victim).unwrap(), b"untouched");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 7);
    }

    #[tokio::test]
    async fn shutdown_unlinks_and_prevents_late_or_future_renames() {
        for _ in 0..10 {
            let dir = TempDir::new();
            let source = source(dir.0.join("status.json"));
            let h = heartbeat();
            let (write, shutdown) = tokio::join!(source.write(&h), source.shutdown());
            write.unwrap();
            shutdown.unwrap();
            assert!(!source.path.exists());
            source.clone().write(&h).await.unwrap();
            assert!(!source.path.exists());
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
        }
    }

    fn spawn_emitter(
        source: Option<StatusSource>,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
    ) {
        spawn_emitter_at(source, 1, crate::heartbeat::RadioState::new())
    }

    fn spawn_emitter_at(
        source: Option<StatusSource>,
        interval: u64,
        radio: crate::heartbeat::RadioState,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let fleet = crate::heartbeat::FleetProprioception {
            identity: crate::identity::RunningIdentity::unverified(),
            silver: Some(crate::silver::Context {
                configured_position: crate::silver::ConfiguredPosition {
                    latitude: 40.0,
                    longitude: -95.0,
                    elevation_meters: 300.0,
                    source: crate::silver::ConfigSource::ConfigStatic,
                },
                config_hash: Some(format!("sha256:{}", "a".repeat(64))),
                fresh_for_ms: 30_000,
            }),
            channel: "stable".into(),
            time_trust: Arc::new(crate::clock_discipline::TimeTrust::new()),
        };
        let task = tokio::spawn(crate::heartbeat::run_heartbeat_emitter(
            "test-node".into(),
            radio,
            None,
            None,
            None,
            None,
            None,
            fleet,
            interval,
            source,
            move |_, bytes| {
                let _ = tx.try_send(bytes);
                async {}
            },
        ));
        (task, rx)
    }

    #[tokio::test]
    async fn meaningful_change_is_written_before_next_heartbeat_or_refresh() {
        let dir = TempDir::new();
        let source = source(dir.0.join("status.json"));
        let radio = crate::heartbeat::RadioState::new();
        let (task, mut rx) = spawn_emitter_at(Some(source.clone()), 10, radio.clone());
        let result = tokio::time::timeout(Duration::from_secs(8), async {
            while !source.path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            radio.set(RadioStatus::Error);
            loop {
                let status: Value =
                    serde_json::from_slice(&fs::read(&source.path).unwrap()).unwrap();
                if status["heartbeat"]["radio_status"] == "error" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // Status polling must NOT cause extra NATS heartbeat publications.
            assert!(matches!(
                rx.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ));
        })
        .await;
        task.abort();
        let _ = task.await;
        source.shutdown().await.unwrap();
        result.unwrap();
    }

    #[tokio::test]
    async fn heartbeat_tick_writes_same_payload_without_a_broker() {
        let dir = TempDir::new();
        let path = dir.0.join("status.json");
        let (task, mut rx) = spawn_emitter(Some(source(path.clone())));
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let published: Value = serde_json::from_slice(&rx.recv().await.unwrap()).unwrap();
            assert_eq!(
                published["update_blocked_reason"],
                "running_identity_unverified"
            );
            assert_eq!(published["silver_schema_version"], 1);
            assert_eq!(published["management_status"], "blocked");
            assert_eq!(published["position_status"]["state"], "no_fix");
            assert!(published.get("current_position").is_none());
            assert_eq!(published["configured_position"]["source"], "config_static");
            assert_eq!(published["product_version"], env!("CARGO_PKG_VERSION"));
            for key in ["image_digest", "engine_version", "build_seq", "policy_ack"] {
                assert!(
                    published.get(key).is_none(),
                    "unverified field emitted: {key}"
                );
            }
            loop {
                if let Ok(bytes) = fs::read(&path) {
                    let status: Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(status["heartbeat"], published);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        task.abort();
        let _ = task.await;
        result.unwrap();
    }

    #[tokio::test]
    async fn write_failure_does_not_stop_heartbeat_and_disabled_creates_nothing() {
        let dir = TempDir::new();
        // Missing parent forces a write failure, even for a root test user.
        let (task, mut rx) = spawn_emitter(Some(source(dir.0.join("missing/status.json"))));
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            assert!(rx.recv().await.is_some());
            assert!(rx.recv().await.is_some());
        })
        .await;
        task.abort();
        let _ = task.await;
        result.unwrap();
        let (task, mut rx) = spawn_emitter(None);
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await;
        task.abort();
        let _ = task.await;
        assert!(result.unwrap().is_some());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
    }
}
