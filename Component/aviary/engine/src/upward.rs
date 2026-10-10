// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! D40: opt-in, independent Silver report and Red distress lanes.
//! Contract/ACL/stream proposals: ../tools/UPWARD-REPORTING.md.
//! Producers never wait for a network operation or filesystem syscall.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::{jetstream, ConnectOptions};
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, timeout, Instant};

const MAX_PAYLOAD: usize = 4096;
const MAX_SPOOL: u64 = 256 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const IDLE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Presence opts in; deployment still requires the release approver's reviewed grants/streams.
    pub spool_dir: PathBuf,
    /// R1 Red boot-grace (seconds). WHAT: delay before a fresh session's distress
    /// codes are EMITTED on the Red lane (observation and the Silver operational
    /// lane are unchanged — unready is published throughout). WHY 300: GPS cold
    /// start 30–120 s; clock trust needs the first fix plus consistent_fixes
    /// (default 3) at 1 Hz; radio bring-up is seconds. A node that NEVER becomes
    /// healthy still pages at ≤ grace + one 5 s tick — never-healthy detection is
    /// delayed, never lost. WHEN-tune: 0 restores edge-triggered immediacy
    /// (today's behaviour); raise for known-slow indoor sites. DEPENDS: should
    /// stay ≥ sensors.gps.startup_grace_secs + 60 — raising the GPS grace past
    /// 240 without raising this violates the floor (Red pages while the
    /// operational gate still legitimately waits). Hard-clamped to 0..=3600.
    #[serde(default = "default_red_boot_grace_s")]
    pub red_boot_grace_s: u64,
}

fn default_red_boot_grace_s() -> u64 { 300 }

/// R1: grace clamp shared by main.rs and the tests.
pub fn clamped_grace_s(v: u64) -> u64 { v.min(3600) }

impl Config {
    pub fn validate(&self, node: &str) -> Result<(), String> {
        if !token(node) || !self.spool_dir.is_absolute() || self.spool_dir.file_name().is_none() {
            return Err("upward requires a single-token node ID and absolute private spool_dir".into());
        }
        Ok(())
    }
}

