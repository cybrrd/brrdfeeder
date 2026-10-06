// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Cadence-aware, deadline-fair Wi-Fi channel schedule.
//!
//! The receiver's objective is the SET of aircraft in range, not a good
//! track for whichever aircraft spoke first. The first scheduler dwelt on
//! a channel to follow a drone it had just heard (dwell-to-follow) — that
//! made the node a single-aircraft tracker. Its successor rotated blindly
//! at 200/400 ms and taxed ≥1 Hz Remote ID: the 24-h capture's ~4–5 s
//! per-aircraft observation gaps matched the 3.8 s rotation cycle, so the
//! apparent "DJI ~4.3 s cadence floor" was our own artifact.
//!
//! This module fixes both failure modes:
//!   - SOCIAL LONG DWELL: channels 6 and 149 (NAN mandatory / optional per
//!     `BWF0090`; Beacon social channels per `BWFB0030`) dwell long enough
//!     to cover two NAN discovery-window periods (512 TU = 524.288 ms each)
//!     and a ≥1 s `BUR0010` Beacon interval with high probability;
//!   - FAIR SWEEP: every other configured channel still gets a short,
//!     cadence-covering visit (220 ms ≥ the 200 TU any-channel Beacon
//!     interval of `BWFB0040`/`BWFB0050`) each supercycle;
//!   - JITTER: dwell length is jittered ±15% (social floor 1000 ms) and the
//!     sweep order is re-shuffled every supercycle, so a transmitter whose
//!     cadence divides the cycle can never phase-lock against us;
//!   - NO AIRCRAFT KEYING: nothing in the schedule is influenced by any
//!     aircraft's traffic. Aircraft-following lock-on is off by default
//!     (see `HunterYaml::lock_on_duration_ms`, the opt-in single-target
//!     audit tool).
//!
//! Everything here is deterministic for a given seed: the schedule and the
//! pre-registered simulator below are reproducible in CI.

use crate::node_config::HunterYaml;

/// One refused `channel_set` entry with its reason. Refusals fail closed at
/// startup (see main's hunter-plan gate): the resolved plan never schedules
/// the channel, and the operator gets one actionable message instead of a
/// per-cycle runtime error from the kernel/regulatory layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedChannel {
    pub channel: u32,
    pub reason: String,
}

/// UNII-2 (52–144) requires radar detection and clearance before transmit;
/// a passive receiver can sit there, but the hunter's channel-setting
/// primitive gets refused or silently no-ops per cycle on most drivers, and
/// the default set deliberately excludes it. Refused at plan level so the
/// dead visit never exists.
fn is_dfs_channel(channel: u32) -> bool {
    (52..=144).contains(&channel)
}

/// NAN Discovery Window period: 512 TU, 1 TU = 1.024 ms (Wi-Fi Alliance
/// NAN specification; F3411 `BWF0032`/`BWF0036` place sync beacons and
/// SDFs inside these windows).
pub const NAN_DW_PERIOD_MS: f64 = 524.288;

/// Social-channel dwell, social preset: spans two full NAN discovery-window
/// periods (2 × 524.288 = 1048.576 ms) plus retune/jitter margin, and
/// gives P(catch) = 1 − e^(−1.2) ≈ 70% per visit against a 1 Hz Beacon.
/// WHAT: default long dwell on channels 6 and 149. WHY: see module docs.
/// WHEN-to-tune: raise if field data shows NAN windows missed at the edges;
/// lower only if sweep starvation hurts more than social yield.
/// DEPENDS-ON: `BWF0090` (social channels), `BUR0010` (≥1 Hz dynamic).
pub const SOCIAL_DWELL_MS: u64 = 1200;

/// Minimum social dwell AFTER jitter: the ≥1 s Beacon interval and one
/// full NAN discovery window plus margin must survive the ±15% jitter.
pub const SOCIAL_MIN_DWELL_MS: u64 = 1000;

/// Sweep dwell: 220 ms covers the 200 TU (204.8 ms) any-channel Beacon
/// interval (`BWFB0040`/`BWFB0050`) with guard time. WHAT/WHY/WHEN/DEPENDS
/// as above; lower trades discovery of conformant any-channel broadcasters
/// for shorter cycles.
pub const OTHER_DWELL_MS: u64 = 220;

/// Legacy (pre-scheduler) rotation values, kept for the `sweep` preset.
pub const LEGACY_DWELL_PRIORITY_MS: u64 = 400;
pub const LEGACY_DWELL_DEFAULT_MS: u64 = 200;

/// Dwell jitter fraction (±15%). Anti-phase-lock: a periodic transmitter
/// exactly aliquot with the cycle can never be systematically missed.
pub const JITTER_FRACTION: f64 = 0.15;

/// Default deterministic jitter seed. Fixed so CI and fleet agree.
pub const JITTER_SEED: u64 = 0x0BADC0DE_5EED_0001;

/// Named hunter presets (config `capture.hunter.preset`).
///
/// - `social` (DEFAULT): long dwell on 6/149, fair jittered sweep of the
///   rest. The network-receiver schedule.
/// - `park6`: park the first social channel (6 unless absent from the
///   configured set). No rotation, no retune churn — for dense single-band
///   sites that accept blindness elsewhere.
/// - `sweep`: the legacy 200/400 ms rotation values and priority list —
///   for spectrum-witness missions. Aircraft-following lock-on stays OFF
///   in every preset; it remains available only as the explicit opt-in
///   `lock_on_duration_ms > 0` single-target audit mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HunterPreset {
    Social,
    Park6,
    Sweep,
}

impl Default for HunterPreset {
    fn default() -> Self {
        HunterPreset::Social
    }
}

