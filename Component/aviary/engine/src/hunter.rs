// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
// Wave 7.1 — the Hunter task. The active observation layer of the
// Kittler Substrate Defense.
//
// Design rationale (2026-05-10): observe the spectrum and its noise floor,
// not only decoded telemetry. Move the radio to the channels that need
// attention, while preserving the established lineage tag on audit events.
//
// Scope (Increment 3):
//   • Round-robin channel rotation through a configurable channel set.
//   • Weighted dwell: priority channels get longer dwell time (the
//     "hunting where the birds are" heuristic).
//   • Tier-A soft self-heal on transient set_channel failures (one
//     retry; per the System-1/2-3 boundary committed with the independent
//     reviewer, deeper recovery rungs belong to brrdsupervisor in Wave
//     7.5, not the engine).
//   • Substrate-honest stdout log on every channel transition and
//     every recovery attempt.
//
// Out of scope until later increments:
//   • get_survey integration (continuous noise-floor telemetry) — Inc 4
//   • Lock-on detection (glue radio to channel after RID frame) — Inc 5
//   • Heartbeat-schema integration (current_channel + per-channel hits) — Inc 5
//   • Lineage-tagged audit events on substrate-attention — Inc 6

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::audit::SubstrateAuditEvent;
use crate::nl80211::{self, SurveyEntry};
use crate::watchdog::CaptureLiveness;

/// Lineage tag carried on every substrate-attention artifact this
/// module produces. The Kittler-Substrate-Defense provenance rides
/// in heartbeat payloads, audit events, and future Hardware
/// Reliability Index queries forever.
pub const KITTLER_LINEAGE: &str = "kittler-substrate-defense-v1";

/// Substrate-truth: the rtw88_8812au in-tree driver (covering Alfa
/// AWUS036ACS and many other RTL8812AU-based monitor adapters) accepts
/// NL80211_CMD_SET_CHANNEL cleanly but does NOT populate
/// NL80211_CMD_GET_SURVEY responses. The Wave 7.1 deploy on test-node-2
/// at the field test site (2026-05-10) surfaced this within ten minutes of
/// first heartbeat. `iw dev wlx<MAC> survey dump` returns empty too —
/// this is a driver-level capability gap, not an engine bug. See
/// `docs/src/substrate-device-identity.md` §3 (Coverage Class
/// Taxonomy) for the operator-facing framing.
///
/// The Hunter detects this empirically (observed survey emptiness
/// over a sample window) rather than via a hard-coded driver
/// blocklist — substrate-honest detection, future-proof against
/// other chipsets with the same gap. Custom-board chipset selection
/// for the summer 2026 build should require BOTH set_channel AND
/// get_survey actual implementation (not just advertisement) in
/// the chosen Wi-Fi silicon.
pub const SURVEY_CAPABILITY_SAMPLE_WINDOW: u64 = 10;

/// Default hard cap on lock-on duration (Wave 7.1b — reduced from
/// 5000ms to 2000ms). Once the capture loop sees ANY drone-class RID
/// frame, the Hunter glues the radio to the current channel for at
/// MOST this long. The actual lock duration is shorter when the
/// capture-budget is satisfied (see `LOCK_ON_MIN_MS_DEFAULT` +
/// `effective_lock_remaining_ms`).
///
/// Substrate-truth rationale for 2000ms: ASTM F3411-22a "at least 1
/// Hz" broadcast cadence; 2 seconds captures 2 full message cycles
/// even from non-Pack-mode drones (worst case). Closes the
/// 25-drone-on-different-channels starvation gap identified on
/// 2026-05-11. Override via `hunter.lock_on_duration_ms` in
/// config.yaml.
pub const LOCK_ON_DURATION_MS_DEFAULT: u64 = 2000;

/// Wave 7.1b — capture-budget minimum lock. After budget complete
/// (drone_id + position observed), the Hunter still holds the lock
/// for at least this long to catch follow-on frames. Below this
/// floor we'd rotate before the radio has actually settled.
pub const LOCK_ON_MIN_MS_DEFAULT: u64 = 500;

/// Empirically-observed coverage class for a sensor radio. The Hunter
/// derives this from observed substrate behavior (set_channel
/// success, survey-emptiness) rather than from a vendor blocklist —
/// substrate-honest, future-proof, and itself the kernel of the
/// Hardware Reliability Index claim (§6.4 of substrate-device-identity.md).
///
/// `Pending` is the substrate-truth-honest answer until we have
/// enough samples to commit to a claim. Operators reading this in
/// Bluejay should see "pending evaluation" for the first ~5 seconds
/// after engine start, then settle to LogicOnly or FullSigint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageClass {
    /// Insufficient samples yet to commit to a class. First
    /// SURVEY_CAPABILITY_SAMPLE_WINDOW attempts.
    Pending,
    /// Channel rotation works (set_channel succeeds), but get_survey
    /// returns consistently empty. The radio observes the protocol
    /// substrate (Wi-Fi/BLE/RID frames) but not the trail substrate
    /// (noise floor, channel busy %). Operationally fine for
    /// presence detection; cannot witness jamming or RF environment.
    LogicOnly,
    /// Both channel rotation and survey populate. The Sovereign
    /// Witness — full Kittler Substrate Defense capabilities.
    FullSigint,
    /// Capture or channel-set is broken. Substrate failure;
    /// operator intervention required.
    RadioError,
}