fn token(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Lane { Operational, Red }

impl Lane {
    fn capacity(self) -> usize { match self { Self::Operational => 8, Self::Red => 32 } }
    fn label(self) -> &'static str { match self { Self::Operational => "operational", Self::Red => "red" } }
    fn subject(self, node: &str) -> String {
        match self {
            Self::Operational => format!("cybrrd.silver.node.operational.{node}"),
            Self::Red => format!("cybrrd.red.algedonic.{node}"),
        }
    }
    fn stream(self) -> &'static str {
        match self { Self::Operational => "CYBRRD_OPERATIONAL", Self::Red => "CYBRRD_ALGEDONIC" }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    pub healthy: bool,
    pub identity_verified: bool,
    pub radio_up: bool,
    pub gps_required: bool,
    pub gps_healthy: bool,
    pub clock_trusted: bool,
}

impl Health {
    pub fn observe(identity_verified: bool, radio_up: bool, gps_required: bool,
                   gps_healthy: bool, clock_trusted: bool) -> Self {
        Self { healthy: identity_verified && radio_up && (!gps_required || (gps_healthy && clock_trusted)),
               identity_verified, radio_up, gps_required, gps_healthy, clock_trusted }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Distress {
    RunningIdentityUnverified,
    RadioUnavailable,
    RequiredGpsUnavailable,
    ClockUntrusted,
    OperationalDeliveryFailed,
    UpdateQuarantined,
    UpdateRolledBack,
    UpdateRollbackFailed,
}

/// Durable host-effector outcome; no keys or credentials cross this handoff.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateOutcome {
    schema: String, node_id: String, kind: String, target: String,
    attempts: u32, observed_unix_ms: u64,
}
impl UpdateOutcome {
    fn read(path: &Path, node: &str) -> Option<Self> {
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path).ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() > 4096 { return None; }
        let mut raw = Vec::new(); file.take(4097).read_to_end(&mut raw).ok()?;
        let value: Self = serde_json::from_slice(&raw).ok()?;
        let (engine, console) = value.target.split_once('+').map_or((value.target.as_str(), None), |(a,b)| (a,Some(b)));
        let valid = |image: &str, repo: &str| image.strip_prefix(repo).is_some_and(|digest|
            digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        if value.schema != "cybrrd.update.outcome.v1" || value.node_id != node
            || !valid(engine, "ghcr.io/cybrrd/brrdfeeder@sha256:")
            || console.is_some_and(|image| !valid(image,"ghcr.io/cybrrd/brrdhouse@sha256:"))
            || !matches!(value.kind.as_str(), "applied" | "rolled_back" | "quarantined" | "rollback_failed")
            || value.attempts > 2 { return None; }
        Some(value)
    }
    fn distress(&self) -> Option<Distress> {
        match self.kind.as_str() {
            "quarantined" => Some(Distress::UpdateQuarantined), // D44 MUTATE Red handoff
            "rolled_back" => Some(Distress::UpdateRolledBack),
            "rollback_failed" => Some(Distress::UpdateRollbackFailed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "schema")]
enum Message {
    #[serde(rename = "cybrrd.node.operational.v1")]
    Operational { image_digest: Option<String>, engine_version: Option<String>, build_seq: Option<u64>, health: Health,
        #[serde(default, skip_serializing_if = "Option::is_none")] update: Option<UpdateOutcome> },
    #[serde(rename = "cybrrd.red.algedonic.v1")]
    Algedonic { code: Distress, operational: Diagnostics,
        #[serde(default, skip_serializing_if = "Option::is_none")] update: Option<UpdateOutcome> },
}

impl Message {
    fn lane(&self) -> Lane { match self { Self::Operational { .. } => Lane::Operational, Self::Algedonic { .. } => Lane::Red } }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    message_id: String,
    node_id: String,
    observed_unix_ms: u64,
    /// Arrival time is never observation time. This may be false before GPS sync.
    clock_trusted: bool,
    test_only: bool,
    message: Message,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostics {
    pub pending: u64,
    pub shed_oldest: u64,
    pub rejected_handoff: u64,
    pub rejected_payload: u64,
    pub attempts: u64,
    pub acknowledged: u64,
    pub failures: u64,
    /// 0=none, 1=spool, 2=connect/credentials, 3=publish/PubAck/permission/stream.
    pub fault: u8,
}

#[derive(Default)]
struct Status {
    pending: AtomicU64, shed: AtomicU64, rejected: AtomicU64, oversized: AtomicU64,
    attempts: AtomicU64, acknowledged: AtomicU64, failures: AtomicU64, fault: AtomicU8,
}
impl Status {
    fn snapshot(&self) -> Diagnostics {
        Diagnostics { pending: self.pending.load(Ordering::Relaxed), shed_oldest: self.shed.load(Ordering::Relaxed),
            rejected_handoff: self.rejected.load(Ordering::Relaxed), rejected_payload: self.oversized.load(Ordering::Relaxed),
            attempts: self.attempts.load(Ordering::Relaxed), acknowledged: self.acknowledged.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed), fault: self.fault.load(Ordering::Relaxed) }
    }
    fn failed(&self, lane: Lane, fault: u8) {
        self.fault.store(fault, Ordering::Relaxed);
        let count = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        // Bounded by worker backoff, not by producer rate. No credentials/server errors.
        eprintln!("[upward] lane={} failure={} count={} retained={} engine_continues=true", lane.label(), fault, count, self.pending.load(Ordering::Relaxed));
    }
    /// Bookkeeping for one acknowledged delivery (a real PubAck). Extracted so
    /// the fault-clear semantics are unit-testable: an ack must clear a fault
    /// whose origin was the publish/transport path (`failed()`), not only the
    /// disk-durability path.
    fn delivered(&self, durable: bool) {
        self.acknowledged.fetch_add(1, Ordering::Relaxed);
        self.fault.store(if durable { 0 } else { 1 }, Ordering::Relaxed);
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Queue {
    records: VecDeque<Record>,
    shed_oldest: u64,
    #[serde(skip)]
    generation: u64,
}
impl Queue {
    fn push(&mut self, record: Record, lane: Lane) {
        if self.records.len() >= lane.capacity() {
            self.records.pop_front(); // D40 mutation target: oldest of THIS lane only.
            self.shed_oldest = self.shed_oldest.saturating_add(1);
        }
        self.records.push_back(record);
        self.generation = self.generation.wrapping_add(1);
    }
    fn retire(&mut self, id: &str) {
        self.records.retain(|r| r.message_id != id);
        self.generation = self.generation.wrapping_add(1);
    }
}

#[derive(Clone)]
struct Channel { lane: Lane, queue: Arc<Mutex<Queue>>, status: Arc<Status> }
impl Channel {
    fn new(lane: Lane) -> Self { Self { lane, queue: Arc::new(Mutex::new(Queue::default())), status: Arc::new(Status::default()) } }
    fn enqueue(&self, record: Record) -> bool {
        if record.message.lane() != self.lane || serde_json::to_vec(&record).map_or(true, |v| v.len() > MAX_PAYLOAD) {
            self.status.oversized.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        // No waiting even if the worker holds the short memory-only lock.
        match self.queue.try_lock() {
            Ok(mut queue) => { queue.push(record, self.lane); self.account(&queue); true }
            Err(_) => { self.status.rejected.fetch_add(1, Ordering::Relaxed); false }
        }
    }
    fn account(&self, q: &Queue) {
        self.status.pending.store(q.records.len() as u64, Ordering::Relaxed);
        self.status.shed.store(q.shed_oldest, Ordering::Relaxed);
    }
}

/// One file + one atomic replacement file per lane, each at most 256 KiB.
/// Separate advisory locks prevent two engine instances sharing one lane store.
struct Store { path: PathBuf, _lease: File }
impl Store {
    fn open(root: &Path, lane: Lane, node: &str) -> std::io::Result<(Self, Queue)> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(root)?;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
        let lease = OpenOptions::new().create(true).read(true).write(true).mode(0o600)
            .custom_flags(libc::O_NOFOLLOW).open(root.join(format!("{}.lock", lane.label())))?;
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let path = root.join(format!("{}.json", lane.label()));
        let queue = match OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Queue::default(),
            Err(e) => return Err(e),
            Ok(file) => {
                if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_SPOOL { return Err(invalid("invalid spool file")); }
                let mut bytes = Vec::new(); file.take(MAX_SPOOL + 1).read_to_end(&mut bytes)?;
                let q: Queue = serde_json::from_slice(&bytes).map_err(|_| invalid("corrupt spool retained"))?;
                let mut ids = std::collections::HashSet::new();
                if q.records.len() > lane.capacity() || q.records.iter().any(|r| r.node_id != node ||
                    r.message.lane() != lane || r.message_id.is_empty() || r.message_id.len() > 160 ||
                    !ids.insert(r.message_id.clone()) || serde_json::to_vec(r).map_or(true, |v| v.len() > MAX_PAYLOAD)) {
                    return Err(invalid("spool identity or bound mismatch; retained"));
                }
                q
            }
        };
        Ok((Self { path, _lease: lease }, queue))
    }
    fn save(path: &Path, queue: &Queue) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(queue).map_err(|_| invalid("spool serialization"))?;
        if bytes.len() as u64 > MAX_SPOOL { return Err(invalid("spool byte bound")); }
        let temporary = path.with_extension("tmp");
        let mut file = OpenOptions::new().create(true).truncate(true).write(true).mode(0o600)
            .custom_flags(libc::O_NOFOLLOW).open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        let parent = path.parent().ok_or_else(|| invalid("spool parent"))?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }
}
fn invalid(s: &str) -> std::io::Error { std::io::Error::new(std::io::ErrorKind::InvalidData, s) }

#[derive(Clone)]
struct Connection {
    urls: Vec<String>, credentials: String,
    #[cfg(test)]
    test_user: Option<(String, String)>,
}
impl Connection {
    async fn connect(&self, node: &str, lane: Lane) -> Result<jetstream::Context, ()> {
        #[cfg(test)]
        let opts = if let Some((user, password)) = &self.test_user {
            ConnectOptions::new().user_and_password(user.clone(), password.clone())
        } else { ConnectOptions::with_credentials_file(&self.credentials).await.map_err(|_| ())? };
        #[cfg(not(test))]
        let opts = ConnectOptions::with_credentials_file(&self.credentials).await.map_err(|_| ())?;
        let client = opts.custom_inbox_prefix(format!("_INBOX.{node}.upward.{}", lane.label()))
            .connection_timeout(IO_TIMEOUT).connect(self.urls.join(",")).await.map_err(|_| ())?;
        let mut context = jetstream::new(client);
        context.set_timeout(IO_TIMEOUT);
        Ok(context)
    }
}

fn retry_delay(failures: u32) -> Duration {
    // Deterministically bounded 1,2,4,8,16,30 seconds; never immediate retry.
    Duration::from_secs((1u64 << failures.min(5)).min(30))
}

async fn publish(js: &jetstream::Context, lane: Lane, record: &Record) -> Result<(), ()> {
    let payload = serde_json::to_vec(record).map_err(|_| ())?;
    let future = js.send_publish(lane.subject(&record.node_id), jetstream::message::PublishMessage::build()
        .payload(payload.into()).message_id(&record.message_id).expected_stream(lane.stream())).await.map_err(|_| ())?;
    let ack = future.await.map_err(|_| ())?;
    if ack.stream != lane.stream() { return Err(()); }
    Ok(())
}

async fn worker(channel: Channel, root: PathBuf, node: String, connection: Connection, stop: Arc<AtomicBool>) {
    let mut store: Option<Store> = None;
    let mut saved_generation: Option<u64> = None;
    let mut js = None;
    let mut failures = 0;
    let mut next_attempt = Instant::now();
    let mut next_disk = Instant::now();
    let mut disk_failures = 0;
    let mut last_drops = (0, 0, 0);
    while !stop.load(Ordering::Relaxed) {
        let stats = channel.status.snapshot();
        let drops = (stats.shed_oldest, stats.rejected_handoff, stats.rejected_payload);
        if drops != last_drops {
            eprintln!("[upward] lane={} shed_oldest={} rejected_handoff={} rejected_payload={} pending={}",
                      channel.lane.label(), drops.0, drops.1, drops.2, stats.pending);
            last_drops = drops;
        }
        // Store recovery is retried on the same bounded schedule as transport.
        if store.is_none() && Instant::now() >= next_disk {
            let (dir, id, lane) = (root.clone(), node.clone(), channel.lane);
            match tokio::task::spawn_blocking(move || Store::open(&dir, lane, &id)).await {
                Ok(Ok((opened, mut recovered))) => {
                    if let Ok(mut queue) = channel.queue.lock() {
                        for r in queue.records.drain(..) { recovered.push(r, channel.lane); }
                        recovered.shed_oldest = recovered.shed_oldest.saturating_add(queue.shed_oldest);
                        recovered.generation = queue.generation.wrapping_add(1);
                        *queue = recovered;
                        channel.account(&queue);
                        store = Some(opened);
                    }
                }
                _ => {
                    channel.status.failed(channel.lane, 1);
                    next_disk = Instant::now() + retry_delay(disk_failures);
                    disk_failures = (disk_failures + 1).min(5);
                }
            }
        }
        let snapshot = channel.queue.lock().ok().map(|q| q.clone());
        let Some(snapshot) = snapshot else { sleep(IDLE).await; continue; };
        let mut durable = saved_generation == Some(snapshot.generation);
        if let Some(opened) = &store {
            if !durable && Instant::now() >= next_disk {
                let (path, copy) = (opened.path.clone(), snapshot.clone());
                match tokio::task::spawn_blocking(move || Store::save(&path, &copy)).await {
                    Ok(Ok(())) => {
                        saved_generation = Some(snapshot.generation); durable = true;
                        disk_failures = 0; next_disk = Instant::now();
                    }
                    _ => {
                        channel.status.failed(channel.lane, 1);
                        next_disk = Instant::now() + retry_delay(disk_failures);
                        disk_failures = (disk_failures + 1).min(5);
                    }
                }
            }
        }
        if Instant::now() < next_attempt { sleep(IDLE).await; continue; }
        // Routine reports wait for durable staging. Red can still escape a broken
        // filesystem, with a visible durability fault and bounded in-memory retry.
        if !durable && channel.lane == Lane::Operational {
            next_attempt = Instant::now() + retry_delay(failures);
            failures = (failures + 1).min(5);
            sleep(IDLE).await; continue;
        }
        let Some(record) = snapshot.records.front() else { sleep(IDLE).await; continue; };
        channel.status.attempts.fetch_add(1, Ordering::Relaxed);
        if js.is_none() {
            match timeout(IO_TIMEOUT, connection.connect(&node, channel.lane)).await {
                Ok(Ok(context)) => js = Some(context),
                _ => channel.status.failed(channel.lane, 2),
            }
        }
        let delivered = if let Some(context) = &js {
            matches!(timeout(IO_TIMEOUT, publish(context, channel.lane, record)).await, Ok(Ok(())))
        } else { false };
        if delivered { // D40 mutation target: retirement requires a real PubAck.
            if let Ok(mut q) = channel.queue.lock() { q.retire(&record.message_id); channel.account(&q); }
            channel.status.delivered(durable);
            eprintln!("[upward] lane={} acknowledged={} durable_staging={} duplicate_delivery_possible=true",
                      channel.lane.label(), record.message_id, durable);
            failures = 0;
            next_attempt = Instant::now();
        } else {
            channel.status.failed(channel.lane, 3);
            js = None; // independent connection, including after ACL/stream failures
            next_attempt = Instant::now() + retry_delay(failures);
            failures = (failures + 1).min(5);
        }
        sleep(IDLE).await;
    }
    // Best-effort durable final snapshot. Engine shutdown's existing outer bound
    // still governs a filesystem that never returns; no network drain is awaited.
    if let Some(opened) = store {
        let snapshot = channel.queue.lock().ok().map(|q| q.clone());
        if let Some(q) = snapshot { let _ = tokio::task::spawn_blocking(move || Store::save(&opened.path, &q)).await; }
    }
}

pub struct Handle {
    node: String, nonce: String, sequence: AtomicU64,
    operational: Channel, red: Channel, stop: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Handle {
    fn record(&self, message: Message, clock_trusted: bool) -> Record {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        Record { message_id: format!("{}:{}:{sequence}", self.node, self.nonce), node_id: self.node.clone(),
            observed_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
            clock_trusted, test_only: cfg!(test), message }
    }
    #[cfg(test)]
    pub fn operational(&self, identity: &crate::identity::RunningIdentity, health: Health) -> bool {
        self.operational_update(identity, health, None)
    }
    fn operational_update(&self, identity: &crate::identity::RunningIdentity, mut health: Health, update: Option<UpdateOutcome>) -> bool {
        health.identity_verified = identity.verified_policy_floor().is_some();
        health.healthy = health.identity_verified && health.radio_up
            && (!health.gps_required || (health.gps_healthy && health.clock_trusted));
        let trusted = health.clock_trusted;
        self.operational.enqueue(self.record(Message::Operational { image_digest: identity.image_digest.clone(),
            engine_version: identity.engine_version.clone(), build_seq: identity.build_seq, health, update }, trusted))
    }
    pub fn alarm(&self, code: Distress, clock_trusted: bool) -> bool {
        self.red.enqueue(self.record(Message::Algedonic { code, operational: self.operational.status.snapshot(), update: None }, clock_trusted))
    }
    fn update_alarm(&self, outcome: &UpdateOutcome, clock_trusted: bool) -> bool {
        let Some(code) = outcome.distress() else { return true; };
        self.red.enqueue(self.record(Message::Algedonic { code, operational: self.operational.status.snapshot(), update: Some(outcome.clone()) }, clock_trusted))
    }
    pub fn routine_failed(&self) -> bool { self.operational.status.fault.load(Ordering::Relaxed) != 0 }
    pub async fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for task in self.tasks.drain(..) { let _ = task.await; }
    }
}

pub fn start(config: &Config, node: &str, urls: Vec<String>, credentials: String) -> Result<Handle, String> {
    config.validate(node)?;
    // Nonblocking kernel randomness, not wall-clock uniqueness on an RTC-less node.
    let mut nonce = [0u8; 16];
    if unsafe { libc::getrandom(nonce.as_mut_ptr().cast(), nonce.len(), libc::GRND_NONBLOCK) } != nonce.len() as isize {
        return Err("upward session identity unavailable".into());
    }
    let connection = Connection { urls, credentials, #[cfg(test)] test_user: None };
    Ok(launch(config, node, hex::encode(nonce), connection))
}
fn launch(config: &Config, node: &str, nonce: String, connection: Connection) -> Handle {
    let operational = Channel::new(Lane::Operational);
    let red = Channel::new(Lane::Red);
    let stop = Arc::new(AtomicBool::new(false));
    let tasks = [&operational, &red].into_iter().map(|channel| tokio::spawn(worker(
        channel.clone(), config.spool_dir.clone(), node.to_string(), connection.clone(), stop.clone()))).collect();
    Handle { node: node.into(), nonce, sequence: AtomicU64::new(0), operational, red, stop, tasks }
}

/// Independent of the old heartbeat queue. One report every 30s and at health
/// transitions; alarm once per fault episode, retrying handoff refusal next tick.
pub async fn monitor(mut handle: Handle, identity: crate::identity::RunningIdentity,
    radio: crate::heartbeat::RadioState, gps: Arc<arc_swap::ArcSwap<crate::sensor::SensorHealth>>,
    time: Arc<crate::clock_discipline::TimeTrust>, gps_required: bool,
    red_boot_grace: Duration,
    cancel: tokio_util::sync::CancellationToken) {
    let _ = red_boot_grace; // RED stub: gating lands with the implementation.
    let mut last_health = None;
    let mut last_report = Instant::now() - Duration::from_secs(30);
    let mut previous = Vec::new();
    let mut last_update = None;
    loop {
        let node = handle.node.clone();
        let update = tokio::task::spawn_blocking(move || UpdateOutcome::read(
            Path::new("/var/lib/brrdfeeder/update_outcome.json"), &node)).await.ok().flatten();
        let health = Health::observe(identity.verified_policy_floor().is_some(),
            radio.get() == crate::heartbeat::RadioStatus::Up, gps_required,
            gps.load().state == crate::sensor::SensorState::Healthy, time.is_trusted());
        if last_health.as_ref() != Some(&health) || last_update != update || last_report.elapsed() >= Duration::from_secs(30) {
            if handle.operational_update(&identity, health.clone(), update.clone()) { last_report = Instant::now(); last_health = Some(health.clone()); }
        }
        if last_update != update {
            if update.as_ref().is_none_or(|outcome| handle.update_alarm(outcome, health.clock_trusted)) { last_update = update; }
        }
        let mut faults = Vec::new();
        if !health.identity_verified { faults.push(Distress::RunningIdentityUnverified); }
        if !health.radio_up { faults.push(Distress::RadioUnavailable); }
        if gps_required && !health.gps_healthy { faults.push(Distress::RequiredGpsUnavailable); }
        if gps_required && !health.clock_trusted { faults.push(Distress::ClockUntrusted); }
        if handle.routine_failed() { faults.push(Distress::OperationalDeliveryFailed); }
        previous.retain(|code| faults.contains(code));
        for code in faults {
            if !previous.contains(&code) && handle.alarm(code, health.clock_trusted) { previous.push(code); }
        }
        tokio::select! { _ = cancel.cancelled() => { handle.shutdown().await; return; }, _ = sleep(Duration::from_secs(5)) => {} }
    }
}

#[cfg(test)]
#[path = "upward_tests.rs"]
mod tests;