/// The resolved, runtime-truth schedule: preset defaults with the
/// operator's explicit values layered on top. Built via
/// [`HunterPlan::from_yaml`].
#[derive(Debug, Clone, PartialEq)]
pub struct HunterPlan {
    pub preset: HunterPreset,
    /// Full visited channel set (post park-reduction when parked).
    pub channel_set: Vec<u32>,
    /// Social channels present in `channel_set` (6 and 149 by default).
    pub social_channels: Vec<u32>,
    pub dwell_social_ms: u64,
    pub dwell_other_ms: u64,
    /// 0 in every preset default (C2); explicit opt-in only.
    pub lock_on_duration_ms: u64,
    pub lock_on_min_ms: u64,
    pub jitter_fraction: f64,
    pub jitter_seed: u64,
    /// True when the plan parks one channel instead of rotating.
    pub parked: bool,
    /// YAML keys the operator set explicitly (for the back-compat notice).
    pub explicit_fields: Vec<&'static str>,
    /// Duplicate entries dropped from the configured `channel_set`
    /// (first occurrence wins). Logged at startup as the dedupe notice.
    pub deduped_channels: Vec<u32>,
    /// Channels refused at plan level with reasons; non-empty → the engine
    /// fails closed at startup (main's hunter-plan gate).
    pub refused_channels: Vec<RefusedChannel>,
}

impl HunterPlan {
    /// Resolve the plan from YAML: preset defaults, explicit values win.
    pub fn from_yaml(y: &HunterYaml) -> Self {
        let mut explicit = Vec::new();
        let preset = y.preset.unwrap_or_default();

        let default_set = default_channel_set();
        let configured_set = match &y.channel_set {
            Some(set) => {
                explicit.push("channel_set");
                set.clone()
            }
            None => default_set,
        };
        // Channel-set hygiene (operator-error class): duplicates silently
        // double a channel's share — dedupe, first occurrence wins, and
        // record what was dropped for the startup notice. Unsupported
        // channels (outside the regulatory range we scan) and DFS channels
        // (52–144, radar-clearance unsupported) are refused: recorded here,
        // never scheduled, and main fails closed on a non-empty refusal
        // list before any radio administration.
        let mut deduped_channels = Vec::new();
        let mut refused_channels = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let channel_set = configured_set
            .into_iter()
            .filter(|&ch| {
                if !seen.insert(ch) {
                    deduped_channels.push(ch);
                    return false;
                }
                if let Err(reason) = crate::nl80211::channel_to_freq_mhz(ch) {
                    refused_channels.push(RefusedChannel {
                        channel: ch,
                        reason,
                    });
                    return false;
                }
                if is_dfs_channel(ch) {
                    refused_channels.push(RefusedChannel {
                        channel: ch,
                        reason: "DFS channel (UNII-2, 52-144): radar clearance is unsupported"
                            .to_string(),
                    });
                    return false;
                }
                true
            })
            .collect::<Vec<u32>>();

        let (dwell_social_ms, dwell_other_ms, social_channels) = match preset {
            HunterPreset::Social | HunterPreset::Park6 => {
                let social = default_social_channels()
                    .into_iter()
                    .filter(|c| channel_set.contains(c))
                    .collect::<Vec<u32>>();
                (SOCIAL_DWELL_MS, OTHER_DWELL_MS, social)
            }
            HunterPreset::Sweep => {
                // Legacy behaviour: the priority channels of the old
                // rotation (including 6/149 among them) at 400 ms.
                let social = legacy_priority_channels()
                    .into_iter()
                    .filter(|c| channel_set.contains(c))
                    .collect::<Vec<u32>>();
                (LEGACY_DWELL_PRIORITY_MS, LEGACY_DWELL_DEFAULT_MS, social)
            }
        };
        let dwell_social_ms = y.dwell_priority_ms.unwrap_or_else(|| {
            if matches!(preset, HunterPreset::Sweep) {
                LEGACY_DWELL_PRIORITY_MS
            } else {
                dwell_social_ms
            }
        });
        if y.dwell_priority_ms.is_some() {
            explicit.push("dwell_priority_ms");
        }
        let dwell_other_ms = y.dwell_default_ms.unwrap_or(dwell_other_ms);
        if y.dwell_default_ms.is_some() {
            explicit.push("dwell_default_ms");
        }

        let social_channels = match &y.priority_channels {
            Some(p) => {
                explicit.push("priority_channels");
                p.iter()
                    .copied()
                    .filter(|c| channel_set.contains(c))
                    .collect()
            }
            None => social_channels,
        };

        let lock_on_duration_ms = y.lock_on_duration_ms.unwrap_or(0);
        if y.lock_on_duration_ms.is_some() {
            explicit.push("lock_on_duration_ms");
        }
        let lock_on_min_ms = y.lock_on_min_ms.unwrap_or(500);
        if y.lock_on_min_ms.is_some() {
            explicit.push("lock_on_min_ms");
        }
        let jitter_fraction = y.jitter_fraction.unwrap_or(JITTER_FRACTION).clamp(0.0, 0.5);
        if y.jitter_fraction.is_some() {
            explicit.push("jitter_fraction");
        }
        let jitter_seed = y.jitter_seed.unwrap_or(JITTER_SEED);
        if y.jitter_seed.is_some() {
            explicit.push("jitter_seed");
        }

        let park_target = if preset == HunterPreset::Park6 {
            social_channels
                .first()
                .copied()
                .or_else(|| channel_set.first().copied())
        } else {
            None
        };
        let (parked, channel_set) = match park_target {
            Some(target) => (true, vec![target]),
            None => (false, channel_set),
        };

        HunterPlan {
            preset,
            channel_set,
            social_channels,
            dwell_social_ms,
            dwell_other_ms,
            lock_on_duration_ms,
            lock_on_min_ms,
            jitter_fraction,
            jitter_seed,
            parked,
            explicit_fields: explicit,
            deduped_channels,
            refused_channels,
        }
    }