/// Per-channel rolling snapshot maintained by the Hunter task. Updated
/// at end of each dwell when get_survey returns. `busy_pct` is computed
/// at record-time as a delta against the previous sample on this
/// channel; the raw counters are retained for the next cycle's delta.
///
/// Channel-scheduler additions (C4): `dwell_ms_total` accumulates the
/// ACTUAL time spent dwelling on the channel (lock-on extensions
/// included — substrate truth, so an opted-in audit mode visibly
/// inflates its channel's share) and `rid_hits_total` counts decoded
/// RID observations received while on it. Entries are created by
/// dwell/hit recording even when the driver returns empty surveys
/// (rtw88_8812au `logic_only`), so share accounting never depends on
/// survey support.
#[derive(Clone, Debug, Default)]
pub struct ChannelSnapshot {
    pub noise_dbm: Option<i8>,
    pub busy_pct: Option<u8>,
    pub last_sample_unix_ms: u64,
    /// Carry the previous-sample raw counters so the next dwell can
    /// compute (active_delta, busy_delta) over a defined window.
    pub last_time_active_ms: u64,
    pub last_time_busy_ms: u64,
    /// Accumulated actual dwell milliseconds on this channel.
    pub dwell_ms_total: u64,
    /// Decoded RID observations received while dwelling on this channel.
    pub rid_hits_total: u64,
}

#[derive(Clone, Debug)]
pub struct HunterConfig {
    /// Channel-schedule plan (preset, channels, dwells, jitter, park).
    pub plan: crate::schedule::HunterPlan,
    /// Wave 7.1 Inc 6 — hard cap on lock-on duration. When the capture
    /// loop sees any drone-class RID frame, the Hunter defers channel
    /// rotation for at MOST this long (vendor-neutral; riding
    /// ingest_frame). ZERO in every preset default (C2): lock-on is an
    /// opt-in single-target audit tool, never a network-receiver mode.
    pub lock_on_duration: Duration,
    /// Wave 7.1b — minimum lock-on duration. After the capture-budget
    /// is satisfied (drone_id + position observed), the Hunter still
    /// holds the lock for at least this long to catch follow-on
    /// frames before resuming rotation. Audit mode only.
    pub lock_on_min: Duration,
}

/// Shared state surfaced to the heartbeat task. The Hunter task
/// writes; the heartbeat emitter reads.
///
/// Surfaces:
///   • `current_channel` — atomic, lock-free, the heartbeat reader's
///     fastest path
///   • `channels` — RwLock-guarded map of per-channel snapshots
///     (noise floor + busy% + age)
///   • `survey_attempts` / `survey_with_entries` — counters used to
///     derive the empirical CoverageClass per Wave 7.1 Inc 5.5
///     (substrate-honest detection of driver-level survey gaps)
///
/// Contention is negligible (Hunter writes once per dwell ~5 Hz,
/// heartbeat reads once per cycle ~0.2 Hz). Future Inc 6 (lock-on
/// detection) will add per-channel hits counters.
pub struct HunterState {
    pub current_channel: AtomicU32,
    pub channels: RwLock<HashMap<u32, ChannelSnapshot>>,
    /// Total get_survey calls attempted since startup.
    pub survey_attempts: std::sync::atomic::AtomicU64,
    /// get_survey calls that returned ≥1 entry. If this stays at 0
    /// after SURVEY_CAPABILITY_SAMPLE_WINDOW attempts, the driver is
    /// silently empty → coverage_class becomes `logic_only`.
    pub survey_with_entries: std::sync::atomic::AtomicU64,
    /// Wave 7.1 Inc 6 — lock-on absolute deadline (Unix ms). When
    /// the capture loop sees a drone-class frame, this is set to
    /// now + lock_on_duration. Hunter's dwell loop computes the
    /// effective sleep as max(dwell_end, lock_on_until), so the
    /// channel-switch is naturally deferred. Zero when not active.
    pub lock_on_until_unix_ms: std::sync::atomic::AtomicU64,
    /// Total lock-on triggers since startup. Telemetry signal:
    /// "how often did we glue to a channel because of a drone-class
    /// frame?" — feeds the Hardware Reliability Index downstream.
    pub lock_on_triggers: std::sync::atomic::AtomicU64,
    /// Wave 7.1b — capture-budget tracking. Within the current
    /// lock-on window, have we observed a non-UNKNOWN drone_id?
    /// Reset on each fresh lock cycle (when transition from inactive
    /// → active).
    pub lock_on_budget_drone_id_seen: std::sync::atomic::AtomicBool,
    /// Wave 7.1b — capture-budget tracking. Have we observed a
    /// non-zero (lat,lon) position? Reset on each fresh lock cycle.
    pub lock_on_budget_position_seen: std::sync::atomic::AtomicBool,
    /// Wave 7.1b — when the current lock cycle began (Unix ms).
    /// Used together with `lock_on_min` to enforce the floor on
    /// effective lock duration.
    pub lock_on_started_unix_ms: std::sync::atomic::AtomicU64,
    /// Wave 7.1b — count of lock cycles that released early because
    /// the capture-budget completed inside the window. Telemetry
    /// signal: how often is the budget-release saving us cycles?
    pub lock_on_budget_releases: std::sync::atomic::AtomicU64,
    /// Channel-scheduler S2 — measured `set_channel` round-trip time.
    /// The modelled retune dead time is a cited 8.5 ms same-silicon
    /// MT7921e PCIe proxy; these counters replace the proxy with the
    /// fleet's own measured value (last / max / count), reported in
    /// the heartbeat hunter block.
    pub retune_last_ms: std::sync::atomic::AtomicU64,
    pub retune_max_ms: std::sync::atomic::AtomicU64,
    pub retune_count: std::sync::atomic::AtomicU64,
}

