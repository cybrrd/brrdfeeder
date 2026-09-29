// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Wave 6.5 Green Protocol — 1 Hz AirspaceState tick publisher.
//!
//! Pack-consensus 2026-06-04 (Cy + Gemini + Synth). The cardinal emits
//! one batched AirspaceState envelope per second on
//! `cybrrd.green.airspace.<node_id>`, carrying all drones observed in
//! the current tick window. Parallel-path to the existing per-frame
//! NATS publisher in `backhaul.rs` — both pipes stay alive during
//! cutover so rollback is always available.
//!
//! ## Substrate-truth design
//!
//! 1. **Best-effort beacons over reliable transport.** Each envelope is
//!    a fresh state snapshot. A consumer that misses one tick simply
//!    sees the next tick's fresh state; no per-tick acknowledgment or
//!    retransmission at the application layer. NATS provides transport
//!    reliability (TCP); we don't double up at the app layer.
//!
//! 2. **The metronome never goes silent.** Every 1000 ms a tick fires.
//!    If the cardinal observed no drones in that window, the envelope's
//!    `drones` array is empty. Empty IS substrate-truth — "I am alive,
//!    nothing in airspace this second." Distinct from a missing tick
//!    (network outage / engine crash); consumers detect those via
//!    `tick_seq` gaps.
//!
//! 3. **`MissedTickBehavior::Skip`** — under sustained load (capture
//!    burst, GC pause, lock contention) we never burst-catch-up. A
//!    skipped tick is gone; the next tick is fresh. Bounded
//!    application-layer jitter.
//!
//! 4. **Substrate-truth on `position_state` (Phase 1 scope).** Drones
//!    are emitted only when their last broadcast falls within
//!    `FRESH_TICK_WINDOW_MS` (1 s). D27 uses `unknown` when the received
//!    position is absent, otherwise `measured`; Phase 3 adds `projected` / `stale`
//!    via velocity-vector kinematic projection.
//!
//! 5. **Color-line at root for broker QoS routing.** Subject prefix is
//!    `cybrrd.green.airspace.` per pack-consensus 2026-06-04 — color-
//!    line at the first NATS subject token lets the broker ACL/QoS-
//!    route without payload inspection.
//!
//! ## Integration model
//!
//! `ObservationStore` is an `Arc<Mutex<HashMap<DroneId, ...>>>` shared
//! between this module and `capture.rs`. The capture loop calls
//! `store.record(&payload)` after building each `NormalizedTelemetry`
//! (right before `inner_tx.try_send`). The tick publisher reads via
//! `store.snapshot_and_prune()` once per second.
//!
//! The shared `Arc<ArcSwap<Option<GpsFix>>>` mirrors the Wave 7.2
//! pattern: GPS sensor task writes the latest fix; the tick publisher
//! reads it lock-free per tick to stamp `node_location` with live GPS
//! when fresh, config-static when stale (Wave 7.4 Reputation-
//! Portability defense).
//!
//! ## Wire format
//!
//! JSON for Wave 6.5; will swap to Cap'n Proto `EcologeeEnvelope` at
//! Wave 8.5 cutover. D27 envelope v2 omits unknown position/altitude and
//! carries observed operational status. Deployment is BLOCKED until the
//! receiving Go consumer handles absent position explicitly: its v1 value
//! type otherwise fabricates 0,0. No consumer gate is changed here.
//!
//! See:
//!   - /srv/pack-stack/docs/ecologee-meta-protocol.md §3 + §4.1
//!   - globe-web/internal/nats/airspace_consumer.go (the receiving end)
//!   - project_evsm_red_color_protocol_2026-06-04.md

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use cybrrd_rid_protocol::models::{
    GeoPoint, NodeLocation, NormalizedTelemetry, PositionSource, ProtocolType,
};
use serde::Serialize;
use tokio::time::{interval, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::nats_publisher::NatsHandle;
use crate::sensor_gps::GpsFix;

/// NATS subject prefix — `<prefix>.<node_id>` is the full subject.
/// Color-line at root per pack-consensus 2026-06-04.
const SUBJECT_PREFIX: &str = "cybrrd.green.airspace";

/// D27's optional position/altitude is a breaking green-envelope contract.
/// Separate from the canonical per-frame CBOS wire-format version (4).
const ENVELOPE_VERSION: &str = "2.0";

/// Maximum age of an observation before it's pruned from the store.
/// Mirrors the pack-canonical 10 s stale threshold (2× cardinal
/// heartbeat) from Gemini directive 2026-06-04. Phase 3 will surface
/// `position_state="stale"` for observations approaching this; Phase 1
/// silently prunes.
const STALE_THRESHOLD_MS: u64 = 10_000;

/// Phase 1 scope: emit only drones whose last broadcast is at most this
/// old. Substrate-truth: anything older is no longer being observed,
/// and Phase 1 doesn't yet model projection. Phase 3 lifts this gate
/// and starts emitting projected / stale states.
const FRESH_TICK_WINDOW_MS: u64 = 1_000;

/// GPS fix freshness threshold for stamping `node_location` from a
/// live fix. Mirrors `capture.rs::GPS_FIX_FRESH_THRESHOLD_MS` so the
/// per-frame audit envelope and the AirspaceState envelope agree on
/// "what's stale."
const GPS_FIX_FRESH_THRESHOLD_MS: i64 = 30_000;

/// Edge certainty: the brick's self-attested confidence that this
/// envelope is substrate-truthful. Phase 1 hardcodes 0.95 when GPS is
/// fresh, 0.50 when falling back to config-static. Phase 3+ may derive
/// this from sensor health composite.
const EDGE_CERTAINTY_GPS_LIVE: f32 = 0.95;
const EDGE_CERTAINTY_CONFIG_STATIC: f32 = 0.50;

// ============================================================================
// Wire-format types
// ============================================================================
//
// The receiving Go consumer MUST migrate before deployment of envelope v2.
// A version label alone does not make the old consumer absence-safe.

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionState {
    Measured,
    Unknown,
    Projected,
    Stale,
}

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct VelocityVector {
    pub vx_mps: f32,
    pub vy_mps: f32,
    pub vz_mps: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct AirspaceDroneObservation {
    pub drone_id: String,
    pub protocol: ProtocolType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<GeoPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operational_status: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_unknown_reason: Option<cybrrd_rid_protocol::models::PositionUnknownReason>,
    pub velocity: VelocityVector,
    pub position_state: PositionState,
    pub last_broadcast_unix_ms: u64,
    pub broadcast_age_ms: u64,
    pub signal_rssi_dbm: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct AirspaceStateEnvelope {
    pub envelope_version: &'static str,
    pub color_state: &'static str,
    pub invariant_delta: bool,
    pub edge_certainty: f32,
    pub node_id: String,
    pub node_location: NodeLocation,
    pub tick_unix_ms: u64,
    pub tick_seq: u64,
    pub drones: Vec<AirspaceDroneObservation>,
}

// ============================================================================
// Observation store (capture-side write, tick-side read)
// ============================================================================

/// Per-drone latest-observation cache feeding the tick publisher.
/// Capture loop writes via `record()`; tick publisher reads via
/// `snapshot_and_prune()` once per second.
///
/// Uses `std::sync::Mutex` rather than `tokio::sync::Mutex` — lock
/// hold is sub-µs (hashmap insert / iter) so the async overhead would
/// dwarf the work. Capture-loop blocking on the mutex for ns is
/// completely fine.
#[derive(Debug, Default)]
pub struct ObservationStore {
    inner: Mutex<HashMap<String, CachedObservation>>,
}

#[derive(Debug, Clone)]
struct CachedObservation {
    protocol: ProtocolType,
    position: Option<GeoPoint>,
    operational_status: Option<u8>,
    position_unknown_reason: Option<cybrrd_rid_protocol::models::PositionUnknownReason>,
    velocity: VelocityVector,
    last_broadcast_unix_ms: u64,
    signal_rssi_dbm: i32,
}

impl CachedObservation {
    fn into_wire(self, drone_id: String, now_ms: u64) -> AirspaceDroneObservation {
        let position_state = if self.position.is_some() { PositionState::Measured } else { PositionState::Unknown };
        AirspaceDroneObservation {
            drone_id, protocol: self.protocol, position: self.position,
            operational_status: self.operational_status,
            position_unknown_reason: self.position_unknown_reason,
            velocity: self.velocity, position_state,
            last_broadcast_unix_ms: self.last_broadcast_unix_ms,
            broadcast_age_ms: now_ms.saturating_sub(self.last_broadcast_unix_ms),
            signal_rssi_dbm: self.signal_rssi_dbm,
        }
    }
}

impl ObservationStore {
    pub fn new() -> Arc<Self> {
        Arc::new(ObservationStore {
            inner: Mutex::new(HashMap::new()),
        })
    }

    /// Record one observation. Called from the capture loop after
    /// building the per-frame `NormalizedTelemetry`. Best-effort: a
    /// poisoned mutex silently no-ops rather than panicking the
    /// capture path (which is on the radio-RX hot path).
    ///
    /// Velocity defaults to zero in Phase 1 (the ASTM F3411 velocity-
    /// vector parser extension is Phase 3a — until then projection
    /// would be stationary anyway).
    pub fn record(&self, payload: &NormalizedTelemetry) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        g.insert(
            payload.data.drone_id.clone(),
            CachedObservation {
                protocol: payload.data.protocol.clone(),
                position: payload.data.pos.clone(),
                operational_status: payload.data.operational_status,
                position_unknown_reason: payload.data.position_unknown_reason,
                velocity: VelocityVector::default(),
                last_broadcast_unix_ms: payload.timestamp_utc,
                signal_rssi_dbm: payload.data.signal_rssi_dbm,
            },
        );
    }

    /// Tick-side: snapshot the cache and prune entries past the stale
    /// threshold. Returns owned clones so the caller can release the
    /// lock immediately.
    fn snapshot_and_prune(&self, now_ms: u64) -> Vec<(String, CachedObservation)> {
        let Ok(mut g) = self.inner.lock() else {
            return Vec::new();
        };
        g.retain(|_, obs| {
            now_ms.saturating_sub(obs.last_broadcast_unix_ms) < STALE_THRESHOLD_MS
        });
        g.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Diagnostic: current cache size (live + about-to-be-pruned).
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }
}

// ============================================================================
// Tick publisher loop
// ============================================================================

/// Run the 1 Hz Green Protocol tick publisher. Blocks until `cancel`
/// fires or an unrecoverable error occurs. Errors per-tick (serialize
/// fail, publish fail) are logged + counted but never crash the loop —
/// the next tick is a fresh attempt.
pub async fn run_tick_publisher(
    node_id: String,
    static_node_location: NodeLocation,
    store: Arc<ObservationStore>,
    gps_latest: Arc<ArcSwap<Option<GpsFix>>>,
    handle: NatsHandle,
    cancel: CancellationToken,
) {
    let subject = format!("{}.{}", SUBJECT_PREFIX, node_id);
    println!(
        "[green-tick] starting publisher on subject={} (1 Hz Green Protocol)",
        subject
    );

    let mut ticker = interval(Duration::from_millis(1000));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // First tick fires immediately — drop it so the first emission lands
    // at +1 s, not at t=0.
    ticker.tick().await;

    let mut tick_seq: u64 = 0;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                println!("[green-tick] cancellation received; exiting");
                return;
            }
            _ = ticker.tick() => {}
        }

        tick_seq = tick_seq.saturating_add(1);
        let now_ms = system_time_ms();

        // Snapshot the cardinal's current witness identity. Live GPS
        // fix when fresh, config-static otherwise. Wave 7.4 attestation
        // ride-along.
        let (node_location, edge_certainty) =
            resolve_node_location(&gps_latest, &static_node_location, now_ms);

        // Snapshot the per-drone observation cache. snapshot_and_prune
        // drops entries past STALE_THRESHOLD_MS as a side-effect so the
        // store can't grow unbounded if a drone leaves the airspace
        // without a clean exit.
        let observations = store.snapshot_and_prune(now_ms);

        // Phase 1 emit gate: only drones observed within the current
        // tick window. Phase 3 will lift this and emit projected /
        // stale states for older observations.
        let mut drones = Vec::with_capacity(observations.len());
        for (drone_id, obs) in observations {
            let age_ms = now_ms.saturating_sub(obs.last_broadcast_unix_ms);
            if age_ms >= FRESH_TICK_WINDOW_MS {
                continue;
            }
            drones.push(obs.into_wire(drone_id, now_ms));
        }

        let envelope = AirspaceStateEnvelope {
            envelope_version: ENVELOPE_VERSION,
            color_state: "green",
            invariant_delta: false,
            edge_certainty,
            node_id: node_id.clone(),
            node_location,
            tick_unix_ms: now_ms,
            tick_seq,
            drones,
        };

        let bytes = match serde_json::to_vec(&envelope) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[green-tick] serialize error (skipping tick): {}", e);
                continue;
            }
        };

        // Non-blocking enqueue onto the shared supervised connection. The
        // supervisor owns delivery/flush/reconnect; a tick is a best-effort
        // beacon, so a momentarily-full handoff simply drops to the next tick's
        // fresh state (consumers detect gaps via tick_seq).
        handle.publish(subject.clone(), bytes);
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn system_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolve the cardinal's witness location for this tick. Live GPS
/// when fresh; falls back to static config when GPS is stale or
/// absent. Mirrors `capture.rs::stamp_node_with_gps` semantics so the
/// per-frame audit envelope and the AirspaceState envelope agree on
/// witness truth.
///
/// Returns `(NodeLocation, edge_certainty)` — certainty is higher for
/// GPS-anchored fixes than for config-static fallbacks.
fn resolve_node_location(
    gps_latest: &ArcSwap<Option<GpsFix>>,
    fallback: &NodeLocation,
    now_ms: u64,
) -> (NodeLocation, f32) {
    let guard = gps_latest.load();
    if let Some(fix) = guard.as_ref() {
        // `now_ms` is u64; GpsFix.fix_at_ms is i64 (engine-wall-clock).
        // Defensive cast: if either bound underflows the conversion, fall
        // back to config-static.
        if let Ok(now_signed) = i64::try_from(now_ms) {
            if now_signed - fix.fix_at_ms <= GPS_FIX_FRESH_THRESHOLD_MS {
                return (
                    NodeLocation {
                        lat: fix.lat,
                        lon: fix.lon,
                        alt_m: fix.alt_m,
                        position_source: Some(PositionSource::GpsLive),
                    },
                    EDGE_CERTAINTY_GPS_LIVE,
                );
            }
        }
    }
    (fallback.clone(), EDGE_CERTAINTY_CONFIG_STATIC)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cybrrd_rid_protocol::models::{Node, TelemetryData};

    fn sample_payload(drone_id: &str, ts_ms: u64) -> NormalizedTelemetry {
        NormalizedTelemetry {
            wire_format_version: Some(cybrrd_rid_protocol::models::WIRE_FORMAT_VERSION),
            node: Node {
                id: "test-node-001".into(),
                location: NodeLocation {
                    lat: 40.882722,
                    lon: -95.691074,
                    alt_m: 361.5,
                    position_source: Some(PositionSource::ConfigStatic),
                },
                version: "test".into(),
            },
            timestamp_utc: ts_ms,
            data: TelemetryData {
                transport: None,
                message_counter: None,
                protocol: ProtocolType::AstmF3411_22a,
                mac_address: [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc],
                drone_id: drone_id.into(),
                hardware_serial: None,
                caa_registration: None,
                operational_status: Some(2),
                position_unknown_reason: None,
                operator_position_unknown_reason: None,
                pos: Some(GeoPoint {
                    lat: 40.882515,
                    lon: -95.690941,
                    alt_m: Some(361.5),
                }),
                operator_pos: None,
                operator_id: None,
                self_id: None,
                auth: None,
                signal_rssi_dbm: -65,
            },
        }
    }

    #[test]
    fn store_records_and_snapshots() {
        let store = ObservationStore::new();
        store.record(&sample_payload("DRONE-A", 1000));
        store.record(&sample_payload("DRONE-B", 2000));
        assert_eq!(store.len(), 2);

        let snap = store.snapshot_and_prune(2500);
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn unknown_position_replaces_known_fix_in_tick_envelope() {
        use cybrrd_rid_protocol::models::PositionUnknownReason;
        let store = ObservationStore::new();
        let mut payload = sample_payload("D27E", 1000);
        store.record(&payload);
        payload.timestamp_utc = 2000;
        payload.data.pos = None;
        payload.data.operational_status = Some(0);
        payload.data.position_unknown_reason = Some(PositionUnknownReason::ZeroPair);
        store.record(&payload);
        let snapshot = store.snapshot_and_prune(2001);
        assert_eq!(snapshot.len(), 1);
        let (id, observation) = snapshot.into_iter().next().unwrap();
        let value = serde_json::to_value(observation.into_wire(id, 2001)).unwrap();
        assert_eq!(value["drone_id"], "D27E");
        assert!(value.get("position").is_none());
        assert_eq!(value["position_state"], "unknown");
        assert_eq!(value["position_unknown_reason"], "zero_pair");
        assert_eq!(value["operational_status"], 0);
    }

    #[test]
    fn store_prunes_stale_entries() {
        let store = ObservationStore::new();
        store.record(&sample_payload("DRONE-A", 1000));
        store.record(&sample_payload("DRONE-B", 15_000));
        // now_ms = 16_000 → DRONE-A is 15_000 ms old (>= 10_000) → prune.
        // DRONE-B is 1_000 ms old → keep.
        let snap = store.snapshot_and_prune(16_000);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, "DRONE-B");
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn store_latest_wins_for_duplicate_drone_id() {
        let store = ObservationStore::new();
        store.record(&sample_payload("DRONE-A", 1000));
        store.record(&sample_payload("DRONE-A", 2500));
        assert_eq!(store.len(), 1);
        let snap = store.snapshot_and_prune(3000);
        assert_eq!(snap[0].1.last_broadcast_unix_ms, 2500);
    }

    #[test]
    fn envelope_serializes_with_locked_field_names() {
        // Substrate-truth gate: the JSON serialization MUST exactly match
        // the Go consumer's expected field names (globe-web/pkg/types/airspace.go).
        // This test catches drift before it becomes a wire-format substrate
        // bug like the 2026-05-29 lng/lon WSOD.
        let envelope = AirspaceStateEnvelope {
            envelope_version: ENVELOPE_VERSION,
            color_state: "green",
            invariant_delta: false,
            edge_certainty: 0.95,
            node_id: "test-001".into(),
            node_location: NodeLocation {
                lat: 40.882722,
                lon: -95.691074,
                alt_m: 361.5,
                position_source: Some(PositionSource::GpsLive),
            },
            tick_unix_ms: 1_780_591_600_000,
            tick_seq: 42,
            drones: vec![AirspaceDroneObservation {
                drone_id: "DRONE-X".into(),
                protocol: ProtocolType::AstmF3411_22a,
                operational_status: Some(2),
                position_unknown_reason: None,
                position: Some(GeoPoint {
                    lat: 40.882515,
                    lon: -95.690941,
                    alt_m: Some(361.5),
                }),
                velocity: VelocityVector::default(),
                position_state: PositionState::Measured,
                last_broadcast_unix_ms: 1_780_591_599_800,
                broadcast_age_ms: 200,
                signal_rssi_dbm: -65,
            }],
        };
        let json = serde_json::to_string(&envelope).expect("serialize");

        // Field-name canaries — exact match to the Go consumer.
        assert!(json.contains("\"envelope_version\":\"2.0\""));
        assert!(json.contains("\"color_state\":\"green\""));
        assert!(json.contains("\"invariant_delta\":false"));
        assert!(json.contains("\"edge_certainty\":0.95"));
        assert!(json.contains("\"node_id\":\"test-001\""));
        assert!(json.contains("\"tick_unix_ms\":1780591600000"));
        assert!(json.contains("\"tick_seq\":42"));
        assert!(json.contains("\"position_state\":\"measured\""));
        assert!(json.contains("\"broadcast_age_ms\":200"));
        assert!(json.contains("\"last_broadcast_unix_ms\":1780591599800"));
        assert!(json.contains("\"signal_rssi_dbm\":-65"));
        // VelocityVector keys
        assert!(json.contains("\"vx_mps\":0.0"));
        assert!(json.contains("\"vy_mps\":0.0"));
        assert!(json.contains("\"vz_mps\":0.0"));
    }

    #[test]
    fn empty_envelope_serializes_correctly() {
        // Empty tick: no drones observed. Envelope still emits the full
        // metadata + an empty drones array. This IS substrate-truth.
        let envelope = AirspaceStateEnvelope {
            envelope_version: ENVELOPE_VERSION,
            color_state: "green",
            invariant_delta: false,
            edge_certainty: 0.50,
            node_id: "test-001".into(),
            node_location: NodeLocation {
                lat: 40.882722,
                lon: -95.691074,
                alt_m: 361.5,
                position_source: Some(PositionSource::ConfigStatic),
            },
            tick_unix_ms: 1_780_591_600_000,
            tick_seq: 1,
            drones: vec![],
        };
        let json = serde_json::to_string(&envelope).expect("serialize");
        assert!(json.contains("\"drones\":[]"));
        // Empty tick still asserts color-line + tick_seq, so a consumer
        // can detect missed ticks.
        assert!(json.contains("\"tick_seq\":1"));
    }
}
