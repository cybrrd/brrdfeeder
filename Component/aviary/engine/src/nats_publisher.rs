// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! The single multiplexed, supervised NATS connection (#178).
//!
//! Pack-ratified 2026-06-17 (Cy + Gemini + Synth). Consolidates the engine's
//! five previously-independent NATS clients (telemetry / heartbeat / audit /
//! substrate-audit / green-tick) into ONE supervised connection. Five TCP
//! connections to the same broker on an embedded edge node was an anti-pattern,
//! and only telemetry carried the warm-capture armor — the **heartbeat liveness
//! nerve** could silently wedge while telemetry pumped fine, making any future
//! command surface believe a healthy node was dead. Now every publisher routes
//! through this one supervisor, so every path inherits the same armor:
//!
//!   - transactional flush-confirm delivery (a frame leaves the edge buffer only
//!     once the broker round-trip confirms receipt — closes the fire-and-forget
//!     silent-loss hole);
//!   - wedge detection via timeout(FLUSH_TIMEOUT, flush()), WEDGE_GRACE patience;
//!   - in-place reconnect (exp backoff, retries forever) with NO process restart;
//!   - bounded edge buffer (ring, drop-oldest) that survives the outage.
//!
//! Producers hold a cheap, cloneable [`NatsHandle`] and call `publish(subject,
//! bytes)` — a non-blocking enqueue. The payload is already serialized: this
//! module is transport-only, type-agnostic (it carries opaque `Outbound`).
//!
//! v1 uses a single drop-oldest buffer for all message types. Priority lanes
//! (never drop a heartbeat/algedonic in favour of a telemetry frame) are a
//! noted v2 refinement.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::{Client, ConnectOptions};
use bytes::Bytes;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::audit::DropCounters;

/// Max messages retained locally during a backhaul outage (ring buffer; oldest
/// evicted on overflow). WHAT: depth between producers and the broker, across
/// ALL message types. WHY: a mobile sensor must keep witnessing through cell
/// dead-zones; drop-oldest = freshest telemetry wins under sustained overflow.
/// WHEN-to-tune: raise for longer-outage retention at more RAM. DEPENDS-ON:
/// per-message size, node RAM.
const EDGE_BUFFER_CAPACITY: usize = 10_000;

/// Messages per publish→flush delivery transaction. WHY: amortizes the
/// confirming flush() round-trip over a batch while bounding unconfirmed
/// in-flight messages.
const PUBLISH_BATCH: usize = 256;

/// Idle liveness-probe cadence (buffer empty → no traffic to carry the
/// round-trip). A wedge must be caught even when nothing is flowing.
const LIVENESS_PROBE_INTERVAL: Duration = Duration::from_secs(15);

/// Single flush round-trip deadline. A healthy broker PONGs in <1 s even over
/// LTE; 8 s tolerates a slow-but-alive link without false-positiving.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(8);

/// Sustained no-confirmed-round-trip window before declaring a wedge and
/// reconnecting in place. THE patience knob. (Effective detection is this +
/// async-nats's own dead-socket detection ≈ +60 s; see #177.)
const WEDGE_GRACE: Duration = Duration::from_secs(60);

/// Wait between flush retries within the grace window (batch already enqueued;
/// we retry the confirm, not the publish).
const FLUSH_RETRY: Duration = Duration::from_secs(1);

const RECONNECT_BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Handoff channel depth (producers → supervisor). The real outage buffer is the
/// VecDeque below; this just needs headroom for the supervisor's non-draining
/// windows (a connect attempt / flush).
const HANDOFF_CAPACITY: usize = 1024;

/// One already-serialized message bound for a NATS subject.
#[derive(Clone)]
pub struct Outbound {
    pub subject: String,
    pub payload: Bytes,
}

/// Cheap, cloneable handle every producer holds. `publish` is a non-blocking
/// enqueue into the supervisor's handoff channel; on a (rare) full channel the
/// message is dropped and counted — the supervisor's ring buffer is the real
/// elastic capacity.
#[derive(Clone)]
pub struct NatsHandle {
    tx: mpsc::Sender<Outbound>,
    counters: DropCounters,
    pub status: Arc<NatsStatus>,
}

/// Only connection state and confirmed-publish time are exposed to status.
/// No URLs, subjects, credentials, tokens, or broker errors are serialized.
#[derive(Default)]
pub struct NatsStatus {
    client: arc_swap::ArcSwapOption<Client>,
    pub last_publish_ms: AtomicU64,
}

