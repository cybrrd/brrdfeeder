// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Sensor trait — common abstraction over the engine's edge producers.
//!
//! Wave 7.2 scaffold (2026-05-26). First non-Wi-Fi producer is
//! `UbloxGps` (this cut); BLE + SDR follow as additional impls in
//! Wave 8.0. The trait deliberately layers OVER the engine's existing
//! `tokio::spawn(run_X(...))` idiom rather than replacing it — each
//! `Sensor::start` returns a `SensorHandle` carrying the readings
//! receiver + a lock-free health snapshot, and the engine keeps
//! its current spawn-pattern visibility.
//!
//! Pack-ratified design corrections (2026-05-26 round-robin):
//!   - **No tracing-subscriber smuggle.** The engine uses `println!`/
//!     `eprintln!` today. Sensors emit `[<name>] ...` to match the
//!     existing visual idiom. Migration to `tracing` is a separate
//!     pre-Wave-8.0 task; bundling it into a sensor cut would be a
//!     Systemic Parasite (Gemini's term, 2026-05-26).
//!   - **No H3 computation in the engine.** Wave 6.3 lock places
//!     lat/lon → H3 res7/res8 at the lake-writer's row-decoration
//!     step. Sensors emit lat/lon only; the existing ingest path
//!     does the spatial indexing. Adding H3 here would create a
//!     second computation site that could drift.
//!
//! The trait + first impl are the ONLY surface this cut touches in
//! the engine. The wire-format extension (GPS-derived lat/lon
//! flowing into `AuditEnvelope.frame.node.location`) is the natural
//! next cut once we observe `gps:u-blox-7` going Healthy on cardinal.

use std::sync::Arc;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::audit::SubstrateAuditEvent;

/// Runtime context handed to every sensor at `start()`.
///
/// Carries:
///   - node identity (so substrate-audit events stamp correctly)
///   - cancellation token (engine-wide shutdown signal)
///   - substrate-audit channel (sensors fire state-transition events
///     here; they ride the existing Wave 7.1 Inc 7 substrate-audit
///     emitter to NATS audit subjects)
#[derive(Clone)]
pub struct SensorContext {
    pub node_id: Arc<str>,
    pub cancel: CancellationToken,
    pub substrate_audit: mpsc::Sender<SubstrateAuditEvent>,
}

/// Health snapshot. Heartbeat emitter (and operator dashboards via
/// the future heartbeat extension) sample this at 1 Hz.
///
/// Lock-free read via `ArcSwap::load()`; sensor task writes via
/// `ArcSwap::store(new)`. No mutex contention on the hot path.
#[derive(Clone, Debug, Serialize)]
pub struct SensorHealth {
    pub name: &'static str,
    pub state: SensorState,
    /// Unix-ms timestamp of the last successful reading (Wave 6.4.1
    /// wire-format lock). `None` until the first reading arrives.
    pub last_reading_ms: Option<i64>,
    /// Monotonic count of recoverable errors (parse fails, transient
    /// serial errors). Useful as a quality signal in heartbeats.
    pub error_count: u64,
    /// Free-form context — last error message, lock-acquisition note,
    /// etc. Cleared on Healthy transition.
    pub detail: Option<String>,
}

impl SensorHealth {
    pub fn initializing(name: &'static str) -> Self {
        SensorHealth {
            name,
            state: SensorState::Initializing,
            last_reading_ms: None,
            error_count: 0,
            detail: None,
        }
    }
}

/// Sensor lifecycle states. Same vocabulary as the engine's other
/// health surfaces (radio_status, etc.).
///
/// Wire-format serde: snake_case matches the existing convention used
/// by `RadioStatus`, `CoverageClass`, etc. When this enum flows out
/// in a heartbeat payload (Wave 7.3b), downstream consumers see
/// `"healthy"` / `"degraded"` / etc., not the Rust variant names.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SensorState {
    /// Constructing connection, waiting for first reading.
    Initializing,
    /// At least one reading received within `stale_after`.
    Healthy,
    /// Connection alive but data quality reduced (e.g., GPS 2D-only,
    /// BLE no advertisers, SDR no peaks). Sensor still emits.
    Degraded,
    /// Unrecoverable: hardware disconnected, serial port gone, etc.
    /// Sensor task has exited or is in indefinite reconnect-loop.
    Failed,
}

/// Returned from `Sensor::start`. The engine consumes readings via
/// the receiver and samples health snapshots through the ArcSwap.
pub struct SensorHandle<R: Send + 'static> {
    pub readings: mpsc::Receiver<R>,
    pub health: Arc<ArcSwap<SensorHealth>>,
}

/// The trait.
///
/// First impl (this cut): `UbloxGps` in `sensor_gps.rs`.
/// Next impls (Wave 8.0): `HolyIotBle`, `NesdrSpectrum`.
///
/// `start` consumes `self` because each instance is started exactly
/// once; restart-on-failure constructs a fresh `Sensor`.
pub trait Sensor: Send + 'static {
    /// What this sensor emits.
    type Reading: Send + 'static;
    /// Stable snake_case identifier (e.g. `"gps:u-blox-7"`).
    fn name(&self) -> &'static str;
    /// Spawn the sensor's task and return its handle.
    fn start(self, ctx: SensorContext) -> SensorHandle<Self::Reading>;
}

/// Unix-ms timestamp helper. Matches Wave 6.4.1 wire-format lock for
/// `frame.timestamp_utc`. Sensors use this for `last_reading_ms` and
/// reading-side timestamps (e.g., `GpsFix::fix_at_ms`).
pub(crate) fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
