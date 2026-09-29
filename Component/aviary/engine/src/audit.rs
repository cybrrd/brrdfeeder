// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Drop-event audit subject for `cybrrd.system.audit.frame.dropped.<node_id>`.
//!
//! Per Wave 6.0b consensus (rate-limited aggregate, not per-frame):
//!   - Capture and backhaul both bump a shared atomic counter when frames
//!     are lost (buffer full at try_send, or NATS publish failure).
//!   - A 1 Hz background task drains the counter; if non-zero, emits one
//!     audit event covering the just-elapsed second's losses.
//!   - During a full backhaul outage the audit event also can't ship —
//!     this is by design. Drop events are useful only for partial
//!     degradation; full outage is signalled by silence on the heartbeat
//!     subject (separate concern).

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Reason a frame was dropped. Carried in the audit event so ops dashboards
/// can distinguish "feeder is local-stressed" from "broker is unreachable".
///
/// **Wire-format names are LOCKED snake_case strings** — globe-frontend uses
/// these to map node-status-panel colors (e.g., `buffer_full` → pulsing
/// orange, `publish_failed` → pulsing red, etc., per upcoming UI spec).
/// Adding a new variant requires updating the front-end's color table at
/// the same time. Renaming a variant is a breaking wire change.
///
/// Canonical values:
///   - `"buffer_full"`     — capture overran its bounded mpsc buffer
///   - `"publish_failed"`  — async-nats publish returned an error
///   - `"dedup_overflow"`  — reserved for future broadcast-storm conditions
///   - `"rid_ble_unassociated_dropped"` — BLE location without a fresh same-MAC ID
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DropReason {
    #[serde(rename = "buffer_full")]
    BufferFull,
    #[serde(rename = "publish_failed")]
    PublishFailed,
    #[serde(rename = "dedup_overflow")]
    DedupOverflow,
    #[serde(rename = "rid_ble_unassociated_dropped")]
    RidBleUnassociatedDropped,
}

/// Aggregate audit payload — one event per second per non-zero reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropAuditEvent {
    pub node_id: String,
    pub window_start_utc: u64,
    pub window_end_utc: u64,
    pub dropped_count: u64,
    pub reason: DropReason,
}

/// Shared counters. Cloning the wrapper is cheap (Arc); the underlying
/// atomics are bumped from any thread / task without a lock.
#[derive(Debug, Clone, Default)]
pub struct DropCounters {
    pub buffer_full: Arc<AtomicU64>,
    pub publish_failed: Arc<AtomicU64>,
    pub dedup_overflow: Arc<AtomicU64>,
    pub rid_ble_unassociated_dropped: Arc<AtomicU64>,
}

impl DropCounters {
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically take and zero each counter; returns the readings.
    /// Called by the audit-emit loop once per second.
    pub fn drain(&self) -> (u64, u64, u64, u64) {
        (
            self.buffer_full.swap(0, Ordering::Relaxed),
            self.publish_failed.swap(0, Ordering::Relaxed),
            self.dedup_overflow.swap(0, Ordering::Relaxed),
            self.rid_ble_unassociated_dropped.swap(0, Ordering::Relaxed),
        )
    }
}