impl NatsStatus {
    pub fn connection_state(&self) -> String {
        self.client
            .load()
            .as_ref()
            .map(|c| c.connection_state().to_string())
            .unwrap_or_else(|| "disconnected".to_string())
    }
}

impl NatsHandle {
    /// Enqueue an already-serialized payload for `subject`. Never blocks the
    /// caller (capture loop, heartbeat, etc.).
    pub fn publish(&self, subject: String, payload: Vec<u8>) {
        let msg = Outbound { subject, payload: Bytes::from(payload) };
        if self.tx.try_send(msg).is_err() {
            // Handoff full (supervisor briefly not draining) — drop + account.
            self.counters.publish_failed.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// #185 Phase 3 Drop 1c — the inbound control channel. When present, the
/// supervisor also binds a durable JetStream pull consumer on `stream` filtered
/// to `subject`, runs each message through the E-CNP-001 verification membrane
/// (`blue_policy::process_blue`), logs the verdict, and acks. VERIFY ONLY — no
/// execution (Drop 2). Re-bound on every reconnect so a node offline at
/// issue-time still pulls its pending Blue the moment it returns.
#[derive(Clone)]
pub struct ControlConfig {
    pub node_id: String,
    pub stream: String,
    pub subject: String,
    pub state_path: std::path::PathBuf,
    /// Verified running identity's policy floor. None refuses every update;
    /// runtime ENV or build sequence alone cannot establish this precondition.
    pub running_build_seq: Option<u64>,
}

/// Start the supervised connection. Spawns the supervisor task and returns the
/// handle producers publish through. Replaces the five inline connects.
pub fn start(
    broker_urls: Vec<String>,
    credentials_path: String,
    counters: DropCounters,
    control: Option<ControlConfig>,
) -> NatsHandle {
    let (tx, rx) = mpsc::channel::<Outbound>(HANDOFF_CAPACITY);
    let status = Arc::new(NatsStatus::default());
    let handle = NatsHandle {
        tx,
        counters: counters.clone(),
        status: Arc::clone(&status),
    };
    tokio::spawn(supervisor_loop(
        rx, broker_urls, credentials_path, counters, control, status,
    ));
    handle
}

enum PumpExit {
    ChannelClosed,
    Wedged,
}

async fn supervisor_loop(
    mut rx: mpsc::Receiver<Outbound>,
    broker_urls: Vec<String>,
    credentials_path: String,
    counters: DropCounters,
    control: Option<ControlConfig>,
    status: Arc<NatsStatus>,
) {
    println!(
        "[nats] multiplexed supervisor starting; broker(s)={:?} creds={} edge_buffer={}",
        broker_urls, credentials_path, EDGE_BUFFER_CAPACITY
    );
    let mut buffer: VecDeque<Outbound> = VecDeque::with_capacity(1024);

    loop {
        let client = match connect_with_backoff(
            &broker_urls,
            &credentials_path,
            &mut rx,
            &mut buffer,
            &counters,
        )
        .await
        {
            Some(c) => c,
            None => {
                println!("[nats] handoff closed during reconnect; supervisor exiting");
                return;
            }
        };
        println!("[nats] connected; pumping (buffered={})", buffer.len());
        status.client.store(Some(Arc::new(client.clone())));

        // #185 Drop 1c — the inbound control consumer rides THIS connection
        // (the #178 ethos: one supervised link). It holds a client clone, so we
        // must abort it before drop(client), or the clone would pin the
        // connection open and defeat the wedge-reconnect.
        let consumer_task = control
            .as_ref()
            .map(|ctrl| tokio::spawn(control_consumer(client.clone(), ctrl.clone())));

        let outcome = pump(&client, &mut rx, &mut buffer, &counters, &status).await;
        status.client.store(None); // release the clone before reconnect/shutdown
        match outcome {
            PumpExit::ChannelClosed => {
                if let Some(h) = consumer_task {
                    h.abort();
                }
                let _ = timeout(FLUSH_TIMEOUT, client.flush()).await;
                println!("[nats] handoff closed; supervisor exiting");
                return;
            }
            PumpExit::Wedged => {
                if let Some(h) = consumer_task {
                    h.abort();
                }
                eprintln!(
                    "[nats] WEDGE: no confirmed broker round-trip in {}s — dropping client, \
                     reconnecting in place (capture stays warm; {} buffered)",
                    WEDGE_GRACE.as_secs(),
                    buffer.len()
                );
                drop(client);
            }
        }
    }
}

/// #185 Drop 1c — the inbound E-CNP-001 verification membrane. Binds a durable
/// JetStream pull consumer (so a Blue issued while the node was offline is
/// delivered on return), runs each control packet through
/// `blue_policy::process_blue`, logs the verdict, and acks. VERIFY ONLY — Drop 1
/// never executes. Panic-free (#179): every error logs and returns/continues;
/// the supervisor respawns this on the next connect.
/// Bind the durable control consumer and open its message stream as ONE
/// fallible unit, so the caller can retry the whole sequence on a backoff.
/// (A slow/transient first attempt on a non-ideal link must not leave the
/// membrane permanently idle — the #185 hummingbird/cathartes bind-fragility.)
async fn bind_control_membrane(
    js: &async_nats::jetstream::Context,
    ctrl: &ControlConfig,
    durable: &str,
) -> Result<async_nats::jetstream::consumer::pull::Stream, String> {
    use async_nats::jetstream;
    let stream = js
        .get_stream(&ctrl.stream)
        .await
        .map_err(|e| format!("get_stream {}: {e}", ctrl.stream))?;
    let consumer = stream
        .get_or_create_consumer(
            durable,
            jetstream::consumer::pull::Config {
                durable_name: Some(durable.to_string()),
                filter_subject: ctrl.subject.clone(),
                ack_policy: jetstream::consumer::AckPolicy::Explicit,
                ..Default::default()
            },
        )
        .await
        .map_err(|e| format!("bind consumer {durable}: {e}"))?;
    consumer
        .messages()
        .await
        .map_err(|e| format!("consumer.messages: {e}"))
}

async fn control_consumer(client: Client, ctrl: ControlConfig) {
    use crate::blue_policy::{self, Verdict};
    use async_nats::jetstream;
    use futures::StreamExt;

    // The default JetStream request timeout (5s) is too tight for non-ideal
    // edge links: the get_stream/get_or_create_consumer request-replies time out
    // on a slow first attempt and the membrane never binds (reproduced live on
    // hummingbird at 60ms RTT, and on cathartes over LTE). Extend it.
    let mut js = jetstream::new(client);
    js.set_timeout(std::time::Duration::from_secs(30));

    // Bind-retry-with-backoff. Previously a single timeout `return`ed, killing
    // the membrane until a NATS *reconnect* — but the connection stays up after
    // a JS-API timeout, so no reconnect fires and the node sits control-less
    // forever. Retry the whole bind on a capped backoff so a slow/transient
    // first attempt self-heals. Telemetry/heartbeat keep flowing on the same
    // (healthy) connection throughout; only the control membrane is retrying.
    let durable = format!("blue-{}", ctrl.node_id);
    let mut backoff = std::time::Duration::from_secs(2);
    let max_backoff = std::time::Duration::from_secs(60);
    let mut messages = loop {
        match bind_control_membrane(&js, &ctrl, &durable).await {
            Ok(m) => break m,
            Err(e) => {
                eprintln!(
                    "[blue] membrane bind failed ({e}) — retrying in {}s (control idle until then)",
                    backoff.as_secs()
                );
                tokio::time::sleep(jittered_delay(backoff)).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    };
    let state = blue_policy::PolicyState::load(&ctrl.state_path);
    println!(
        "[blue] control membrane bound: stream={} filter={} durable={} (verify-only)",
        ctrl.stream, ctrl.subject, durable
    );

    while let Some(item) = messages.next().await {
        let msg = match item {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[blue] message stream error: {e}");
                continue;
            }
        };
        let outcome = blue_policy::process_blue(
            &msg.payload,
            &ctrl.node_id,
            &state,
            ctrl.running_build_seq,
            &blue_policy::KEYRING,
        );
        match (&outcome.verdict, &outcome.policy) {
            // D44: cryptographic verification is retained, but Blue has NO
            // update authority. Only the node's signed-release poller stages.
            (Verdict::Verified, Some(p)) => println!(
                "[BLUE VERIFIED; UPDATE RETIRED] node={} target_build_seq={} — use signed ring release",
                p.node_id, p.target_build_seq
            ),
            (v, p) => eprintln!(
                "[BLUE REJECTED] verdict={:?} node={} (dropped)",
                v,
                p.as_ref().map(|x| x.node_id.as_str()).unwrap_or("?")
            ),
        }
        // Drop 1: ack on receipt (verify-only). Drop 2 will move the ack to
        // post-apply so an un-applied policy is redelivered.
        if let Err(e) = msg.ack().await {
            eprintln!("[blue] ack failed: {e}");
        }
    }
    println!("[blue] control membrane ended (reconnect)");
}

async fn pump(
    client: &Client,
    rx: &mut mpsc::Receiver<Outbound>,
    buffer: &mut VecDeque<Outbound>,
    counters: &DropCounters,
    status: &NatsStatus,
) -> PumpExit {
    let mut last_roundtrip = Instant::now();

    loop {
        ingest_ready(rx, buffer, counters);

        if buffer.is_empty() {
            tokio::select! {
                maybe = rx.recv() => match maybe {
                    Some(msg) => push_drop_oldest(buffer, msg, counters),
                    None => return PumpExit::ChannelClosed,
                },
                _ = tokio::time::sleep(LIVENESS_PROBE_INTERVAL) => {
                    match timeout(FLUSH_TIMEOUT, client.flush()).await {
                        Ok(Ok(())) => last_roundtrip = Instant::now(),
                        _ => {
                            if last_roundtrip.elapsed() >= WEDGE_GRACE {
                                return PumpExit::Wedged;
                            }
                        }
                    }
                }
            }
            continue;
        }

        // Take a batch out so concurrent eviction can't misalign removal.
        let n = buffer.len().min(PUBLISH_BATCH);
        let in_flight: Vec<Outbound> = buffer.drain(..n).collect();

        for msg in &in_flight {
            if let Err(e) = client.publish(msg.subject.clone(), msg.payload.clone()).await {
                eprintln!("[nats] publish error: {} — reconnecting in place", e);
                prepend(buffer, in_flight);
                return PumpExit::Wedged;
            }
        }

        // Confirm the batch reached the broker (flush = round-trip). Retry the
        // flush (messages already enqueued), not the publish.
        loop {
            match timeout(FLUSH_TIMEOUT, client.flush()).await {
                Ok(Ok(())) => {
                    last_roundtrip = Instant::now();
                    status
                        .last_publish_ms
                        .store(crate::heartbeat::now_unix_ms(), Ordering::Relaxed);
                    break; // confirmed → in_flight dropped (delivered)
                }
                _ => {
                    if last_roundtrip.elapsed() >= WEDGE_GRACE {
                        prepend(buffer, in_flight);
                        return PumpExit::Wedged;
                    }
                    ingest_ready(rx, buffer, counters);
                    tokio::time::sleep(FLUSH_RETRY).await;
                }
            }
        }
    }
}

/// Drain everything currently waiting in the handoff into the ring buffer
/// without blocking (drop-oldest at capacity).
fn ingest_ready(
    rx: &mut mpsc::Receiver<Outbound>,
    buffer: &mut VecDeque<Outbound>,
    counters: &DropCounters,
) {
    while let Ok(msg) = rx.try_recv() {
        push_drop_oldest(buffer, msg, counters);
    }
}

/// Ring-buffer push: evict (and count) the oldest at capacity. Generic so the
/// drop policy is unit-testable without constructing `Outbound`.
fn push_drop_oldest<T>(buffer: &mut VecDeque<T>, msg: T, counters: &DropCounters) {
    if buffer.len() >= EDGE_BUFFER_CAPACITY {
        buffer.pop_front();
        counters.publish_failed.fetch_add(1, Ordering::Relaxed);
    }
    buffer.push_back(msg);
}

/// Return an unconfirmed batch to the FRONT (preserving order) so it re-sends
/// first after reconnect. Generic for testability.
fn prepend<T>(buffer: &mut VecDeque<T>, in_flight: Vec<T>) {
    for msg in in_flight.into_iter().rev() {
        buffer.push_front(msg);
    }
}

/// Connect, retrying forever with exponential backoff, while still ingesting
/// producer messages into the buffer during the wait. Returns `None` only if
/// the handoff channel closes (clean shutdown).
async fn connect_with_backoff(
    broker_urls: &[String],
    credentials_path: &str,
    rx: &mut mpsc::Receiver<Outbound>,
    buffer: &mut VecDeque<Outbound>,
    counters: &DropCounters,
) -> Option<Client> {
    let mut backoff = RECONNECT_BACKOFF_INITIAL;
    loop {
        match connect_once(broker_urls, credentials_path).await {
            Ok(client) => return Some(client),
            Err(e) => {
                let delay = jittered_delay(backoff);
                eprintln!(
                    "[nats] connect failed: {} — retry in {:?} (buffered={})",
                    e, delay, buffer.len()
                );
                if !sleep_while_ingesting(delay, rx, buffer, counters).await {
                    return None;
                }
                backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
            }
        }
    }
}

// RandomState is seeded by the standard library's OS randomness; no new runtime
// dependency. Equal jitter stays in [base/2, base], including at the 30s cap.
fn jittered_delay(base: Duration) -> Duration {
    use std::hash::BuildHasher;
    use std::sync::atomic::{AtomicU64, Ordering};
    static ATTEMPT: AtomicU64 = AtomicU64::new(0);
    let sample = std::collections::hash_map::RandomState::new().hash_one(ATTEMPT.fetch_add(1,Ordering::Relaxed));
    delay_from_sample(base,sample)
}
fn delay_from_sample(base:Duration,sample:u64)->Duration {
    let ceiling=base.as_millis().min(u64::MAX as u128) as u64;
    let floor=ceiling/2;
    Duration::from_millis(floor + sample % (ceiling-floor+1))
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    #[test]
    fn jitter_varies_and_stays_inside_backoff_cap() {
        for base in [RECONNECT_BACKOFF_INITIAL,RECONNECT_BACKOFF_MAX,Duration::from_secs(60)] {
            for sample in [0,1,500,1000,u64::MAX] {
                let delay=delay_from_sample(base,sample);
                assert!(delay>=base/2 && delay<=base);
            }
            assert_ne!(delay_from_sample(base,0),delay_from_sample(base,1));
        }
    }
}

async fn connect_once(broker_urls: &[String], credentials_path: &str) -> Result<Client, String> {
    let opts = ConnectOptions::with_credentials_file(credentials_path)
        .await
        .map_err(|e| format!("credentials {}: {}", credentials_path, e))?;
    opts.connection_timeout(Duration::from_secs(10))
        .ping_interval(Duration::from_secs(15))
        .connect(broker_urls.join(","))
        .await
        .map_err(|e| format!("connect: {}", e))
}

/// Sleep up to `dur` while draining producer messages into the buffer. Returns
/// `false` if the handoff channel closed.
async fn sleep_while_ingesting(
    dur: Duration,
    rx: &mut mpsc::Receiver<Outbound>,
    buffer: &mut VecDeque<Outbound>,
    counters: &DropCounters,
) -> bool {
    let deadline = Instant::now() + dur;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        let remaining = deadline - now;
        tokio::select! {
            _ = tokio::time::sleep(remaining) => return true,
            maybe = rx.recv() => match maybe {
                Some(msg) => push_drop_oldest(buffer, msg, counters),
                None => return false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_drops_oldest_at_capacity() {
        let counters = DropCounters::new();
        let mut buf: VecDeque<usize> = VecDeque::new();
        for i in 0..EDGE_BUFFER_CAPACITY {
            push_drop_oldest(&mut buf, i, &counters);
        }
        assert_eq!(buf.len(), EDGE_BUFFER_CAPACITY);
        assert_eq!(counters.publish_failed.load(Ordering::Relaxed), 0);
        for i in EDGE_BUFFER_CAPACITY..EDGE_BUFFER_CAPACITY + 5 {
            push_drop_oldest(&mut buf, i, &counters);
        }
        assert_eq!(buf.len(), EDGE_BUFFER_CAPACITY);
        assert_eq!(counters.publish_failed.load(Ordering::Relaxed), 5);
        assert_eq!(*buf.front().unwrap(), 5);
        assert_eq!(*buf.back().unwrap(), EDGE_BUFFER_CAPACITY + 4);
    }

    #[test]
    fn prepend_restores_batch_in_order_at_front() {
        let mut buf: VecDeque<usize> = VecDeque::new();
        buf.push_back(100);
        prepend(&mut buf, vec![1usize, 2, 3]);
        assert_eq!(buf.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3, 100]);
    }
}