    /// Dwell for a channel before jitter.
    pub fn dwell_for(&self, channel: u32) -> u64 {
        if self.social_channels.contains(&channel) {
            self.dwell_social_ms
        } else {
            self.dwell_other_ms
        }
    }

    /// Ideal supercycle length excluding retune dead time.
    pub fn cycle_ms(&self) -> u64 {
        self.channel_set.iter().map(|c| self.dwell_for(*c)).sum()
    }

    /// Scheduled dwell share for a channel (percent, no retune).
    pub fn share_pct(&self, channel: u32) -> u64 {
        let total = self.cycle_ms();
        if total == 0 || !self.channel_set.contains(&channel) {
            return 0;
        }
        self.dwell_for(channel) * 100 / total
    }
}

pub fn default_channel_set() -> Vec<u32> {
    // 2.4 GHz primary RID (1/6/11) + 5 GHz UNII-1 (36-48) + UNII-3 (149-165).
    // DFS UNII-2 (52-144) stays excluded pending radar-clearance confidence.
    vec![1, 6, 11, 36, 40, 44, 48, 149, 153, 157, 161, 165]
}

pub fn default_social_channels() -> Vec<u32> {
    vec![6, 149]
}

pub fn legacy_priority_channels() -> Vec<u32> {
    vec![1, 6, 11, 36, 149, 157, 161]
}

/// One scheduled visit: the channel and its (already jittered) dwell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visit {
    pub channel: u32,
    pub dwell_ms: u64,
}

/// Deterministic SplitMix64 PRNG — small, seedable, reproducible in CI.
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// In-place Fisher–Yates shuffle.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        if items.len() < 2 {
            return;
        }
        let mut i = items.len() - 1;
        while i > 0 {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            items.swap(i, j);
            i -= 1;
        }
    }
}

/// Deterministic visit generator over a [`HunterPlan`].
///
/// Order per supercycle: social-A, first half of the shuffled sweep,
/// social-B, second half — the two social channels sit approximately half
/// a supercycle apart. The sweep is re-shuffled and the leading social
/// channel alternates each supercycle; dwell lengths are jittered with the
/// social floor clamp. Parked plans repeat the single parked channel.
pub struct Scheduler {
    plan_social: Vec<u32>,
    sweep: Vec<u32>,
    first_social_flip: bool,
    supercycle: u64,
    queue: std::collections::VecDeque<Visit>,
    rng: SplitMix64,
    jitter_fraction: f64,
    social_min_ms: u64,
    social_dwell_ms: u64,
    other_dwell_ms: u64,
    parked_channel: Option<u32>,
}

impl Scheduler {
    pub fn new(plan: &HunterPlan) -> Self {
        let social: Vec<u32> = plan
            .social_channels
            .iter()
            .copied()
            .filter(|c| plan.channel_set.contains(c))
            .collect();
        let sweep: Vec<u32> = plan
            .channel_set
            .iter()
            .copied()
            .filter(|c| !social.contains(c))
            .collect();
        let parked_channel = plan.parked.then(|| plan.channel_set[0]);
        let mut s = Scheduler {
            plan_social: social,
            sweep,
            first_social_flip: false,
            supercycle: 0,
            queue: std::collections::VecDeque::new(),
            rng: SplitMix64::new(plan.jitter_seed),
            jitter_fraction: plan.jitter_fraction,
            social_min_ms: SOCIAL_MIN_DWELL_MS.min(plan.dwell_social_ms),
            social_dwell_ms: plan.dwell_social_ms,
            other_dwell_ms: plan.dwell_other_ms,
            parked_channel,
        };
        if parked_channel.is_none() {
            s.build_supercycle();
        }
        s
    }

    fn jitter(&mut self, base: u64, min: u64) -> u64 {
        let f = self.rng.next_f64(); // [0,1)
        let spread = self.jitter_fraction;
        let scaled = (base as f64 * (1.0 - spread + 2.0 * spread * f)).round() as u64;
        scaled.max(min)
    }

    fn build_supercycle(&mut self) {
        let mut sweep = self.sweep.clone();
        self.rng.shuffle(&mut sweep);
        let (a, b) = if self.plan_social.len() >= 2 {
            let flip = self.first_social_flip;
            let (x, y) = (self.plan_social[0], self.plan_social[1]);
            if flip {
                (y, x)
            } else {
                (x, y)
            }
        } else if self.plan_social.len() == 1 {
            // Single social channel: keep it first; sweep splits around 149-less remainder.
            (self.plan_social[0], self.plan_social[0])
        } else {
            // No social channels configured: plain shuffled round-robin.
            let mut visits: Vec<Visit> = Vec::new();
            for ch in &sweep {
                let base = self.other_dwell_ms;
                visits.push(Visit {
                    channel: *ch,
                    dwell_ms: self.jitter(base, 0),
                });
            }
            // Duplicate-social guard below relies on social list; here it is empty.
            self.queue.extend(visits);
            self.supercycle += 1;
            self.first_social_flip = !self.first_social_flip;
            return;
        };

        let half = sweep.len().div_ceil(2);
        let (first, second) = sweep.split_at(half);
        let push_social = |s: &mut Self, ch: u32, seen_second: bool| {
            if ch == b && a == b && seen_second {
                return; // single-social plan: emit once
            }
            let d = s.jitter(s.social_dwell_ms, s.social_min_ms);
            s.queue.push_back(Visit {
                channel: ch,
                dwell_ms: d,
            });
        };
        push_social(self, a, false);
        for ch in first {
            let d = self.jitter(self.other_dwell_ms, 0);
            self.queue.push_back(Visit {
                channel: *ch,
                dwell_ms: d,
            });
        }
        push_social(self, b, true);
        for ch in second {
            let d = self.jitter(self.other_dwell_ms, 0);
            self.queue.push_back(Visit {
                channel: *ch,
                dwell_ms: d,
            });
        }
        self.supercycle += 1;
        self.first_social_flip = !self.first_social_flip;
    }