impl HunterState {
    pub fn new() -> Arc<Self> {
        Arc::new(HunterState {
            current_channel: AtomicU32::new(0),
            channels: RwLock::new(HashMap::new()),
            survey_attempts: std::sync::atomic::AtomicU64::new(0),
            survey_with_entries: std::sync::atomic::AtomicU64::new(0),
            lock_on_until_unix_ms: std::sync::atomic::AtomicU64::new(0),
            lock_on_triggers: std::sync::atomic::AtomicU64::new(0),
            lock_on_budget_drone_id_seen: std::sync::atomic::AtomicBool::new(false),
            lock_on_budget_position_seen: std::sync::atomic::AtomicBool::new(false),
            lock_on_started_unix_ms: std::sync::atomic::AtomicU64::new(0),
            lock_on_budget_releases: std::sync::atomic::AtomicU64::new(0),
            retune_last_ms: std::sync::atomic::AtomicU64::new(0),
            retune_max_ms: std::sync::atomic::AtomicU64::new(0),
            retune_count: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// C4 — accumulate actual dwell time on a channel (lock-on
    /// extensions included). Creates the entry when surveys are empty
    /// so share accounting never depends on driver survey support.
    pub fn record_dwell(&self, channel: u32, ms: u64) {
        let mut map = match self.channels.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let snap = map.entry(channel).or_default();
        snap.dwell_ms_total = snap.dwell_ms_total.saturating_add(ms);
    }

    /// C4 — count a decoded RID observation received on a channel.
    pub fn record_rid_hit(&self, channel: u32) {
        let mut map = match self.channels.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let snap = map.entry(channel).or_default();
        snap.rid_hits_total = snap.rid_hits_total.saturating_add(1);
    }

    /// S2 — record a measured set_channel round-trip.
    pub fn record_retune(&self, ms: u64) {
        self.retune_last_ms.store(ms, Ordering::Relaxed);
        self.retune_max_ms.fetch_max(ms, Ordering::Relaxed);
        self.retune_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Capture loop calls this whenever `ingest_frame` returns
    /// `Some(TelemetryData)` — protocol-neutral, vendor-blind: ANY
    /// frame that parses as a valid drone RID broadcast triggers
    /// lock-on, regardless of which manufacturer's dialect it speaks.
    ///
    /// Multiple frames during a lock-on window each reset the
    /// deadline to `now + duration` — a continuously-broadcasting
    /// drone keeps the radio glued to its channel as long as it
    /// keeps speaking.
    ///
    /// Wave 7.1b: this method ALSO updates the capture-budget flags
    /// from the observed `TelemetryData`. A non-"UNKNOWN" drone_id
    /// flips `lock_on_budget_drone_id_seen`; an explicitly known position
    /// flips `lock_on_budget_position_seen`. These flags + the
    /// `lock_on_min_ms` floor drive `effective_lock_remaining_ms`,
    /// which is what the Hunter dwell loop actually consults.
    pub fn record_drone_observation(
        &self,
        drone_id: &str,
        position_known: bool,
        lock_on_max_ms: u64,
    ) {
        if lock_on_max_ms == 0 {
            return; // lock-on disabled
        }
        let now = now_unix_ms();
        let was_inactive = !self.lock_on_active();
        let deadline = now.saturating_add(lock_on_max_ms);
        self.lock_on_until_unix_ms
            .store(deadline, Ordering::Relaxed);
        self.lock_on_triggers.fetch_add(1, Ordering::Relaxed);

        if was_inactive {
            // Fresh lock cycle — reset budget flags + start timestamp.
            self.lock_on_budget_drone_id_seen
                .store(false, Ordering::Relaxed);
            self.lock_on_budget_position_seen
                .store(false, Ordering::Relaxed);
            self.lock_on_started_unix_ms.store(now, Ordering::Relaxed);
        }
        // Update budget flags from this observation.
        if !drone_id.is_empty() && drone_id != "UNKNOWN" {
            self.lock_on_budget_drone_id_seen
                .store(true, Ordering::Relaxed);
        }
        if position_known {
            self.lock_on_budget_position_seen
                .store(true, Ordering::Relaxed);
        }
    }

    /// Wave 7.1b — capture-budget-aware lock remaining. The Hunter
    /// dwell loop consults this (NOT `lock_on_remaining_ms`) so the
    /// radio releases the lock as soon as we have what we need
    /// (drone identity + position) AND have held the channel at
    /// least `min_lock_ms` to catch follow-on frames.
    ///
    /// Three-state policy:
    ///   • No lock active (deadline expired): return 0.
    ///   • Budget complete + elapsed ≥ min_lock_ms: return 0 (release).
    ///   • Budget complete + elapsed < min_lock_ms: return min_lock - elapsed.
    ///   • Budget incomplete: return raw remaining (full max-cap behavior).
    ///
    /// First call after release tracks the budget release counter
    /// so the heartbeat can surface "how often we saved cycles by
    /// capturing forensically-sufficient data quickly."
    pub fn effective_lock_remaining_ms(&self, min_lock_ms: u64) -> u64 {
        let raw_remaining = self.lock_on_remaining_ms();
        if raw_remaining == 0 {
            return 0;
        }
        let budget_complete = self.lock_on_budget_drone_id_seen.load(Ordering::Relaxed)
            && self.lock_on_budget_position_seen.load(Ordering::Relaxed);
        if !budget_complete {
            return raw_remaining;
        }
        let started = self.lock_on_started_unix_ms.load(Ordering::Relaxed);
        let elapsed = now_unix_ms().saturating_sub(started);
        if elapsed >= min_lock_ms {
            // Budget complete + minimum floor met → release immediately.
            // Count this as a budget-driven release for telemetry.
            self.lock_on_budget_releases.fetch_add(1, Ordering::Relaxed);
            // Clear the lock_on_until so subsequent reads return 0
            // until next observation fires it again.
            self.lock_on_until_unix_ms.store(0, Ordering::Relaxed);
            return 0;
        }
        min_lock_ms.saturating_sub(elapsed)
    }

    /// Hunter dwell loop reads this once per loop tick. Returns the
    /// number of milliseconds remaining in the lock-on window, or
    /// 0 when no lock-on is active. Substrate-truth: this is a
    /// pure clock-watch, no notifications, no async channels —
    /// the AtomicU64 read is wait-free.
    pub fn lock_on_remaining_ms(&self) -> u64 {
        let until = self.lock_on_until_unix_ms.load(Ordering::Relaxed);
        until.saturating_sub(now_unix_ms())
    }

    /// True if a lock-on window is currently active. Surfaced in
    /// heartbeat for operator visibility.
    pub fn lock_on_active(&self) -> bool {
        self.lock_on_remaining_ms() > 0
    }

    /// Hunter calls this after every survey attempt — success or
    /// silent-empty. The two counters together feed coverage_class().
    pub fn record_survey_attempt(&self, returned_entries: bool) {
        self.survey_attempts.fetch_add(1, Ordering::Relaxed);
        if returned_entries {
            self.survey_with_entries.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Derive the empirical CoverageClass for this radio. Substrate-
    /// honest: we report `Pending` until we've collected enough
    /// samples to make a substantiated claim, `LogicOnly` if the
    /// driver consistently returns empty surveys despite advertising
    /// the capability, and `FullSigint` if we observe any populated
    /// survey within the sample window.
    pub fn coverage_class(&self) -> CoverageClass {
        let attempts = self.survey_attempts.load(Ordering::Relaxed);
        let populated = self.survey_with_entries.load(Ordering::Relaxed);
        if attempts < SURVEY_CAPABILITY_SAMPLE_WINDOW {
            CoverageClass::Pending
        } else if populated == 0 {
            CoverageClass::LogicOnly
        } else {
            CoverageClass::FullSigint
        }
    }

    /// Hunter calls this at end of each dwell with the fresh survey
    /// dump and the channel_set we're currently scanning. Only
    /// channels in `channel_set` are recorded — survey entries for
    /// channels we don't actually visit would carry stale or zero
    /// counters and would be substrate-untruthful.
    ///
    /// busy_pct is computed as a delta against the previous stored
    /// sample on the same channel. Initial sample (no prior) leaves
    /// busy_pct = None.
    pub fn record_survey(&self, sample_unix_ms: u64, entries: &[SurveyEntry], channel_set: &[u32]) {
        let mut map = match self.channels.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        for entry in entries {
            let ch = match entry.channel {
                Some(c) => c,
                None => continue,
            };
            if !channel_set.contains(&ch) {
                continue;
            }
            let active_now = entry.time_active_ms.unwrap_or(0);
            let busy_now = entry.time_busy_ms.unwrap_or(0);

            let busy_pct = map.get(&ch).and_then(|prev| {
                let dt_active = active_now.saturating_sub(prev.last_time_active_ms);
                let dt_busy = busy_now.saturating_sub(prev.last_time_busy_ms);
                if dt_active > 0 {
                    let pct = dt_busy.saturating_mul(100) / dt_active;
                    Some(pct.min(100) as u8)
                } else {
                    None
                }
            });

            // Preserve the scheduler's dwell/hit accumulators when the
            // survey sample replaces the RF-facing fields.
            let (dwell_ms_total, rid_hits_total) = {
                let prev = map.get(&ch);
                (
                    prev.map(|p| p.dwell_ms_total).unwrap_or(0),
                    prev.map(|p| p.rid_hits_total).unwrap_or(0),
                )
            };
            map.insert(
                ch,
                ChannelSnapshot {
                    noise_dbm: entry.noise_dbm,
                    busy_pct,
                    last_sample_unix_ms: sample_unix_ms,
                    last_time_active_ms: active_now,
                    last_time_busy_ms: busy_now,
                    dwell_ms_total,
                    rid_hits_total,
                },
            );
        }
    }

    /// Heartbeat task calls this to read out the current snapshot.
    /// Returns (current_channel, Vec<(channel, snapshot)>). Sorted by
    /// channel number so wire output is stable for diffing.
    pub fn snapshot(&self) -> (u32, Vec<(u32, ChannelSnapshot)>) {
        let current = self.current_channel.load(Ordering::Relaxed);
        let map = match self.channels.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut entries: Vec<(u32, ChannelSnapshot)> =
            map.iter().map(|(ch, s)| (*ch, s.clone())).collect();
        entries.sort_by_key(|(ch, _)| *ch);
        (current, entries)
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
    fn coverage_class_pending_within_sample_window() {
        let state = HunterState::new();
        // No attempts yet → Pending.
        assert_eq!(state.coverage_class(), CoverageClass::Pending);
        // First few attempts, all empty → still Pending (below threshold).
        for _ in 0..(SURVEY_CAPABILITY_SAMPLE_WINDOW - 1) {
            state.record_survey_attempt(false);
        }
        assert_eq!(state.coverage_class(), CoverageClass::Pending);
    }

    #[test]
    fn coverage_class_logic_only_after_consistent_empty() {
        // Substrate-truth scenario: rtw88_8812au returns Ok(empty)
        // every time. Engine should empirically declare LogicOnly
        // once past the sample window without needing a vendor
        // blocklist.
        let state = HunterState::new();
        for _ in 0..SURVEY_CAPABILITY_SAMPLE_WINDOW {
            state.record_survey_attempt(false);
        }
        assert_eq!(state.coverage_class(), CoverageClass::LogicOnly);
    }

    #[test]
    fn coverage_class_full_sigint_when_survey_populates() {
        // A single populated survey within the window is enough to
        // declare FullSigint — the substrate has proven it can
        // observe the trail. Empty samples afterwards don't downgrade.
        let state = HunterState::new();
        state.record_survey_attempt(true);
        for _ in 0..SURVEY_CAPABILITY_SAMPLE_WINDOW {
            state.record_survey_attempt(false);
        }
        assert_eq!(state.coverage_class(), CoverageClass::FullSigint);
    }

    #[test]
    fn lock_on_inactive_by_default() {
        let state = HunterState::new();
        assert!(!state.lock_on_active());
        assert_eq!(state.lock_on_remaining_ms(), 0);
    }

    #[test]
    fn lock_on_record_observation_activates_window() {
        let state = HunterState::new();
        state.record_drone_observation("DJI-MAVIC-001", true, 2000);
        assert!(state.lock_on_active());
        let remaining = state.lock_on_remaining_ms();
        assert!(
            remaining > 1_500 && remaining <= 2_000,
            "lock_on_remaining_ms={} not in (1500, 2000]",
            remaining
        );
        assert_eq!(
            state
                .lock_on_triggers
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn lock_on_duration_zero_is_disabled() {
        let state = HunterState::new();
        state.record_drone_observation("DJI-MAVIC-001", true, 0);
        assert!(!state.lock_on_active());
        assert_eq!(
            state
                .lock_on_triggers
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn lock_on_budget_sets_flags_from_observation() {
        let state = HunterState::new();
        state.record_drone_observation("DJI-MAVIC-001", true, 2000);
        assert!(state
            .lock_on_budget_drone_id_seen
            .load(std::sync::atomic::Ordering::Relaxed));
        assert!(state
            .lock_on_budget_position_seen
            .load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn lock_on_budget_ignores_unknown_drone_id() {
        // Substrate-truth: "UNKNOWN" drone_id means parser didn't
        // find a Basic ID message in the Pack. Don't flip the
        // budget flag on that — we don't have identity yet.
        let state = HunterState::new();
        state.record_drone_observation("UNKNOWN", true, 2000);
        assert!(!state
            .lock_on_budget_drone_id_seen
            .load(std::sync::atomic::Ordering::Relaxed));
        assert!(state
            .lock_on_budget_position_seen
            .load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn lock_on_budget_ignores_zero_position() {
        // Substrate-truth: lat=0, lon=0 means drone hasn't acquired
        // GPS lock yet (or is broadcasting placeholder zeros).
        // Don't flip the budget flag — we don't have position yet.
        let state = HunterState::new();
        state.record_drone_observation("DJI-MAVIC-001", false, 2000);
        assert!(state
            .lock_on_budget_drone_id_seen
            .load(std::sync::atomic::Ordering::Relaxed));
        assert!(!state
            .lock_on_budget_position_seen
            .load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn effective_lock_remaining_returns_raw_when_budget_incomplete() {
        let state = HunterState::new();
        // Position-only observation (drone_id stays UNKNOWN)
        state.record_drone_observation("UNKNOWN", true, 2000);
        let raw = state.lock_on_remaining_ms();
        let effective = state.effective_lock_remaining_ms(500);
        // Budget incomplete → return raw (within microsecond tolerance)
        assert!(
            effective.abs_diff(raw) <= 2,
            "effective={}, raw={}, expected ~equal",
            effective,
            raw
        );
    }

    #[test]
    fn effective_lock_remaining_returns_zero_after_budget_complete_and_min_elapsed() {
        // Substrate-truth: drone_id + position both observed,
        // minimum lock time elapsed → effective remaining is 0
        // (the lock can release; Hunter can rotate).
        let state = HunterState::new();
        state.record_drone_observation("DJI-MAVIC-001", true, 2000);
        // Wait past the min_lock_ms floor.
        std::thread::sleep(std::time::Duration::from_millis(60));
        // With min_lock_ms = 50, after 60ms elapsed budget-release fires.
        let effective = state.effective_lock_remaining_ms(50);
        assert_eq!(
            effective, 0,
            "expected 0 after budget complete + min elapsed"
        );
        // Counter should have incremented.
        assert_eq!(
            state
                .lock_on_budget_releases
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn effective_lock_remaining_holds_until_min_lock_when_budget_complete_early() {
        // Substrate-truth: budget complete immediately, but we
        // haven't held the channel long enough yet → effective
        // remaining is min_lock - elapsed (positive).
        let state = HunterState::new();
        state.record_drone_observation("DJI-MAVIC-001", true, 2000);
        // Min lock 500ms, elapsed ~0 → effective should be ~500.
        let effective = state.effective_lock_remaining_ms(500);
        assert!(
            effective > 400 && effective <= 500,
            "expected ~500ms remaining, got {}",
            effective
        );
    }

    #[test]
    fn lock_on_new_cycle_resets_budget_flags() {
        // Substrate-truth: after lock releases, the next fresh
        // observation should reset budget flags (don't carry
        // stale state across distinct lock cycles).
        let state = HunterState::new();
        state.record_drone_observation("DJI-MAVIC-001", true, 100);
        // Wait for lock to fully expire.
        std::thread::sleep(std::time::Duration::from_millis(120));
        // Verify expired.
        assert!(!state.lock_on_active());
        // New observation, but only drone_id, no position
        state.record_drone_observation("DJI-MAVIC-002", false, 2000);
        // Budget flags should be: drone_id=true (new id seen), position=false (reset, not yet seen).
        assert!(state
            .lock_on_budget_drone_id_seen
            .load(std::sync::atomic::Ordering::Relaxed));
        assert!(!state
            .lock_on_budget_position_seen
            .load(std::sync::atomic::Ordering::Relaxed));
    }
}

/// Run the Hunter task. Spawns no children; it is itself the channel
/// rotation loop that should be passed to `tokio::spawn`. Runs for the
/// lifetime of the engine.
///
/// Substrate-truth note on timing: each `nl80211::set_channel` call
/// is a synchronous netlink round-trip (~1–10 ms) wrapped in
/// `tokio::task::spawn_blocking` so it does not stall the async
/// runtime. Effective dwell is `dwell_target − set_channel_latency`.
/// Hardware variation in the kernel-driver netlink response time is
/// itself one of the substrate-truth signals the Hardware Reliability
/// Index (Wave 7.6) will eventually aggregate.
///
/// Wave 7.1 Inc 7 — Tier-A self-heal events and survey failures are
/// published as structured `SubstrateAuditEvent`s on the optional
/// `audit_tx` channel. The Hunter uses `try_send` so a saturated
/// audit consumer never stalls the rotation loop (substrate-honest
/// posture: better to drop an audit event than miss a drone frame).
///
/// Wave 7.1 Inc 8 — the Hunter no longer caches the ifindex at
/// startup. It reads `liveness.current_ifindex` fresh at the top of
/// every dwell cycle, so when the watchdog (or the capture loop)
/// re-resolves the ifindex after a USB re-enumeration, the Hunter's
/// next set_channel automatically targets the new index. This is the
/// direct fix for the 2026-05-13 blind spot where the Hunter kept
/// issuing set_channel against a stale ifindex 5 (the device had
/// re-enumerated to 6) and reported success against nothing.
pub async fn run_hunter(
    iface: String,
    cfg: HunterConfig,
    state: Arc<HunterState>,
    node_id: String,
    audit_tx: Option<tokio::sync::mpsc::Sender<SubstrateAuditEvent>>,
    liveness: Arc<CaptureLiveness>,
) {
    let plan = &cfg.plan;
    if plan.channel_set.is_empty() {
        eprintln!("[hunter] empty channel set — Hunter will not start");
        return;
    }

    let mut scheduler = crate::schedule::Scheduler::new(plan);

    println!(
        "[hunter] starting (Kittler Substrate Defense): iface={} initial_ifindex={} \
         preset={:?} channels={:?} social={:?} dwell_social={}ms dwell_other={}ms \
         jitter=±{:.0}% parked={} lock_on={}ms \
         (ifindex re-read from CaptureLiveness every dwell — Inc 8)",
        iface,
        liveness.current_ifindex.load(Ordering::Relaxed),
        plan.preset,
        plan.channel_set,
        plan.social_channels,
        plan.dwell_social_ms,
        plan.dwell_other_ms,
        plan.jitter_fraction * 100.0,
        plan.parked,
        plan.lock_on_duration_ms,
    );
    if !plan.explicit_fields.is_empty() {
        println!(
            "[hunter] explicit config fields {:?} override preset defaults (back-compat)",
            plan.explicit_fields
        );
    }

    // Parked plans set the channel once; rotation plans re-set every visit.
    let mut parked_set = false;
    loop {
        // Inc 8: never cache the ifindex. Read it fresh every dwell so
        // a watchdog-driven re-resolution after re-enumeration is
        // picked up on the very next channel set.
        let ifindex = liveness.current_ifindex.load(Ordering::Relaxed);
        let visit = scheduler.next_visit();
        let channel = visit.channel;
        let dwell_ms = visit.dwell_ms;

        if !(plan.parked && parked_set) {
            // Primary channel-set attempt, timed for the S2 retune
            // telemetry (cited proxy 8.5 ms; this replaces it with the
            // fleet's own measured value).
            let primary = tokio::task::spawn_blocking(move || {
                let t0 = Instant::now();
                let r = nl80211::set_channel(ifindex, channel);
                (r, t0.elapsed().as_millis() as u64)
            })
            .await;

            match primary {
                Ok((Ok(()), ms)) => {
                    state.record_retune(ms);
                    state.current_channel.store(channel, Ordering::Relaxed);
                }
                Ok((Err(e), _)) => {
                    // Tier-A soft self-heal: re-issue once. If it sticks,
                    // fine; if not, audit-log and move on (next rotation
                    // iteration will try this channel again, which is its
                    // own form of recovery).
                    // Wave 7.1 Inc 7: publish the recovery attempt with
                    // provenance — every Tier-A becomes auditable in the
                    // §6.3 patent-claim sense.
                    let original_err = e.to_string();
                    eprintln!(
                        "[hunter] set_channel(ch{}) failed: {} — attempting tier-A self-heal",
                        channel, original_err
                    );
                    let healing_start = Instant::now();
                    let healing = tokio::task::spawn_blocking(move || {
                        let t0 = Instant::now();
                        let r = nl80211::set_channel(ifindex, channel);
                        (r, t0.elapsed().as_millis() as u64)
                    })
                    .await;
                    let elapsed_ms = healing_start.elapsed().as_millis() as u64;
                    let (success, combined_err) = match healing {
                        Ok((Ok(()), ms)) => {
                            state.record_retune(ms);
                            eprintln!("[hunter] tier-A self-heal ok for ch{}", channel);
                            state.current_channel.store(channel, Ordering::Relaxed);
                            (true, Some(original_err))
                        }
                        Ok((Err(e2), _)) => {
                            eprintln!(
                                "[hunter] tier-A self-heal failed for ch{}: {} — \
                                 remaining on previous channel until next rotation",
                                channel, e2
                            );
                            (false, Some(format!("{} | retry: {}", original_err, e2)))
                        }
                        Err(join_err) => {
                            eprintln!("[hunter] self-heal spawn_blocking join error: {}", join_err);
                            (
                                false,
                                Some(format!("{} | join: {}", original_err, join_err)),
                            )
                        }
                    };
                    if let Some(tx) = audit_tx.as_ref() {
                        let event = SubstrateAuditEvent::tier_a_recovery(
                            node_id.clone(),
                            channel,
                            success,
                            combined_err,
                            elapsed_ms,
                        );
                        let _ = tx.try_send(event); // best-effort; never block the hunt
                    }
                }
                Err(join_err) => {
                    eprintln!("[hunter] spawn_blocking join error: {}", join_err);
                }
            }
            if plan.parked {
                // Only mark a parked plan as set after at least one
                // attempt outcome; a failed park keeps retrying here.
                parked_set = state.current_channel.load(Ordering::Relaxed) == channel;
            }
        }

        // Wave 7.1 Inc 6 + 7.1b — dwell with capture-budget-aware
        // lock-on respect (audit mode only; ZERO by default — C2). The
        // lock loop consults effective_lock_remaining_ms, which releases
        // as soon as the capture budget (identity + position) is met AND
        // the min-lock floor has elapsed.
        let dwell_start = Instant::now();
        let dwell_end = now_unix_ms() + dwell_ms;
        let min_lock_ms = cfg.lock_on_min.as_millis() as u64;
        loop {
            let lock_remaining = state.effective_lock_remaining_ms(min_lock_ms);
            let lock_deadline = if lock_remaining > 0 {
                now_unix_ms() + lock_remaining
            } else {
                0
            };
            let effective_deadline = dwell_end.max(lock_deadline);
            let remaining = effective_deadline.saturating_sub(now_unix_ms());
            if remaining == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(remaining.min(50))).await;
        }

        // C4 — record the ACTUAL dwell (lock-on extensions included) on
        // the channel we actually listened to. Substrate truth: if a
        // set_channel failed we dwelled on the previous channel.
        let actual_channel = state.current_channel.load(Ordering::Relaxed);
        if actual_channel != 0 {
            state.record_dwell(actual_channel, dwell_start.elapsed().as_millis() as u64);
        }

        // Substrate-truth sample: take a survey snapshot at end of
        // dwell so we have fresh noise-floor + accumulated-counter
        // readings for the channel we just witnessed. Per-channel
        // busy_pct is computed as a delta against the previous
        // sample (see HunterState::record_survey).
        //
        // Substrate-truth gap surfaced 2026-05-10: the rtw88_8812au
        // in-tree driver returns Ok(empty Vec) silently — kernel
        // succeeds, driver doesn't populate. We track empirical
        // "returned at least one entry" alongside attempt count so
        // CoverageClass settles to LogicOnly within the sample
        // window without needing a hard-coded driver blocklist.
        let survey_result = tokio::task::spawn_blocking(move || nl80211::get_survey(ifindex)).await;
        match survey_result {
            Ok(Ok(entries)) => {
                let any_entries = !entries.is_empty();
                state.record_survey_attempt(any_entries);
                state.record_survey(now_unix_ms(), &entries, &plan.channel_set);
            }
            Ok(Err(e)) => {
                state.record_survey_attempt(false);
                let err_msg = e.to_string();
                eprintln!(
                    "[hunter] get_survey failed: {} — heartbeat will see stale data",
                    err_msg
                );
                // Wave 7.1 Inc 7: lift kernel-level survey errors onto
                // the substrate-audit channel. NOTE: this only fires
                // for actual Err returns, NOT for the rtw88_8812au
                // silent-empty case (which is reported by
                // coverage_class: logic_only in heartbeat, not as
                // a per-attempt failure).
                if let Some(tx) = audit_tx.as_ref() {
                    let event = SubstrateAuditEvent::survey_failure(node_id.clone(), err_msg);
                    let _ = tx.try_send(event);
                }
            }
            Err(join_err) => {
                state.record_survey_attempt(false);
                eprintln!("[hunter] survey spawn_blocking join error: {}", join_err);
            }
        }
    }
}
