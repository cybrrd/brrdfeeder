// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! #183 — GPS clock discipline (no-RTC edge-node time correctness).
//!
//! ## Why this exists
//! The feeders are no-RTC ARM nodes (Pi 4 / CM4) — and the mobile one
//! (field-node, in the Tacoma) hard-power-cycles when the truck shuts off.
//! On cold-boot a no-RTC node comes up with a **stale wall clock** (observed:
//! 2026-06-13 instead of 06-17) until NTP corrects it — and NTP needs a working
//! cellular backhaul that may not exist yet in the field. Any telemetry frame
//! stamped from the system wall clock in that window carries a multi-day-stale
//! `timestamp_utc`, which the backend **Bohr Spool / Minkowski Trust Filter
//! correctly rejects as a causality violation** — SILENTLY. A whole flight's
//! origin frames can evaporate at the trust gate with no error at the edge.
//!
//! Wave 7.4 gated GPS *position* before publishing; it did NOT gate *time*.
//! Position and time are separate trust dimensions. This closes the second one.
//!
//! ## What it does (the Linux build)
//! The u-blox already broadcasts atomic-clock UTC (NMEA RMC date + time). On the
//! first fix carrying GPS UTC, if the system clock is off by more than
//! [`CLOCK_SKEW_THRESHOLD_MS`], we step `CLOCK_REALTIME` to GPS truth via
//! `clock_settime` (requires **CAP_SYS_TIME** on the rootful Quadlet). This pulls
//! the *whole OS timeline* — journald included — into reality, so a field
//! forensic timeline is never split-brained between a 2026 application layer and
//! a 1970 hypervisor. Once the clock is aligned (stepped, or already NTP-correct
//! within threshold) [`TimeTrust`] flips trusted, releasing the publish gate.
//!
//! ## Portability
//! Stepping `CLOCK_REALTIME` is a Linux syscall. On the future RTC-less ESP32
//! `#![no_std]` Sentinel there is no OS clock to set — that build uses the
//! application-level GPS-time-stamping + publish-gate path instead. [`TimeTrust`]
//! and the gate concept are shared; only the clock-step is Linux-specific.

use crate::node_config::ClockYaml;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Max system-vs-GPS skew (ms) we tolerate before stepping the clock. Within
/// this, the clock is "good enough" (NTP-synced, or already stepped) and we
/// trust it without touching it. WHY 5 s: comfortably inside the Bohr Spool
/// causal window while absorbing normal NTP jitter / fix latency. WHEN-to-tune:
/// lower if the Minkowski cone tightens; DEPENDS-ON the backend trust-filter
/// tolerance.
const CLOCK_SKEW_THRESHOLD_MS: i64 = 5_000;

/// Shared "is system time trustworthy yet?" flag. The GPS keeper sets it once
/// the clock is GPS-disciplined (or confirmed already-correct); the startup
/// publish gate reads it. Cheap to clone via `Arc`.
#[derive(Debug, Default)]
pub struct TimeTrust {
    trusted: AtomicBool,
    consensus: Mutex<Consensus>,
}

impl TimeTrust {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_trusted(&self) -> bool {
        self.trusted.load(Ordering::Relaxed)
    }

    fn mark_trusted(&self) {
        self.trusted.store(true, Ordering::Relaxed);
    }
}

fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Discipline the system clock from a GPS-UTC reading. Idempotent and cheap:
/// once `trust` is set, this is a no-op, so it's safe to call on every fix.
///
/// - skew ≤ threshold → clock already correct (NTP or a prior step) → mark trusted.
/// - skew  > threshold → step `CLOCK_REALTIME` to GPS UTC. On success, mark
///   trusted. On failure (most likely `EPERM` = missing CAP_SYS_TIME) we do
///   **not** mark trusted — the publish gate must keep waiting rather than ship
///   frames with a clock we couldn't fix.
pub fn discipline_from_gps(gps_utc_ms: i64, trust: &TimeTrust, policy: &ClockYaml) {
    if trust.is_trusted() {
        return;
    }
    let before = system_now_ms();
    let Ok(mut consensus) = trust.consensus.lock() else {
        return;
    };
    if !consensus.observe(gps_utc_ms, Instant::now(), policy.consistent_fixes) {
        return;
    }
    let skew = before.abs_diff(gps_utc_ms);
    if skew > policy.max_step_secs.saturating_mul(1000) && !policy.allow_large_step {
        eprintln!("[clock] refused GPS step: before_unix_ms={before} proposed_after_unix_ms={gps_utc_ms} jump_ms={skew} cap_secs={}; publish gate stays closed (explicit allow_large_step override required)", policy.max_step_secs);
        return;
    }
    if skew <= CLOCK_SKEW_THRESHOLD_MS as u64 {
        println!(
            "[clock] system clock within {} ms of GPS UTC (skew {} ms) — trusted, no step needed",
            CLOCK_SKEW_THRESHOLD_MS, skew
        );
        trust.mark_trusted();
        return;
    }
    match step_realtime(gps_utc_ms) {
        Ok(()) => {
            println!(
                "[clock] stepped CLOCK_REALTIME before_unix_ms={} after_unix_ms={} jump_ms={} override={} — publish gate released",
                before, gps_utc_ms, skew, policy.allow_large_step
            );
            trust.mark_trusted();
        }
        Err(e) => {
            eprintln!(
                "[clock] FAILED to step CLOCK_REALTIME to GPS UTC: {} — system clock still off by {} ms. \
                 Grant CAP_SYS_TIME to the container (Quadlet AddCapability=CAP_SYS_TIME), or restore NTP. \
                 NOT marking time trusted; publish gate stays closed.",
                e, skew
            );
        }
    }
}

