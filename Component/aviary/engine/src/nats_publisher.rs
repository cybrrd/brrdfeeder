// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! One supervised connection with disk custody independent of network waits.
//! Producers await a bounded disk-writer handoff; network reconnect, publish,
//! and flush never stop the writer. Every accepted message is fsynced before
//! acceptance. A replay batch remains on disk until the broker barrier passes.
use crate::audit::DropCounters;
use crate::outage_spool::{Config, Spool};
use crate::telemetry_loss::{Boundary, Ledger};
use async_nats::{Client, ConnectOptions};
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch, Notify};
use tokio::time::timeout;

const PUBLISH_BATCH: usize = 256;
const LIVENESS_PROBE_INTERVAL: Duration = Duration::from_secs(15);
const FLUSH_TIMEOUT: Duration = Duration::from_secs(8);
const RECONNECT_BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(30);
const HANDOFF_CAPACITY: usize = 1024;

#[derive(Clone, Debug)]
pub struct Outbound {
    pub subject: String,
    pub payload: Bytes,
    pub capture_ms: u64,
}
impl Outbound {
    fn new(subject: String, payload: Vec<u8>) -> Self {
        // Transport metadata only; the original v4 bytes are never rewritten.
        let capture_ms = serde_json::from_slice::<serde_json::Value>(&payload)
            .ok()
            .and_then(|v| v.get("timestamp_utc").and_then(|n| n.as_u64()))
            .unwrap_or_else(crate::heartbeat::now_unix_ms);
        Self {
            subject,
            payload: payload.into(),
            capture_ms,
        }
    }
}
struct Ingest {
    message: Outbound,
    retained_by_caller: bool,
    accepted: oneshot::Sender<bool>,
}
#[derive(Clone)]
pub struct NatsHandle {
    rotation: watch::Sender<u64>,
    rotated: watch::Receiver<u64>,
    tx: mpsc::Sender<Ingest>,
    counters: DropCounters,
    pub status: Arc<NatsStatus>,
}
#[derive(Default)]
pub struct NatsStatus {
    client: arc_swap::ArcSwapOption<Client>,
    pub last_publish_ms: AtomicU64,
    pub spool_bytes: AtomicU64,
    pub spool_fault: AtomicBool,
    pub max_sync_us: AtomicU64,
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
    pub async fn rotate_credentials(&self) -> bool {
        self.rotation.send_modify(|n| *n = n.saturating_add(1));
        let generation = *self.rotation.borrow();
        crate::credential_renewal::signal_upward_rotation();
        let mut confirmed = self.rotated.clone();
        let ok = matches!(
            timeout(
                Duration::from_secs(45),
                confirmed.wait_for(|n| *n >= generation)
            )
            .await,
            Ok(Ok(_))
        );
        ok
    }
    pub async fn publish(&self, subject: String, payload: Vec<u8>) -> bool {
        self.enqueue(subject, payload, false).await
    }
    /// Audit retries retain their own window. A refused attempt is not another
    /// dropped observation and must not recursively create more audit loss.
    pub async fn publish_retained(&self, subject: String, payload: Vec<u8>) -> bool {
        self.enqueue(subject, payload, true).await
    }
    async fn enqueue(&self, subject: String, payload: Vec<u8>, retained_by_caller: bool) -> bool {
        let message = Outbound::new(subject, payload);
        let capture_ms = message.capture_ms;
        let (accepted, receipt) = oneshot::channel();
        if self
            .tx
            .send(Ingest {
                message,
                retained_by_caller,
                accepted,
            })
            .await
            .is_err()
        {
            if !retained_by_caller {
                self.counters.publish_failed.fetch_add(1, Ordering::Relaxed);
                self.counters
                    .record_loss(Boundary::ProducerHandoff, capture_ms);
            }
            return false;
        }
        receipt.await.unwrap_or(false)
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

/// Open durable state before capture starts. A broken/locked store fails loud;
/// no implicit RAM-only fallback can silently discard the only saved copy.
pub fn start(
    broker_urls: Vec<String>,
    credentials_path: String,
    counters: DropCounters,
    control: Option<ControlConfig>,
    config: &Config,
) -> std::io::Result<NatsHandle> {
    let spool = Spool::open(config)?;
    *counters.loss.lock().unwrap_or_else(|e| e.into_inner()) = Ledger::open(&config.directory)?;
    counters
        .loss
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .storage_fault = spool.corrupt;
    let status = Arc::new(NatsStatus::default());
    status.spool_bytes.store(spool.bytes(), Ordering::Relaxed);
    status.spool_fault.store(spool.corrupt, Ordering::Relaxed);
    let spool = Arc::new(Mutex::new(spool));
    let wake = Arc::new(Notify::new());
    let (tx, mut rx) = mpsc::channel::<Ingest>(HANDOFF_CAPACITY);
    let (rotation, rotation_rx) = watch::channel(0u64);
    let (rotated_tx, rotated) = watch::channel(0u64);
    let handle = NatsHandle {
        rotation,
        rotated,
        tx,
        counters: counters.clone(),
        status: status.clone(),
    };
    let (disk, signal, stats) = (spool.clone(), wake.clone(), status.clone());
    // Dedicated bounded writer continues through DNS, connect, flush, and
    // broker timeouts. No file syscall runs on a Tokio reactor thread.
    std::thread::Builder::new()
        .name("outage-spool".into())
        .spawn(move || {
            while let Some(request) = rx.blocking_recv() {
                let mut spool = disk.lock().unwrap_or_else(|e| e.into_inner());
                let result = spool.append(&request.message);
                let accepted = matches!(result, Ok(true));
                stats.spool_bytes.store(spool.bytes(), Ordering::Relaxed);
                stats
                    .max_sync_us
                    .store(spool.max_sync_us, Ordering::Relaxed);
                stats
                    .spool_fault
                    .store(spool.corrupt || result.is_err(), Ordering::Relaxed);
                counters
                    .loss
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .storage_fault = spool.corrupt || result.is_err();
                if accepted {
                    counters.resumed(Boundary::OutageSpool);
                }
                if !accepted && !request.retained_by_caller {
                    counters.publish_failed.fetch_add(1, Ordering::Relaxed);
                    counters.record_loss(Boundary::OutageSpool, request.message.capture_ms);
                }
                if let Err(e) = result {
                    eprintln!("[nats] spool append failed; saved records retained: {e}");
                }
                drop(spool);
                let _ = request.accepted.send(accepted);
                signal.notify_one();
            }
        })?;
    tokio::spawn(supervisor_loop(
        spool,
        wake,
        broker_urls,
        credentials_path,
        control,
        status,
        rotation_rx,
        rotated_tx,
    ));
    Ok(handle)
}
enum PumpExit {
    Rotate,
    Wedged,
}
async fn supervisor_loop(
    spool: Arc<Mutex<Spool>>,
    wake: Arc<Notify>,
    broker_urls: Vec<String>,
    credentials_path: String,
    control: Option<ControlConfig>,
    status: Arc<NatsStatus>,
    mut rotation: watch::Receiver<u64>,
    rotated: watch::Sender<u64>,
) {
    let mut backoff = RECONNECT_BACKOFF_INITIAL;
    loop {
        if rotation.has_changed().is_err() {
            return;
        }
        let generation = *rotation.borrow_and_update();
        let result = tokio::select! {
            result=connect_once(&broker_urls,&credentials_path)=>result,
            _=rotation.changed()=>continue,
        };
        let client = match result {
            Ok(c) => {
                backoff = RECONNECT_BACKOFF_INITIAL;
                c
            }
            Err(e) => {
                eprintln!("[nats] connect failed: {e}; disk writer remains active");
                tokio::select! { _=tokio::time::sleep(jittered_delay(backoff))=>{}, _=rotation.changed()=>{} }
                backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
                continue;
            }
        };
        status.client.store(Some(Arc::new(client.clone())));
        rotated.send_replace(generation);
        let consumer = control
            .as_ref()
            .map(|ctrl| tokio::spawn(control_consumer(client.clone(), ctrl.clone())));
        let outcome = pump(
            &client,
            &spool,
            &wake,
            &status,
            &mut rotation,
            control.as_ref(),
        )
        .await;
        if let Some(task) = consumer {
            task.abort();
        }
        status.client.store(None);
        drop(client);
        if matches!(outcome, PumpExit::Wedged) {
            tokio::time::sleep(jittered_delay(RECONNECT_BACKOFF_INITIAL)).await;
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
/// membrane permanently idle — the #185 hummingbird/test-node-3 bind-fragility.)
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
    // hummingbird at 60ms RTT, and on test-node-3 over LTE). Extend it.
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

async fn broker_barrier(client: &Client, control: Option<&ControlConfig>) -> Result<(), String> {
    client.flush().await.map_err(|e| e.to_string())?;
    // async-nats 0.50 flush only drains its local socket buffer. The existing
    // Blue stream-info permission provides an ordered broker round trip on
    // THIS connection. The caller checks the connection generation: a new
    // socket cannot confirm messages from an old one. This confirms broker receipt,
    // not permanent lake custody (v5's separate contract).
    let stream = control
        .map(|c| c.stream.as_str())
        .unwrap_or("CYBRRD_CONTROL");
    async_nats::jetstream::new(client.clone())
        .get_stream(stream)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
async fn pump(
    client: &Client,
    spool: &Arc<Mutex<Spool>>,
    wake: &Notify,
    status: &NatsStatus,
    rotation: &mut watch::Receiver<u64>,
    control: Option<&ControlConfig>,
) -> PumpExit {
    loop {
        if rotation.has_changed().unwrap_or(false) {
            return PumpExit::Rotate;
        }
        let disk = spool.clone();
        let batch = match tokio::task::spawn_blocking(move || {
            disk.lock()
                .unwrap_or_else(|e| e.into_inner())
                .peek(PUBLISH_BATCH)
        })
        .await
        {
            Ok(Ok(b)) => b,
            _ => {
                status.spool_fault.store(true, Ordering::Relaxed);
                return PumpExit::Wedged;
            }
        };
        if batch.messages.is_empty() {
            tokio::select! {
                _=rotation.changed()=>return PumpExit::Rotate,
                _=wake.notified()=>{},
                _=tokio::time::sleep(LIVENESS_PROBE_INTERVAL)=>{
                    if !matches!(timeout(FLUSH_TIMEOUT,broker_barrier(client,control)).await,Ok(Ok(()))) {return PumpExit::Wedged;}
                }
            }
            continue;
        }
        let generation = client.statistics().connects.load(Ordering::Acquire);
        let publish = async {
            for msg in &batch.messages {
                client
                    .publish(msg.subject.clone(), msg.payload.clone())
                    .await
                    .map_err(|e| e.to_string())?;
            }
            broker_barrier(client, control).await
        };
        let confirmed = tokio::select! {
            result=timeout(FLUSH_TIMEOUT,publish)=>match result {
                Ok(Ok(()))=>true,
                Ok(Err(e))=>{eprintln!("[nats] broker confirmation failed; batch retained: {e}");false},
                Err(_)=>{eprintln!("[nats] broker confirmation timed out; batch retained");false},
            },
            _=rotation.changed()=>return PumpExit::Rotate,
        };
        if !confirmed || generation != client.statistics().connects.load(Ordering::Acquire) {
            return PumpExit::Wedged;
        }
        let disk = spool.clone();
        match tokio::task::spawn_blocking(move || {
            let mut spool = disk.lock().unwrap_or_else(|e| e.into_inner());
            spool.commit(&batch)?;
            Ok::<_, std::io::Error>(spool.bytes())
        })
        .await
        {
            Ok(Ok(bytes)) => {
                status.spool_bytes.store(bytes, Ordering::Relaxed);
                status
                    .last_publish_ms
                    .store(crate::heartbeat::now_unix_ms(), Ordering::Relaxed);
            }
            _ => {
                status.spool_fault.store(true, Ordering::Relaxed);
                return PumpExit::Wedged;
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
    let sample = std::collections::hash_map::RandomState::new()
        .hash_one(ATTEMPT.fetch_add(1, Ordering::Relaxed));
    delay_from_sample(base, sample)
}
fn delay_from_sample(base: Duration, sample: u64) -> Duration {
    let ceiling = base.as_millis().min(u64::MAX as u128) as u64;
    let floor = ceiling / 2;
    Duration::from_millis(floor + sample % (ceiling - floor + 1))
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    #[test]
    fn jitter_varies_and_stays_inside_backoff_cap() {
        for base in [
            RECONNECT_BACKOFF_INITIAL,
            RECONNECT_BACKOFF_MAX,
            Duration::from_secs(60),
        ] {
            for sample in [0, 1, 500, 1000, u64::MAX] {
                let delay = delay_from_sample(base, sample);
                assert!(delay >= base / 2 && delay <= base);
            }
            assert_ne!(delay_from_sample(base, 0), delay_from_sample(base, 1));
        }
    }
}

async fn connect_once(broker_urls: &[String], credentials_path: &str) -> Result<Client, String> {
    let opts = ConnectOptions::with_credentials_file(credentials_path)
        .await
        .map_err(|e| format!("credentials {}: {}", credentials_path, e))?;
    opts.max_reconnects(1)
        .connection_timeout(Duration::from_secs(10))
        .ping_interval(Duration::from_secs(15))
        .connect(broker_urls.join(","))
        .await
        .map_err(|e| format!("connect: {}", e))
}

#[cfg(test)]
#[path = "nats_publisher_tests.rs"]
pub(crate) mod tests;
