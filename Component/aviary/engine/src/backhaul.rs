//! Telemetry pump — drains captured frames onto the single multiplexed,
//! supervised NATS connection (#178).
//!
//! ## History
//! This module *was* the Warm-Capture Supervisor (#176, ratified 2026-06-16):
//! it owned a dedicated telemetry NATS connection plus all the edge-buffering /
//! wedge-detect / in-place-reconnect armor. #178 (ratified 2026-06-17, Cy +
//! Gemini + Synth) lifted that supervisor out into [`crate::nats_publisher`] and
//! generalized it to carry ALL message types (telemetry / heartbeat / audit /
//! substrate-audit / green-tick) over ONE connection. The armor is now shared by
//! every path — most importantly the heartbeat liveness nerve, which previously
//! rode a bare unsupervised client and could silently wedge while telemetry
//! pumped fine.
//!
//! ## What's left here
//! Just the telemetry-specific glue: drain the capture handoff channel
//! (`mpsc::Receiver<NormalizedTelemetry>`), JSON-serialize each frame, derive its
//! per-node subject, and hand the bytes to the [`NatsHandle`]. All buffering,
//! batching, flush-confirm delivery, and reconnect now live in the supervisor —
//! `handle.publish` is a non-blocking enqueue, so capture (System 1) still never
//! blocks on the network (System 2). A serialization failure is a bug, not a
//! network condition: it's counted and the poison frame dropped.

use crate::audit::DropCounters;
use crate::nats_publisher::NatsHandle;
use cybrrd_rid_protocol::models::NormalizedTelemetry;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;

/// Telemetry subject prefix; full subject is `<prefix>.<node_id>`.
const TELEMETRY_SUBJECT_PREFIX: &str = "cybrrd.telemetry.frame.rid";

/// Drain captured frames and publish them onto the shared supervised connection.
/// Returns when the capture channel closes (clean shutdown).
pub async fn run_telemetry_pump(
    mut rx: mpsc::Receiver<NormalizedTelemetry>,
    handle: NatsHandle,
    counters: DropCounters,
) {
    println!("[backhaul] telemetry pump starting (publishing via multiplexed NATS supervisor)");
    while let Some(frame) = rx.recv().await {
        let subject = format!("{}.{}", TELEMETRY_SUBJECT_PREFIX, frame.node.id);
        match serde_json::to_vec(&frame) {
            Ok(bytes) => handle.publish(subject, bytes),
            Err(e) => {
                // Serialization failure is a bug, not a network condition.
                // Count it; the poison frame is dropped.
                eprintln!("[backhaul] serde error (frame dropped): {}", e);
                counters.publish_failed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    println!("[backhaul] capture channel closed; telemetry pump exiting");
}