/// Wall-clock for audit window boundaries (Wave 6.4.1: ms resolution).
/// See engine/src/capture.rs::now_unix_ms for the substrate-truth
/// rationale on the s→ms unit change.
fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Background task: every 1s, drain counters and emit one audit event per
/// non-zero reason. Caller provides a publish closure abstracting the
/// NATS client (so unit tests can substitute an in-memory sink).
pub async fn run_audit_emitter<P, Fut>(
    counters: DropCounters,
    node_id: String,
    mut publish: P,
) where
    P: FnMut(String, Vec<u8>) -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    let subject = format!("cybrrd.system.audit.frame.dropped.{}", node_id);

    loop {
        let window_start = now_unix_ms();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let window_end = now_unix_ms();

        let (buffer_full, publish_failed, dedup_overflow, ble_unassociated) = counters.drain();

        for (count, reason) in [
            (buffer_full, DropReason::BufferFull),
            (publish_failed, DropReason::PublishFailed),
            (dedup_overflow, DropReason::DedupOverflow),
            (ble_unassociated, DropReason::RidBleUnassociatedDropped),
        ] {
            if count == 0 {
                continue;
            }
            let event = DropAuditEvent {
                node_id: node_id.clone(),
                window_start_utc: window_start,
                window_end_utc: window_end,
                dropped_count: count,
                reason,
            };
            if let Ok(bytes) = serde_json::to_vec(&event) {
                publish(subject.clone(), bytes).await;
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Wave 7.1 Inc 7 — Substrate-attention audit events
// (separate from the counter-based DropAuditEvent above; this is an
// incident-event channel for the Kittler Substrate Defense's
// substrate-attention layer to publish recovery + failure events
// with full provenance metadata.)
// ─────────────────────────────────────────────────────────────────────

/// Lineage tag locked to the engine binary revision. Carried verbatim
/// in every SubstrateAuditEvent so downstream Hardware-Reliability-Index
/// queries can trace observation lineage. Mirrors
/// `hunter::KITTLER_LINEAGE` (which heartbeat already uses); kept here
/// as a separate const so audit.rs doesn't reach into hunter.rs.
pub const KITTLER_LINEAGE: &str = "kittler-substrate-defense-v1";

/// Substrate-attention event types. Wire-format strings are
/// LOCKED snake_case — downstream consumers (globe-backend audit
/// indexer, future Bluejay ops dashboard) key off these names.
///
/// Canonical values:
///   - `"tier_a_recovery"` — Hunter attempted a soft self-heal after
///     a transient set_channel failure (Wave 7.1 Inc 3 behavior).
///     `success` field tells whether the retry stuck.
///   - `"survey_failure"` — nl80211::get_survey returned an Err from
///     the kernel (not the silent-empty rtw88_8812au substrate-truth;
///     that case shows as `coverage_class: logic_only` in heartbeat).
///   - `"capture_stall"` — Wave 7.1 Inc 8. The Capture Liveness
///     Watchdog observed no frames for longer than STALL_THRESHOLD.
///     `elapsed_ms` carries the observed silence duration;
///     `error_message` carries the structured forensic diagnostic
///     (ifindex change, monitor-mode loss, dmesg root-cause). This is
///     the event the 2026-05-13 Alfa-cable-bump blind spot would have
///     fired had it existed.
///   - `"capture_recovered"` — Wave 7.1 Inc 8. The watchdog re-
///     established the capture path after a stall. `success: true`,
///     `elapsed_ms` is wall-clock recovery latency, `error_message`
///     summarizes what was healed.
///   - `"capture_recovery_failed"` — Wave 7.1 Inc 8. The watchdog
///     attempted a heal but capture did not resume within
///     RECOVERY_TIMEOUT. `success: false` — radio_status goes `error`
///     and operator intervention is required.
///   - `"wedge_detected"` — reserved (Wave 7.5 supervisor will fire
///     this when channel-stuck heuristics trigger).
///   - `"noise_anomaly"` — reserved (Wave 7.6 fleet-wide
///     Hardware-Reliability-Index will detect noise-floor deltas).
///   - `"auto_power_cycle"` — reserved (Wave 7.5 supervisor; substrate
///     §6.3 patent-claim Tier-C recovery).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubstrateAuditEvent {
    pub node_id: String,
    pub timestamp_utc: u64,
    /// Always equals KITTLER_LINEAGE. The provenance tag rides in
    /// every substrate-attention artifact forever.
    pub lineage: String,
    /// Locked snake_case event type identifier; see above for canonical
    /// values.
    pub event_type: String,
    /// Wi-Fi channel relevant to this event (e.g., which channel the
    /// recovery was attempted on). Optional because not every event
    /// type has a single-channel context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<u32>,
    /// For recovery-class events: did the recovery action succeed?
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    /// Substrate-truth error message from the failed operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Wall-clock ms spent on the recovery attempt, for ops dashboards
    /// to graph recovery-action latency over time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

impl SubstrateAuditEvent {
    /// Wave 7.1 Inc 7 — substrate-attention event constructors are
    /// kept here as static helpers (not enum variants) to keep the
    /// wire-format struct stable while still giving the Hunter task
    /// strongly-typed construction.
    pub fn tier_a_recovery(
        node_id: String,
        channel: u32,
        success: bool,
        error_message: Option<String>,
        elapsed_ms: u64,
    ) -> Self {
        Self {
            node_id,
            timestamp_utc: now_unix_ms(),
            lineage: KITTLER_LINEAGE.to_string(),
            event_type: "tier_a_recovery".to_string(),
            channel: Some(channel),
            success: Some(success),
            error_message,
            elapsed_ms: Some(elapsed_ms),
        }
    }

    pub fn survey_failure(node_id: String, error_message: String) -> Self {
        Self {
            node_id,
            timestamp_utc: now_unix_ms(),
            lineage: KITTLER_LINEAGE.to_string(),
            event_type: "survey_failure".to_string(),
            channel: None,
            success: None,
            error_message: Some(error_message),
            elapsed_ms: None,
        }
    }

    /// Wave 7.1 Inc 8 — the Capture Liveness Watchdog detected a
    /// capture stall (no frames for longer than STALL_THRESHOLD).
    /// `silence_ms` is the observed dead-air duration; the structured
    /// forensic detail (ifindex change, monitor-mode loss, dmesg
    /// root-cause) is folded into `error_message` so the wire-format
    /// struct stays stable while the diagnostic stays rich.
    ///
    /// Substrate-truth: this is the event the engine could NOT emit on
    /// 2026-05-13 — it ran `radio_status: "up"` for ~18 hours while
    /// the Alfa was actually in managed mode, capture-dark. Inc 8
    /// closes that seam.
    pub fn capture_stall(
        node_id: String,
        silence_ms: u64,
        ifindex_changed: bool,
        monitor_mode_lost: bool,
        dmesg_cause: Option<String>,
    ) -> Self {
        let mut diag = format!(
            "capture stall: {}ms of dead air; ifindex_changed={}; monitor_mode_lost={}",
            silence_ms, ifindex_changed, monitor_mode_lost
        );
        if let Some(cause) = dmesg_cause {
            diag.push_str("; dmesg: ");
            diag.push_str(&cause);
        }
        Self {
            node_id,
            timestamp_utc: now_unix_ms(),
            lineage: KITTLER_LINEAGE.to_string(),
            event_type: "capture_stall".to_string(),
            channel: None,
            success: None,
            error_message: Some(diag),
            elapsed_ms: Some(silence_ms),
        }
    }

    /// Wave 7.1 Inc 8 — the watchdog re-established the capture path
    /// after a stall. `elapsed_ms` is wall-clock recovery latency
    /// (stall detection → first fresh packet); `error_message`
    /// summarizes what the heal actually touched.
    pub fn capture_recovered(
        node_id: String,
        elapsed_ms: u64,
        ifindex_changed: bool,
        monitor_mode_lost: bool,
    ) -> Self {
        Self {
            node_id,
            timestamp_utc: now_unix_ms(),
            lineage: KITTLER_LINEAGE.to_string(),
            event_type: "capture_recovered".to_string(),
            channel: None,
            success: Some(true),
            error_message: Some(format!(
                "capture recovered: re_resolved_ifindex={}; re_established_monitor_mode={}",
                ifindex_changed, monitor_mode_lost
            )),
            elapsed_ms: Some(elapsed_ms),
        }
    }

    /// Wave 7.1 Inc 8 — the watchdog attempted a heal but capture did
    /// not resume within RECOVERY_TIMEOUT. `success: false`. The radio
    /// status goes `error` and operator intervention is required —
    /// substrate-honest: the engine reports it cannot fix this itself.
    pub fn capture_recovery_failed(
        node_id: String,
        elapsed_ms: u64,
        dmesg_cause: Option<String>,
    ) -> Self {
        let mut diag =
            String::from("capture did not resume within recovery timeout — operator intervention required");
        if let Some(cause) = dmesg_cause {
            diag.push_str("; dmesg: ");
            diag.push_str(&cause);
        }
        Self {
            node_id,
            timestamp_utc: now_unix_ms(),
            lineage: KITTLER_LINEAGE.to_string(),
            event_type: "capture_recovery_failed".to_string(),
            channel: None,
            success: Some(false),
            error_message: Some(diag),
            elapsed_ms: Some(elapsed_ms),
        }
    }
}

/// Background task: drain the substrate-audit channel, publish each
/// event to `cybrrd.audit.substrate.<event_type>.<node_id>`. Caller
/// provides a publish closure that abstracts the NATS client (so
/// unit tests can substitute an in-memory sink).
///
/// Lifecycle: runs until the channel sender is dropped (engine
/// shutdown). Lossy under audit-storm conditions only if the
/// upstream sender uses try_send instead of send.
pub async fn run_substrate_audit_emitter<P, Fut>(
    mut rx: tokio::sync::mpsc::Receiver<SubstrateAuditEvent>,
    mut publish: P,
) where
    P: FnMut(String, Vec<u8>) -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    while let Some(event) = rx.recv().await {
        let subject = format!(
            "cybrrd.audit.substrate.{}.{}",
            event.event_type, event.node_id
        );
        if let Ok(bytes) = serde_json::to_vec(&event) {
            publish(subject, bytes).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_zeros_counters() {
        let c = DropCounters::new();
        c.buffer_full.fetch_add(7, Ordering::Relaxed);
        c.publish_failed.fetch_add(3, Ordering::Relaxed);
        c.rid_ble_unassociated_dropped.fetch_add(2, Ordering::Relaxed);
        let (b, p, d, ble) = c.drain();
        assert_eq!(b, 7);
        assert_eq!(p, 3);
        assert_eq!(d, 0);
        assert_eq!(ble, 2);
        assert_eq!(serde_json::to_string(&DropReason::RidBleUnassociatedDropped).unwrap(), "\"rid_ble_unassociated_dropped\"");
        // Second drain yields zeros.
        assert_eq!(c.drain(), (0, 0, 0, 0));
    }

    #[test]
    fn drop_audit_event_serializes_canonically() {
        let event = DropAuditEvent {
            node_id: "bf-test-001".into(),
            window_start_utc: 1714342400,
            window_end_utc: 1714342401,
            dropped_count: 42,
            reason: DropReason::BufferFull,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"node_id\":\"bf-test-001\""));
        assert!(json.contains("\"dropped_count\":42"));
        assert!(json.contains("\"reason\":\"buffer_full\""), "got: {}", json);
    }

    // ─── Wave 7.1 Inc 7 — substrate-attention event tests ───

    #[test]
    fn tier_a_recovery_success_serializes_with_lineage() {
        let event = SubstrateAuditEvent::tier_a_recovery(
            "brrdfeeder-saker-cardinal-001".to_string(),
            149,
            true,
            Some("EAGAIN: temporary nl80211 socket busy".to_string()),
            12,
        );
        let json = serde_json::to_string(&event).unwrap();
        for field in &[
            "\"node_id\":\"brrdfeeder-saker-cardinal-001\"",
            "\"lineage\":\"kittler-substrate-defense-v1\"",
            "\"event_type\":\"tier_a_recovery\"",
            "\"channel\":149",
            "\"success\":true",
            "\"elapsed_ms\":12",
        ] {
            assert!(json.contains(field), "missing {} in {}", field, json);
        }
    }

    #[test]
    fn tier_a_recovery_failure_carries_error_message() {
        let event = SubstrateAuditEvent::tier_a_recovery(
            "test".to_string(),
            36,
            false,
            Some("driver did not accept channel".to_string()),
            8,
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"success\":false"));
        assert!(json.contains("driver did not accept channel"));
    }

    #[test]
    fn survey_failure_event_omits_channel_and_success() {
        // Substrate-truth: get_survey isn't channel-scoped, so channel
        // should be absent. success doesn't apply (the survey wasn't
        // a recovery attempt).
        let event = SubstrateAuditEvent::survey_failure(
            "test".to_string(),
            "netlink socket connect: permission denied".to_string(),
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("\"channel\""), "survey_failure should omit channel: {}", json);
        assert!(!json.contains("\"success\""), "survey_failure should omit success: {}", json);
        assert!(json.contains("\"event_type\":\"survey_failure\""));
        assert!(json.contains("permission denied"));
        assert!(json.contains("\"lineage\":\"kittler-substrate-defense-v1\""));
    }

    // ─── Wave 7.1 Inc 8 — capture-liveness event tests ───

    #[test]
    fn capture_stall_folds_forensics_into_error_message() {
        let event = SubstrateAuditEvent::capture_stall(
            "brrdfeeder-saker-cardinal-001".to_string(),
            18_000,
            true,
            true,
            Some("rtw88_8812au: -71 EPROTO | usb 1-1: USB disconnect".to_string()),
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event_type\":\"capture_stall\""));
        assert!(json.contains("\"elapsed_ms\":18000"));
        assert!(json.contains("\"lineage\":\"kittler-substrate-defense-v1\""));
        assert!(json.contains("ifindex_changed=true"));
        assert!(json.contains("monitor_mode_lost=true"));
        assert!(json.contains("EPROTO"));
        // capture_stall is a detection, not a recovery outcome.
        assert!(!json.contains("\"success\""), "capture_stall should omit success: {}", json);
        assert!(!json.contains("\"channel\""), "capture_stall is not channel-scoped: {}", json);
    }

    #[test]
    fn capture_stall_omits_dmesg_when_unreadable() {
        // Substrate-honest: on a hardened kernel the engine can't read
        // dmesg (no cap_syslog). The event still carries the
        // substantive forensics; the dmesg clause is just absent.
        let event = SubstrateAuditEvent::capture_stall(
            "test".to_string(),
            15_000,
            false,
            true,
            None,
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("monitor_mode_lost=true"));
        assert!(!json.contains("dmesg:"), "no dmesg clause when cause is None: {}", json);
    }

    #[test]
    fn capture_recovered_marks_success_with_latency() {
        let event = SubstrateAuditEvent::capture_recovered(
            "test".to_string(),
            1_240,
            true,
            true,
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event_type\":\"capture_recovered\""));
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"elapsed_ms\":1240"));
        assert!(json.contains("re_resolved_ifindex=true"));
        assert!(json.contains("re_established_monitor_mode=true"));
    }

    #[test]
    fn capture_recovery_failed_marks_failure() {
        let event = SubstrateAuditEvent::capture_recovery_failed(
            "test".to_string(),
            30_000,
            Some("usb 1-1: device not accepting address 7, error -110".to_string()),
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event_type\":\"capture_recovery_failed\""));
        assert!(json.contains("\"success\":false"));
        assert!(json.contains("\"elapsed_ms\":30000"));
        assert!(json.contains("operator intervention required"));
        assert!(json.contains("error -110"));
    }

    #[test]
    fn substrate_audit_subject_naming_convention() {
        // Confirms the subject hierarchy is
        // cybrrd.audit.substrate.<event_type>.<node_id> — this is
        // the wire-format contract globe-backend's audit indexer
        // will key off of.
        let event = SubstrateAuditEvent::tier_a_recovery(
            "bf-001".to_string(),
            6,
            true,
            None,
            5,
        );
        let subject = format!(
            "cybrrd.audit.substrate.{}.{}",
            event.event_type, event.node_id
        );
        assert_eq!(subject, "cybrrd.audit.substrate.tier_a_recovery.bf-001");
    }
}