#[derive(Debug, Default)]
struct Consensus {
    last: Option<(i64, Instant)>,
    count: u32,
}
impl Consensus {
    fn observe(&mut self, gps: i64, now: Instant, required: u32) -> bool {
        let agrees = self.last.is_some_and(|(previous, at)| {
            let elapsed = now.saturating_duration_since(at).as_millis() as i128;
            let advance = gps as i128 - previous as i128;
            advance > 0 && elapsed <= 10_000 && (advance - elapsed).abs() <= 1000
        });
        self.count = if agrees {
            self.count.saturating_add(1)
        } else {
            1
        };
        self.last = Some((gps, now));
        gps > 0 && self.count >= required
    }
}

/// Step the real-time clock to `gps_utc_ms` (Unix-ms). Linux-only.
#[cfg(target_os = "linux")]
fn step_realtime(gps_utc_ms: i64) -> Result<(), String> {
    let secs = gps_utc_ms.div_euclid(1000);
    let nanos = gps_utc_ms.rem_euclid(1000) * 1_000_000;
    let ts = libc::timespec {
        tv_sec: secs as libc::time_t,
        tv_nsec: nanos as libc::c_long,
    };
    // SAFETY: `ts` is a fully-initialized, valid `timespec`; `clock_settime`
    // only reads it. Returns 0 on success, -1 on error (errno set).
    let rc = unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

#[cfg(not(target_os = "linux"))]
fn step_realtime(_gps_utc_ms: i64) -> Result<(), String> {
    Err("clock stepping is implemented only on Linux".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_starts_false_and_marks_true() {
        let t = TimeTrust::new();
        assert!(!t.is_trusted());
        t.mark_trusted();
        assert!(t.is_trusted());
    }

    #[test]
    fn small_skew_marks_trusted_without_stepping() {
        // A GPS UTC equal to "now" is well within threshold → trusted, and
        // crucially this does NOT attempt a clock step (no CAP_SYS_TIME needed
        // in CI). Proves the NTP-already-correct fast path.
        let t = TimeTrust::new();
        let now = Instant::now();
        let gps = system_now_ms();
        *t.consensus.lock().unwrap() = Consensus {
            last: Some((gps - 1000, now - std::time::Duration::from_secs(1))),
            count: 2,
        };
        discipline_from_gps(gps, &t, &ClockYaml::default());
        assert!(
            t.is_trusted(),
            "in-threshold clock should be trusted without a step"
        );
    }

    #[test]
    fn idempotent_once_trusted() {
        let t = TimeTrust::new();
        t.mark_trusted();
        // A wildly-wrong GPS UTC must be ignored once already trusted (no step
        // attempted), so this is a safe no-op even without CAP_SYS_TIME.
        discipline_from_gps(0, &t, &ClockYaml::default());
        assert!(t.is_trusted());
    }

    #[test]
    fn consensus_rejects_replay_jump_and_long_gap() {
        let mut c = Consensus::default();
        let now = Instant::now();
        assert!(!c.observe(10000, now, 3));
        assert!(!c.observe(11000, now + std::time::Duration::from_secs(1), 3));
        assert!(c.observe(12000, now + std::time::Duration::from_secs(2), 3));
        assert!(!c.observe(12000, now + std::time::Duration::from_secs(3), 3));
        assert!(!c.observe(i64::MAX, now + std::time::Duration::from_secs(4), 3));
        assert!(!c.observe(i64::MIN, now + std::time::Duration::from_secs(5), 3));
        assert!(!c.observe(50000, now + std::time::Duration::from_secs(50), 3));
    }
    #[test]
    fn huge_consistent_step_stays_untrusted_without_syscall() {
        let t = TimeTrust::new();
        let gps = 1_000_000;
        *t.consensus.lock().unwrap() = Consensus {
            last: Some((
                gps - 1000,
                Instant::now() - std::time::Duration::from_secs(1),
            )),
            count: 2,
        };
        discipline_from_gps(gps, &t, &ClockYaml::default());
        assert!(!t.is_trusted());
    }
}