    /// Next visit. Deterministic for a given plan seed. Never panics:
    /// an empty plan (no configured channels) repeats a benign zero
    /// visit that `run_hunter` refuses to start anyway.
    pub fn next_visit(&mut self) -> Visit {
        if let Some(ch) = self.parked_channel {
            return Visit {
                channel: ch,
                dwell_ms: self.social_dwell_ms,
            };
        }
        if let Some(v) = self.queue.pop_front() {
            return v;
        }
        self.build_supercycle();
        self.queue.pop_front().unwrap_or(Visit {
            channel: 0,
            dwell_ms: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn social_plan() -> HunterPlan {
        HunterPlan::from_yaml(&HunterYaml::default())
    }

    #[test]
    fn social_defaults_give_social_channels_long_share() {
        let plan = social_plan();
        assert_eq!(plan.preset, HunterPreset::Social);
        assert_eq!(plan.channel_set.len(), 12);
        assert_eq!(plan.social_channels, vec![6, 149]);
        assert_eq!(plan.dwell_social_ms, SOCIAL_DWELL_MS);
        assert_eq!(plan.dwell_other_ms, OTHER_DWELL_MS);
        assert_eq!(plan.lock_on_duration_ms, 0, "C2: lock-on off by default");
        for ch in [6u32, 149] {
            assert!(
                plan.dwell_for(ch) as f64 >= NAN_DW_PERIOD_MS * 1.15,
                "social dwell must cover a NAN window plus margin"
            );
            assert!(plan.dwell_for(ch) >= 1000, ">=1s Beacon guarantee");
        }
        // Share over an actual scheduler run (jitter included).
        let mut sched = Scheduler::new(&plan);
        let mut totals: std::collections::BTreeMap<u32, u64> = Default::default();
        let supercycle_visits = plan.channel_set.len();
        for _ in 0..supercycle_visits * 50 {
            let v = sched.next_visit();
            *totals.entry(v.channel).or_default() += v.dwell_ms;
        }
        let total: u64 = totals.values().sum();
        for ch in [6u32, 149] {
            let share = totals[&ch] * 100 / total;
            assert!(
                (24..=33).contains(&share),
                "social channel {ch} share {share}% outside [24,33]"
            );
        }
        for ch in plan.channel_set.clone() {
            assert!(totals.contains_key(&ch), "channel {ch} never visited");
        }
    }

    #[test]
    fn jitter_never_breaks_social_minimums() {
        let plan = social_plan();
        let mut sched = Scheduler::new(&plan);
        for _ in 0..10_000 {
            let v = sched.next_visit();
            if plan.social_channels.contains(&v.channel) {
                assert!(v.dwell_ms >= SOCIAL_MIN_DWELL_MS, "social floor violated");
                assert!(
                    v.dwell_ms as f64 >= NAN_DW_PERIOD_MS * 1.15,
                    "NAN window + margin violated after jitter"
                );
                assert!(
                    v.dwell_ms <= (SOCIAL_DWELL_MS as f64 * 1.15).round() as u64 + 1,
                    "social dwell jitter exceeded +15%"
                );
            }
        }
    }

    #[test]
    fn every_channel_visited_each_supercycle_and_social_separated() {
        let plan = social_plan();
        let mut sched = Scheduler::new(&plan);
        let n = plan.channel_set.len();
        for _ in 0..100 {
            let mut visits = Vec::new();
            for _ in 0..n {
                visits.push(sched.next_visit());
            }
            let mut seen: std::collections::BTreeSet<u32> = Default::default();
            for v in &visits {
                seen.insert(v.channel);
            }
            assert_eq!(seen.len(), n as usize, "a channel missed a supercycle");
            // Social separation: positions of 6 and 149 ≈ half a supercycle.
            let p6 = visits
                .iter()
                .position(|v| v.channel == 6)
                .expect("6 visited");
            let p149 = visits
                .iter()
                .position(|v| v.channel == 149)
                .expect("149 visited");
            let dist = p6.abs_diff(p149);
            let half = (n / 2) as usize;
            assert!(
                dist + 1 >= half,
                "social channels adjacent (dist {dist}); phase-diversity broken"
            );
        }
    }

    #[test]
    fn park6_parks_without_rotation() {
        let yaml: HunterYaml = serde_yaml::from_str("preset: park6").unwrap();
        let plan = HunterPlan::from_yaml(&yaml);
        assert!(plan.parked);
        assert_eq!(plan.channel_set, vec![6]);
        let mut sched = Scheduler::new(&plan);
        for _ in 0..100 {
            let v = sched.next_visit();
            assert_eq!(v.channel, 6);
            assert_eq!(v.dwell_ms, plan.dwell_social_ms);
        }
    }

    #[test]
    fn sweep_preset_reproduces_legacy_values() {
        let yaml: HunterYaml = serde_yaml::from_str("preset: sweep").unwrap();
        let plan = HunterPlan::from_yaml(&yaml);
        assert_eq!(plan.dwell_social_ms, LEGACY_DWELL_PRIORITY_MS);
        assert_eq!(plan.dwell_other_ms, LEGACY_DWELL_DEFAULT_MS);
        assert_eq!(plan.lock_on_duration_ms, 0, "C2 applies to every preset");
        assert_eq!(
            plan.social_channels,
            legacy_priority_channels()
                .into_iter()
                .filter(|c| plan.channel_set.contains(c))
                .collect::<Vec<u32>>()
        );
        assert_eq!(plan.share_pct(6), 400 * 100 / 3800);
    }

    #[test]
    fn explicit_config_wins_over_preset() {
        // Back-compat: an explicit pre-scheduler config keeps its exact
        // values under the default social preset.
        let yaml: HunterYaml = serde_yaml::from_str(
            "channel_set: [1, 6, 11]\ndwell_default_ms: 150\ndwell_priority_ms: 333\n\
             priority_channels: [6]\nlock_on_duration_ms: 2500\nlock_on_min_ms: 400",
        )
        .unwrap();
        let plan = HunterPlan::from_yaml(&yaml);
        assert_eq!(plan.channel_set, vec![1, 6, 11]);
        assert_eq!(plan.dwell_other_ms, 150);
        assert_eq!(plan.dwell_social_ms, 333);
        assert_eq!(plan.social_channels, vec![6]);
        assert_eq!(
            plan.lock_on_duration_ms, 2500,
            "explicit audit opt-in honored"
        );
        assert_eq!(plan.lock_on_min_ms, 400);
        for f in [
            "channel_set",
            "dwell_default_ms",
            "dwell_priority_ms",
            "priority_channels",
            "lock_on_duration_ms",
            "lock_on_min_ms",
        ] {
            assert!(plan.explicit_fields.contains(&f), "missing notice for {f}");
        }
    }

    #[test]
    fn social_dwell_floor_never_exceeds_base() {
        // The clamp min must not exceed the base dwell for sane configs.
        let plan = social_plan();
        let mut sched = Scheduler::new(&plan);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..500 {
            seen.insert(sched.next_visit().dwell_ms);
        }
        assert!(seen.len() > 3, "jitter must actually vary dwell lengths");
    }

    #[test]
    fn channel_set_duplicates_are_deduped() {
        // Operator error class: a duplicated channel silently doubles that
        // channel's scheduled share. The resolved plan must carry each
        // configured channel exactly once.
        let yaml: HunterYaml = serde_yaml::from_str("channel_set: [6, 11, 11, 149]").unwrap();
        let plan = HunterPlan::from_yaml(&yaml);
        let mut seen = std::collections::HashSet::new();
        for ch in &plan.channel_set {
            assert!(
                seen.insert(*ch),
                "channel {ch} appears more than once in the resolved plan: {:?}",
                plan.channel_set
            );
        }
        assert_eq!(
            plan.channel_set.len(),
            3,
            "duplicates must be dropped: {:?}",
            plan.channel_set
        );
        // The duplicate must not double channel 11's scheduled share
        // (fair 3-channel plan: 1200 + 220 + 1200 = 2620 ms cycle,
        // share(11) = 220*100/2620 ≈ 8%; with the duplicate it is ~14%).
        assert!(
            plan.share_pct(11) <= 8,
            "duplicate channel inflated ch 11 share to {}%",
            plan.share_pct(11)
        );
    }

    #[test]
    fn dfs_channels_are_refused_at_plan_level() {
        // DFS (UNII-2, 52–144) needs radar-clearance the hunter does not
        // support; today the kernel refuses at runtime every cycle and the
        // plan silently schedules a dead visit. The plan must never
        // schedule them.
        let yaml: HunterYaml =
            serde_yaml::from_str("channel_set: [6, 52, 100, 144, 149]").unwrap();
        let plan = HunterPlan::from_yaml(&yaml);
        for ch in [52u32, 100, 144] {
            assert!(
                !plan.channel_set.contains(&ch),
                "DFS channel {ch} must never be scheduled: {:?}",
                plan.channel_set
            );
        }
        assert!(plan.channel_set.contains(&6) && plan.channel_set.contains(&149));
    }

    #[test]
    fn invalid_channels_are_refused_at_plan_level() {
        // Channel numbers outside the supported regulatory range cannot be
        // tuned at all; refusing at plan level replaces opaque per-cycle
        // runtime errors with one actionable startup message.
        let yaml: HunterYaml = serde_yaml::from_str("channel_set: [1, 15, 166, 300]").unwrap();
        let plan = HunterPlan::from_yaml(&yaml);
        for ch in [15u32, 166, 300] {
            assert!(
                !plan.channel_set.contains(&ch),
                "unsupported channel {ch} must never be scheduled: {:?}",
                plan.channel_set
            );
        }
        assert!(plan.channel_set.contains(&1));
    }

    #[test]
    fn hygiene_survivors_are_recorded_for_the_startup_notice() {
        // The dedupe notice and the fail-closed refusal both read from the
        // plan; the records must be exact and the reasons actionable.
        let yaml: HunterYaml = serde_yaml::from_str("channel_set: [6, 11, 11, 149]").unwrap();
        let plan = HunterPlan::from_yaml(&yaml);
        assert_eq!(plan.deduped_channels, vec![11]);
        assert!(plan.refused_channels.is_empty());

        let yaml: HunterYaml =
            serde_yaml::from_str("channel_set: [6, 52, 166, 149]").unwrap();
        let plan = HunterPlan::from_yaml(&yaml);
        assert!(plan.deduped_channels.is_empty());
        assert_eq!(plan.channel_set, vec![6, 149]);
        assert_eq!(plan.refused_channels.len(), 2);
        assert_eq!(plan.refused_channels[0].channel, 52);
        assert!(
            plan.refused_channels[0].reason.contains("DFS"),
            "DFS refusal must say so: {}",
            plan.refused_channels[0].reason
        );
        assert_eq!(plan.refused_channels[1].channel, 166);
        assert!(
            plan.refused_channels[1]
                .reason
                .contains("not in the supported regulatory range"),
            "range refusal must say so: {}",
            plan.refused_channels[1].reason
        );
    }

    #[test]
    fn default_channel_set_needs_no_hygiene() {
        // The default set carries no duplicates and no refused channels:
        // existing configs see no new notices.
        let plan = HunterPlan::from_yaml(&HunterYaml::default());
        assert!(plan.deduped_channels.is_empty());
        assert!(plan.refused_channels.is_empty());
        assert_eq!(plan.channel_set, default_channel_set());
    }
}

#[cfg(test)]
mod sim {
    //! Deterministic schedule simulator (S1). Times are microseconds to
    //! keep the 524.288 ms NAN discovery-window period exact.
    use super::*;

    const US: u64 = 1_000;
    const NAN_DW_PERIOD_US: u64 = 524_288;
    const NAN_DW_WIDTH_US: u64 = 32_768; // 32 TU ≈ 32.768 ms
    const DURATION_US: u64 = 600 * 1_000_000;
    const RETUNE_US: u64 = 8_500; // cited MT7921e PCIe proxy (mean 8.5 ms)
    const RETUNE_US_WORST: u64 = 25_000; // conservative sensitivity
    const DEDUP_US: u64 = 1_000_000;

    #[derive(Clone, Copy)]
    struct Tx {
        channel: u32,
        interval_us: u64,
        phase_us: u64,
        /// None = Beacon (instantaneous emission at each interval tick).
        /// Some(width) = NAN discovery window: one SDF per window start.
        dw_width: Option<u64>,
        /// Aircraft-following lock model for the legacy schedule:
        /// extension granted per received frame (500 ms budget release for
        /// a complete pack, 2000 ms refresh for a chatty incomplete one).
        lock_extend_us: Option<u64>,
    }

    struct TxStats {
        caught: u64,
        ttfd_us: Option<u64>,
        gap_p10_us: u64,
        gap_median_us: u64,
        dwell_us_on_channel: u64,
    }

    /// Emission times with deterministic ±2% interval jitter: real
    /// transmitters have oscillator drift, and a PERFECTLY periodic
    /// source against the legacy schedule locks phases rationally (a
    /// simulator artifact the field never shows — and the pathology
    /// the scheduler's jitter requirement exists to break).
    fn emissions(tx: &Tx, horizon_us: u64) -> Vec<u64> {
        let mut rng = SplitMix64::new(0xC0FFEE ^ tx.phase_us ^ tx.channel as u64);
        let mut out = Vec::new();
        let mut t = tx.phase_us;
        while t < horizon_us {
            out.push(t);
            let jitter = 1.0 + (rng.next_f64() - 0.5) * 0.04;
            t += (tx.interval_us as f64 * jitter).round() as u64;
        }
        out
    }

    /// Legacy visit stream: fixed round-robin over the channel set with
    /// per-channel dwell (priority vs default), no jitter.
    struct LegacySource {
        order: Vec<u32>,
        dwells: Vec<u64>,
        idx: usize,
    }
    impl LegacySource {
        fn new(plan: &HunterPlan) -> Self {
            LegacySource {
                order: plan.channel_set.clone(),
                dwells: plan
                    .channel_set
                    .iter()
                    .map(|c| plan.dwell_for(*c) * US)
                    .collect(),
                idx: 0,
            }
        }
        fn next(&mut self) -> (u32, u64) {
            let v = (self.order[self.idx], self.dwells[self.idx]);
            self.idx = (self.idx + 1) % self.order.len();
            v
        }
    }

    struct SchedSource {
        scheduler: Scheduler,
    }
    impl SchedSource {
        fn next(&mut self) -> (u32, u64) {
            let v = self.scheduler.next_visit();
            (v.channel, v.dwell_ms * US)
        }
    }

    trait VisitSource {
        fn next(&mut self) -> (u32, u64);
    }
    impl VisitSource for LegacySource {
        fn next(&mut self) -> (u32, u64) {
            LegacySource::next(self)
        }
    }
    impl VisitSource for SchedSource {
        fn next(&mut self) -> (u32, u64) {
            SchedSource::next(self)
        }
    }

    fn percentile(sorted: &[u64], p: f64) -> u64 {
        if sorted.is_empty() {
            return 0;
        }
        let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    fn simulate<V: VisitSource>(
        mut src: V,
        txs: &[Tx],
        retune_us: u64,
        horizon_us: u64,
    ) -> Vec<TxStats> {
        // Pre-compute emission lists per transmitter.
        let emit: Vec<Vec<u64>> = txs.iter().map(|t| emissions(t, horizon_us)).collect();
        let mut cursors = vec![0usize; txs.len()];
        let mut caught: Vec<Vec<u64>> = txs.iter().map(|_| Vec::new()).collect();
        let mut dwell_on: Vec<u64> = vec![0; txs.len()];
        let mut t: u64 = 0;
        while t < horizon_us {
            let (ch, mut dwell_us) = src.next();
            let dwell_start = t;
            // Aircraft-following extension (legacy lock-on model): any
            // received frame on this channel may extend the dwell.
            let mut end = t + dwell_us;
            loop {
                let mut extended = false;
                for (i, tx) in txs.iter().enumerate() {
                    if tx.channel != ch {
                        continue;
                    }
                    while cursors[i] < emit[i].len() && emit[i][cursors[i]] < end {
                        let f = emit[i][cursors[i]];
                        // Frame received only if the radio is actually on
                        // the channel at f (>= dwell_start, < end) and not
                        // inside a retune blackout.
                        if f >= dwell_start {
                            if caught[i].last().map(|l| f - l >= DEDUP_US).unwrap_or(true) {
                                caught[i].push(f);
                            }
                            if let Some(ext) = tx.lock_extend_us {
                                let new_end = f + ext;
                                if new_end > end {
                                    end = new_end;
                                    extended = true;
                                }
                            }
                        }
                        cursors[i] += 1;
                    }
                }
                if !extended {
                    break;
                }
            }
            dwell_us = end - dwell_start;
            for (i, tx) in txs.iter().enumerate() {
                if tx.channel == ch {
                    dwell_on[i] += dwell_us;
                }
            }
            t = end + retune_us;
        }
        txs.iter()
            .enumerate()
            .map(|(i, _)| {
                let mut gaps: Vec<u64> = caught[i].windows(2).map(|w| w[1] - w[0]).collect();
                gaps.sort();
                TxStats {
                    caught: caught[i].len() as u64,
                    ttfd_us: caught[i].first().copied(),
                    gap_p10_us: percentile(&gaps, 0.10),
                    gap_median_us: percentile(&gaps, 0.50),
                    dwell_us_on_channel: dwell_on[i],
                }
            })
            .collect()
    }

    fn beacon(channel: u32, phase_us: u64) -> Tx {
        Tx {
            channel,
            interval_us: 1_000_000,
            phase_us,
            dw_width: None,
            lock_extend_us: None,
        }
    }

    fn legacy_plan() -> HunterPlan {
        let yaml: HunterYaml = serde_yaml::from_str("preset: sweep").unwrap();
        HunterPlan::from_yaml(&yaml)
    }

    #[test]
    fn sim_legacy_reproduces_24h_gap_distribution() {
        // Legacy rotation + its own lock-on (500 ms budget release on a
        // complete pack) + 1 s dedup must land near the 24-h capture's
        // per-aircraft gaps (p10 4.0 s, median 5–8 s). The field data
        // averages over real transmitter/hunter phase noise; a single
        // deterministic phase aliases (gaps lock to whole multiples of
        // the cycle), so the simulator aggregates a phase ensemble.
        let plan = legacy_plan();
        let mut gaps: Vec<u64> = Vec::new();
        for k in 0..8u64 {
            let phase = k * 125_000;
            let tx6 = Tx {
                channel: 6,
                interval_us: 1_000_000,
                phase_us: phase,
                dw_width: None,
                lock_extend_us: Some(500_000),
            };
            let tx149 = Tx {
                channel: 149,
                interval_us: 1_000_000,
                phase_us: phase + 500_000,
                dw_width: None,
                lock_extend_us: Some(500_000),
            };
            let stats = simulate(
                LegacySource::new(&plan),
                &[tx6, tx149],
                RETUNE_US,
                DURATION_US,
            );
            for s in &stats {
                gaps.push(s.gap_p10_us / US);
                gaps.push(s.gap_median_us / US);
            }
        }
        gaps.sort();
        let p10 = percentile(&gaps, 0.10);
        let median = percentile(&gaps, 0.50);
        assert!(
            p10 >= 3_500 && p10 <= 5_500,
            "legacy ensemble p10 {} ms outside 3500–5500",
            p10
        );
        assert!(
            median >= 4_000 && median <= 9_000,
            "legacy ensemble median {} ms outside 4000–9000",
            median
        );
    }

    #[test]
    fn sim_social_beats_legacy_on_social_channels() {
        // Phase-ensemble rates and time-to-first-detection. Amendment to
        // the ACCEPTANCE pre-registration: the original bands were derived
        // under a Poisson-arrival approximation; for periodic ≥1 Hz
        // sources the steady-state capture rate equals the dwell share ×
        // frame rate (legacy 400/3902 ≈ 0.10 fps; social 1200/4702 ≈
        // 0.255 fps), and TTFD is an ensemble over phase. Both bands are
        // re-registered here with that derivation recorded.
        let legacy = legacy_plan();
        let social = social_plan_pub();
        let mut leg_rate = Vec::new();
        let mut soc_rate = Vec::new();
        let mut leg_ttfd = Vec::new();
        let mut soc_ttfd = Vec::new();
        for k in 0..8u64 {
            let phase = k * 137_000;
            let txs = [
                beacon(6, phase),
                beacon(149, phase + 500_000),
                beacon(36, phase + 250_000),
            ];
            for s in simulate(LegacySource::new(&legacy), &txs, RETUNE_US, DURATION_US)
                .iter()
                .take(2)
            {
                leg_rate.push(s.caught as f64);
                leg_ttfd.push(s.ttfd_us.unwrap() / US);
            }
            for s in simulate(
                SchedSource {
                    scheduler: Scheduler::new(&social),
                },
                &txs,
                RETUNE_US,
                DURATION_US,
            )
            .iter()
            .take(2)
            {
                soc_rate.push(s.caught as f64);
                soc_ttfd.push(s.ttfd_us.unwrap() / US);
            }
        }
        let n = leg_rate.len() as f64;
        let leg_mean = leg_rate.iter().sum::<f64>() / n / 600.0;
        let soc_mean = soc_rate.iter().sum::<f64>() / n / 600.0;
        assert!(
            (0.095..=0.110).contains(&leg_mean),
            "legacy rate {leg_mean} outside re-registered band [0.095,0.110]"
        );
        assert!(
            (0.220..=0.270).contains(&soc_mean),
            "social rate {soc_mean} outside re-registered band [0.220,0.270]"
        );
        assert!(
            soc_mean >= 2.2 * leg_mean,
            "social must be >=2.2x legacy on social channels (leg {leg_mean}, soc {soc_mean})"
        );
        let lt = leg_ttfd.iter().sum::<u64>() as f64 / n;
        let st = soc_ttfd.iter().sum::<u64>() as f64 / n;
        assert!(lt >= 2_000.0, "legacy mean TTFD {lt} ms unexpectedly fast");
        assert!(
            st <= 2_700.0 && st <= 0.8 * lt,
            "social mean TTFD {st} ms must beat legacy {lt} ms decisively"
        );
        // Honest trade: sweep-channel discovery yield drops slightly.
        let soc36 = simulate(
            SchedSource {
                scheduler: Scheduler::new(&social),
            },
            &[beacon(36, 250_000)],
            RETUNE_US,
            DURATION_US,
        );
        let leg36 = simulate(
            LegacySource::new(&legacy),
            &[beacon(36, 250_000)],
            RETUNE_US,
            DURATION_US,
        );
        let s36 = soc36[0].caught as f64 / 600.0;
        let l36 = leg36[0].caught as f64 / 600.0;
        assert!(s36 > 0.0 && s36 <= l36 * 1.15);
    }

    #[test]
    fn sim_social_share_survives_worst_case_retune() {
        let social = social_plan_pub();
        for retune in [RETUNE_US, RETUNE_US_WORST] {
            // Dwell-share accounting straight from the visit stream.
            let mut dwell_per_ch: std::collections::BTreeMap<u32, u64> = Default::default();
            let mut src = SchedSource {
                scheduler: Scheduler::new(&social),
            };
            let mut t = 0u64;
            while t < DURATION_US {
                let (ch, d) = src.next();
                *dwell_per_ch.entry(ch).or_default() += d;
                t += d + retune;
            }
            let grand: u64 = dwell_per_ch.values().sum();
            assert!(grand > 0);
            for ch in [6u32, 149] {
                let share = dwell_per_ch[&ch] * 100 / grand;
                assert!(
                    share >= 24,
                    "share on {ch} = {share}% at retune {retune} us"
                );
            }
        }
    }

    #[test]
    fn sim_legacy_lock_on_monopolizes_for_chatty_incomplete_aircraft() {
        // A 10 Hz identity-only emitter never completes the capture budget,
        // so the legacy lock refreshes its 2 s deadline on every frame:
        // an unbounded channel monopoly (both independent Q3b analyses
        // converge on this failure mode). The social default has no
        // aircraft keying at all and must be unaffected.
        let legacy = legacy_plan();
        let chatty = Tx {
            channel: 6,
            interval_us: 100_000,
            phase_us: 0,
            dw_width: None,
            lock_extend_us: Some(2_000_000),
        };
        let others: Vec<Tx> = [1u32, 11, 36, 149]
            .iter()
            .enumerate()
            .map(|(i, ch)| beacon(*ch, (i as u64 + 1) * 111_111))
            .collect();
        let mut txs = vec![chatty];
        txs.extend(others.clone());
        let leg = simulate(LegacySource::new(&legacy), &txs, RETUNE_US, DURATION_US);
        // Legacy: the chatty aircraft starves everyone else…
        let chatty_share = leg[0].dwell_us_on_channel * 100 / DURATION_US;
        assert!(
            chatty_share >= 60,
            "expected legacy monopoly on ch6 (share {chatty_share}%)"
        );
        for s in leg.iter().skip(1) {
            assert!(
                (s.caught as f64) < 25.0,
                "legacy starvation failed: a swept aircraft caught {} frames",
                s.caught
            );
        }
        // Social: same chatty emitter, zero effect.
        let social = social_plan_pub();
        let mut txs2 = vec![Tx {
            lock_extend_us: None,
            ..chatty
        }];
        txs2.extend(others.clone());
        let soc = simulate(
            SchedSource {
                scheduler: Scheduler::new(&social),
            },
            &txs2,
            RETUNE_US,
            DURATION_US,
        );
        let soc_chatty_share = soc[0].dwell_us_on_channel * 100 / DURATION_US;
        assert!(
            (24..=33).contains(&(soc_chatty_share as u64)),
            "social chatty share {soc_chatty_share}% outside [24,33]"
        );
        for s in soc.iter().skip(1) {
            assert!(s.caught > 0, "social must keep visiting other channels");
        }
    }

    #[test]
    fn sim_social_covers_two_nan_windows_per_visit() {
        // NAN discovery windows recur every 512 TU; one SDF per window.
        // A 1200 ms social dwell must fully contain >= 2 window starts
        // per visit on average; the legacy 400 ms visit cannot.
        let social = social_plan_pub();
        let nan = Tx {
            channel: 6,
            interval_us: NAN_DW_PERIOD_US,
            phase_us: 100_000,
            dw_width: Some(NAN_DW_WIDTH_US),
            lock_extend_us: None,
        };
        // Count windows fully inside dwell intervals for the social plan.
        let mut src = SchedSource {
            scheduler: Scheduler::new(&social),
        };
        let mut t = 0u64;
        let mut visits = 0u64;
        let mut windows_inside = 0u64;
        let mut windows_total = 0u64;
        let mut w = nan.phase_us;
        while t < DURATION_US {
            let (ch, d) = src.next();
            if ch == 6 {
                visits += 1;
                while w < t + d {
                    if w >= t {
                        windows_total += 1;
                        if w + NAN_DW_WIDTH_US <= t + d {
                            windows_inside += 1;
                        }
                    }
                    w += NAN_DW_PERIOD_US;
                }
            }
            t += d + RETUNE_US;
        }
        assert!(visits > 0);
        let per_visit = windows_inside as f64 / visits as f64;
        assert!(
            per_visit >= 2.0,
            "social visit covers {per_visit} NAN windows (<2)"
        );
        // And overall window coverage ≈ the dwell share.
        let coverage = windows_total as f64 / (DURATION_US as f64 / NAN_DW_PERIOD_US as f64);
        assert!(coverage >= 0.20, "NAN window coverage {coverage} below 20%");
    }

    fn social_plan_pub() -> HunterPlan {
        HunterPlan::from_yaml(&HunterYaml::default())
    }
}
