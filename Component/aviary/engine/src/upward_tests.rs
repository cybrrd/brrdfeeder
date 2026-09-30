// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! All brokers/auth/identities below are local TEST ONLY; no JWT minting/signing.
use super::*;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicUsize;

static NEXT: AtomicUsize = AtomicUsize::new(0);

#[test]
fn d44_quarantine_enters_existing_red_lane_and_durable_spool() {
    let root = Scratch::new();
    let path = root.0.join("update_outcome.json");
    // Optional Go-effector receipt proves the cross-language handoff unchanged.
    let raw = if let Ok(input) = std::env::var("D44_OUTCOME") {
        std::fs::read(input).unwrap()
    } else {
        serde_json::to_vec(&serde_json::json!({"schema":"cybrrd.update.outcome.v1","node_id":"brrdg3s1",
            "kind":"quarantined","target":format!("ghcr.io/cybrrd/brrdfeeder@sha256:{}","2".repeat(64)),
            "attempts":2,"observed_unix_ms":1})).unwrap()
    };
    std::fs::write(&path, raw).unwrap();
    let outcome = UpdateOutcome::read(&path,"brrdg3s1").unwrap();
    assert_eq!(outcome.distress(),Some(Distress::UpdateQuarantined));
    assert!(UpdateOutcome::read(&path,"other-node").is_none());
    let handle = Handle {node:"brrdg3s1".into(),nonce:"ephemeral-test-session".into(),sequence:AtomicU64::new(0),
        operational:Channel::new(Lane::Operational),red:Channel::new(Lane::Red),stop:Arc::new(AtomicBool::new(false)),tasks:Vec::new()};
    assert!(handle.update_alarm(&outcome,true));
    let queue = handle.red.queue.lock().unwrap().clone();
    assert_eq!(queue.records.len(),1,"quarantine must emit Red, not just a local log");
    assert_eq!(queue.records[0].message.lane(),Lane::Red);
    assert!(queue.records[0].test_only);
    let (store,_) = Store::open(&root.0,Lane::Red,"brrdg3s1").unwrap();
    Store::save(&store.path,&queue).unwrap();drop(store);
    let (_,recovered) = Store::open(&root.0,Lane::Red,"brrdg3s1").unwrap();
    assert_eq!(recovered.records.len(),1);
    let json = serde_json::to_vec_pretty(&recovered.records[0]).unwrap();
    if let Ok(dir)=std::env::var("D44_EVIDENCE") {std::fs::write(Path::new(&dir).join("red-emission.json"),&json).unwrap();}
    println!("D44 quarantine -> existing Red lane -> durable recovered record; no broker publish");
}
#[test]
fn d44_applied_receipt_checks_back_in_without_forging_running_identity() {
    let root = Scratch::new();
    let mut identity = crate::identity::RunningIdentity::upward_test_fixture();
    let raw = if let Ok(dir) = std::env::var("D44_EVIDENCE") {
        let receipt: serde_json::Value = serde_json::from_slice(&std::fs::read(Path::new(&dir).join("offline-week.json")).unwrap()).unwrap();
        let running = &receipt["running_status"]["heartbeat"];
        // This is the sandbox's measured post-update status, not release metadata.
        identity.image_digest = Some(running["image_digest"].as_str().unwrap().into());
        identity.engine_version = Some(running["engine_version"].as_str().unwrap().into());
        identity.build_seq = Some(running["build_seq"].as_u64().unwrap());
        serde_json::to_vec(&receipt["outcome"]).unwrap()
    } else {
        serde_json::to_vec(&serde_json::json!({"schema":"cybrrd.update.outcome.v1","node_id":"brrdg3s1",
            "kind":"applied","target":format!("ghcr.io/cybrrd/brrdfeeder@{}",identity.image_digest.as_ref().unwrap()),
            "attempts":1,"observed_unix_ms":1})).unwrap()
    };
    let path = root.0.join("update_outcome.json");
    std::fs::write(&path,raw).unwrap();
    let outcome = UpdateOutcome::read(&path,"brrdg3s1").unwrap();
    let handle = Handle {node:"brrdg3s1".into(),nonce:"ephemeral-test-session".into(),sequence:AtomicU64::new(0),
        operational:Channel::new(Lane::Operational),red:Channel::new(Lane::Red),stop:Arc::new(AtomicBool::new(false)),tasks:Vec::new()};
    assert!(handle.operational_update(&identity,Health::observe(true,true,false,false,true),Some(outcome.clone())));
    assert!(handle.update_alarm(&outcome,true));
    assert!(handle.red.queue.lock().unwrap().records.is_empty());
    let queue = handle.operational.queue.lock().unwrap().clone();
    let (store,_) = Store::open(&root.0,Lane::Operational,"brrdg3s1").unwrap();
    Store::save(&store.path,&queue).unwrap();drop(store);
    let (_,recovered) = Store::open(&root.0,Lane::Operational,"brrdg3s1").unwrap();
    if let Message::Operational {image_digest,build_seq,health,update,..} = &recovered.records[0].message {
        assert!(health.healthy);assert_eq!(image_digest,&identity.image_digest);assert_eq!(build_seq,&identity.build_seq);
        assert_eq!(update.as_ref().unwrap(),&outcome);
    } else {panic!("wrong upward lane");}
    if let Ok(dir)=std::env::var("D44_EVIDENCE") {std::fs::write(Path::new(&dir).join("operational-emission.json"),serde_json::to_vec_pretty(&recovered.records[0]).unwrap()).unwrap();}
    assert!(handle.operational_update(&crate::identity::RunningIdentity::unverified(),Health::observe(true,true,false,false,true),Some(outcome)));
    let q = handle.operational.queue.lock().unwrap();
    if let Message::Operational {image_digest,health,..} = &q.records[1].message {
        assert!(image_digest.is_none());assert!(!health.healthy,"applied receipt cannot forge current health");
    } else {panic!("wrong upward lane");}
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("d40-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap(); Self(path)
    }
}
impl Drop for Scratch { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

fn record(id: usize, lane: Lane) -> Record {
    Record { message_id: format!("TEST-ONLY:{id}"), node_id: "test-node".into(), observed_unix_ms: 1,
        clock_trusted: false, test_only: true, message: match lane {
            Lane::Operational => Message::Operational { image_digest: Some(format!("sha256:{}", "a".repeat(64))),
                engine_version: Some("b".repeat(40)), build_seq: Some(42), health: Health::observe(true, true, false, false, false), update: None },
            Lane::Red => Message::Algedonic { code: Distress::OperationalDeliveryFailed, operational: Diagnostics::default(), update: None },
        } }
}

#[test]
fn bounds_shed_oldest_with_red_reserve_and_nonblocking_handoff() {
    let normal = Channel::new(Lane::Operational);
    let red = Channel::new(Lane::Red);
    for i in 0..20 { assert!(normal.enqueue(record(i, Lane::Operational))); }
    let q = normal.queue.lock().unwrap();
    assert_eq!(q.records.len(), 8);
    assert_eq!(q.records.front().unwrap().message_id, "TEST-ONLY:12");
    assert_eq!(q.shed_oldest, 12);
    let start = std::time::Instant::now();
    assert!(!normal.enqueue(record(100, Lane::Operational)));
    assert!(start.elapsed() < Duration::from_millis(100));
    assert_eq!(normal.status.snapshot().rejected_handoff, 1);
    assert!(red.enqueue(record(0, Lane::Red))); // routine lock/fullness cannot block Red
    drop(q);
    for i in 1..40 { assert!(red.enqueue(record(i, Lane::Red))); }
    let q = red.queue.lock().unwrap();
    assert_eq!(q.records.len(), 32);
    assert_eq!(q.records.front().unwrap().message_id, "TEST-ONLY:8");
    assert_eq!(q.records.back().unwrap().message_id, "TEST-ONLY:39");
    assert_eq!(q.shed_oldest, 8);
    println!("D40 bound: routine retained=8 shed_oldest=12 first=12; Red retained=32 shed_oldest=8 first=8 last=39; capture enqueue nonblocking");
}

#[test]
fn payload_subject_and_retry_checks_fail_closed() {
    for node in ["", "other.node", ">", "*", "a b", "a\n"] { assert!(!token(node)); }
    assert!(!token(&"a".repeat(65)));
    assert!(Config { spool_dir: "/tmp/private".into() }.validate("test-node").is_ok());
    assert!(Config { spool_dir: "relative".into() }.validate("test-node").is_err());
    assert_eq!(Lane::Operational.subject("test-node"), "cybrrd.silver.node.operational.test-node");
    assert_eq!(Lane::Red.subject("test-node"), "cybrrd.red.algedonic.test-node");
    assert_ne!(Lane::Operational.stream(), Lane::Red.stream());
    let c = Channel::new(Lane::Operational);
    let mut big = record(1, Lane::Operational); big.message_id = "x".repeat(MAX_PAYLOAD);
    assert!(!c.enqueue(big));
    assert!(!c.enqueue(record(1, Lane::Red)));
    assert_eq!(c.status.snapshot().rejected_payload, 2);
    let delays: Vec<_> = (0..9).map(|i| retry_delay(i).as_secs()).collect();
    assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30, 30, 30]);
}

#[test]
fn durable_recovery_identity_corruption_and_write_failure() {
    let root = Scratch::new();
    let (store, mut q) = Store::open(&root.0, Lane::Operational, "test-node").unwrap();
    q.push(record(7, Lane::Operational), Lane::Operational);
    Store::save(&store.path, &q).unwrap();
    assert!(Store::open(&root.0, Lane::Operational, "test-node").is_err()); // another process/worker
    let path = store.path.clone(); drop(store);
    let (store, recovered) = Store::open(&root.0, Lane::Operational, "test-node").unwrap();
    assert_eq!(recovered.records[0].message_id, "TEST-ONLY:7"); drop(store);
    assert!(Store::open(&root.0, Lane::Operational, "other-node").is_err());
    assert!(Store::save(&root.0.join("missing/queue.json"), &q).is_err());
    std::fs::write(&path, b"not a spool").unwrap();
    assert!(Store::open(&root.0, Lane::Operational, "test-node").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"not a spool");
    let mut excessive = Queue::default(); excessive.records = (0..9).map(|i| record(i, Lane::Operational)).collect();
    std::fs::write(&path, serde_json::to_vec(&excessive).unwrap()).unwrap();
    assert!(Store::open(&root.0, Lane::Operational, "test-node").is_err());
    let mut duplicate = Queue::default(); duplicate.records = vec![record(1, Lane::Operational), record(1, Lane::Operational)].into();
    std::fs::write(&path, serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(Store::open(&root.0, Lane::Operational, "test-node").is_err());
    println!("D40 persistence: same ID recovered; foreign-node/duplicate/corrupt/overbound spools refused; corrupt bytes preserved; write failure observed");
}

#[tokio::test]
async fn health_never_promotes_unknown_identity_or_unready_sensor() {
    for (id, radio, required, gps, clock) in [(false,true,true,true,true), (true,false,true,true,true),
        (true,true,true,false,true), (true,true,true,true,false)] {
        assert!(!Health::observe(id,radio,required,gps,clock).healthy);
    }
    assert!(Health::observe(true,true,true,true,true).healthy);
    let handle = Handle { node: "test-node".into(), nonce: "fixture".into(), sequence: AtomicU64::new(0),
        operational: Channel::new(Lane::Operational), red: Channel::new(Lane::Red),
        stop: Arc::new(AtomicBool::new(false)), tasks: vec![] };
    let good = crate::identity::RunningIdentity::upward_test_fixture();
    assert!(handle.operational(&good, Health::observe(true,true,true,true,true)));
    assert!(handle.operational(&crate::identity::RunningIdentity::unverified(), Health::observe(true,true,true,true,true)));
    let q = handle.operational.queue.lock().unwrap();
    if let Message::Operational { image_digest, engine_version, health, .. } = &q.records[0].message {
        assert!(health.healthy); assert_eq!(image_digest, &good.image_digest); assert_eq!(engine_version, &good.engine_version);
    } else { panic!("wrong schema"); }
    if let Message::Operational { image_digest, health, .. } = &q.records[1].message {
        assert!(!health.healthy); assert!(image_digest.is_none());
    } else { panic!("wrong schema"); }
}

struct Broker { child: Child, root: PathBuf, port: u16, password: String }
impl Broker {
    fn start(root: &Path, port: u16, mode: &str, password: &str) -> Self {
        let allow = match mode { "deny-normal" => vec![Lane::Red.subject("test-node")],
            "deny-all" => vec!["test-only.no-rights".into()],
            _ => vec![Lane::Operational.subject("test-node"), Lane::Red.subject("test-node")] };
        let config = serde_json::json!({"host":"127.0.0.1", "port":port,
            "jetstream":{"store_dir":root.join("js")},
            "authorization":{"users":[{"user":"test-admin","password":password},
              {"user":"test-node","password":password,"permissions":{
                "publish":{"allow":allow},"subscribe":{"allow":["_INBOX.test-node.upward.>"]}}}]}});
        std::fs::write(root.join("broker.json"), serde_json::to_vec(&config).unwrap()).unwrap();
        let log = OpenOptions::new().create(true).append(true).open(root.join("broker.log")).unwrap();
        let child = Command::new(std::env::var("D40_NATS_SERVER").expect("sandbox broker path required"))
            .arg("-c").arg(root.join("broker.json")).stdout(Stdio::null()).stderr(log).spawn().unwrap();
        Self { child, root: root.into(), port, password: password.into() }
    }
    async fn admin(&self) -> jetstream::Context {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(client) = ConnectOptions::new().user_and_password("test-admin".into(), self.password.clone())
                .connect(format!("127.0.0.1:{}",self.port)).await { return jetstream::new(client); }
            assert!(Instant::now() < deadline, "broker never ready"); sleep(Duration::from_millis(50)).await;
        }
    }
    fn stop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
    fn restart(&mut self, mode: &str) {
        self.stop();
        *self = Self::start(&self.root.clone(), self.port, mode, &self.password.clone());
    }
}
impl Drop for Broker { fn drop(&mut self) { self.stop(); } }

async fn streams(js: &jetstream::Context) {
    for lane in [Lane::Operational, Lane::Red] {
        js.get_or_create_stream(jetstream::stream::Config { name: lane.stream().into(),
            subjects: vec![lane.subject("test-node")], storage: jetstream::stream::StorageType::File,
            retention: jetstream::stream::RetentionPolicy::WorkQueue,
            discard: jetstream::stream::DiscardPolicy::New, max_messages: 128,
            max_bytes: 1024 * 1024, max_message_size: MAX_PAYLOAD as i32,
            duplicate_window: Duration::from_secs(120), ..Default::default() }).await.unwrap();
    }
}
fn handle(root: &Path, broker: &Broker, nonce: &str) -> Handle {
    launch(&Config { spool_dir: root.into() }, "test-node", nonce.into(), Connection {
        urls: vec![format!("127.0.0.1:{}",broker.port)], credentials: String::new(),
        test_user: Some(("test-node".into(), broker.password.clone())) })
}
async fn wait_for(mut condition: impl FnMut() -> bool, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !condition() { assert!(Instant::now() < deadline, "condition timed out"); sleep(Duration::from_millis(50)).await; }
}
fn port() -> u16 { std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() }

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated worldport NATS broker; run with --ignored --nocapture"]
async fn broker_permission_offline_restart_and_independent_red() {
    let root = Scratch::new();
    // Ephemeral local password, no NKey/JWT/signature/minter used anywhere.
    let password = std::fs::read_to_string("/proc/sys/kernel/random/uuid").unwrap().trim().to_string();
    let mut broker = Broker::start(&root.0, port(), "deny-normal", &password);
    let admin = broker.admin().await; streams(&admin).await;
    let spool = root.0.join("spool");
    let engine = handle(&spool, &broker, "first-boot");
    let identity = crate::identity::RunningIdentity::upward_test_fixture();
    let normal = engine.operational.clone();
    let red = engine.red.clone();
    let radio = crate::heartbeat::RadioState::new();
    radio.set(crate::heartbeat::RadioStatus::Up);
    let mut gps = crate::sensor::SensorHealth::initializing("TEST-ONLY");
    gps.state = crate::sensor::SensorState::Healthy;
    let cancel = tokio_util::sync::CancellationToken::new();
    let monitoring = tokio::spawn(monitor(engine, identity.clone(), radio,
        Arc::new(arc_swap::ArcSwap::from_pointee(gps)),
        Arc::new(crate::clock_discipline::TimeTrust::new()), false, cancel.clone()));
    let progress = Arc::new(AtomicU64::new(0));
    let progress_copy = progress.clone();
    let capture = tokio::spawn(async move { loop { progress_copy.fetch_add(1, Ordering::Relaxed); sleep(Duration::from_millis(10)).await; } });
    wait_for(|| normal.status.snapshot().failures > 0 || normal.status.snapshot().acknowledged > 0, 12).await;
    assert_eq!(normal.status.snapshot().acknowledged, 0);
    assert_eq!(normal.status.snapshot().pending, 1);
    assert_ne!(normal.status.snapshot().fault, 0);
    // The actual production monitor, not the test, must raise this Red.
    wait_for(|| red.status.snapshot().acknowledged == 1, 12).await;
    assert!(progress.load(Ordering::Relaxed) > 100, "engine executor stalled");
    assert!(normal.status.snapshot().attempts <= 3, "retry spun");
    let original_id = normal.queue.lock().unwrap().records[0].message_id.clone();
    println!("D40 permission kill: routine pending=1 ack=0 failure>0; Red ack=1; independent engine ticker={} attempts={}",
             progress.load(Ordering::Relaxed), normal.status.snapshot().attempts);
    cancel.cancel(); monitoring.await.unwrap();
    broker.restart("allow"); let admin = broker.admin().await;
    let mut engine = handle(&spool, &broker, "second-boot");
    wait_for(|| engine.operational.status.snapshot().acknowledged == 1, 12).await;
    let stream = admin.get_stream(Lane::Operational.stream()).await.unwrap();
    let raw: async_nats::Message = stream.get_raw_message(1).await.unwrap().try_into().unwrap();
    let recovered: Record = serde_json::from_slice(&raw.payload).unwrap();
    assert_eq!(recovered.message_id, original_id); assert!(recovered.test_only);
    // A receiver that was absent at emit time can later bind a durable pull.
    use futures::StreamExt;
    let consumer = stream.get_or_create_consumer("test-receiver", jetstream::consumer::pull::Config {
        durable_name: Some("test-receiver".into()), ack_policy: jetstream::consumer::AckPolicy::Explicit,
        filter_subject: Lane::Operational.subject("test-node"), ..Default::default() }).await.unwrap();
    let mut messages = consumer.messages().await.unwrap();
    let delivered = timeout(Duration::from_secs(5), messages.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(serde_json::from_slice::<Record>(&delivered.payload).unwrap().message_id, original_id);
    delivered.ack().await.unwrap();
    println!("D40 restart: same pending message ID recovered and PubAcked; later durable pull receiver received it");
    // Server offline: both paths bounded and memory producers remain responsive.
    broker.stop();
    assert!(engine.operational(&identity, Health::observe(true,true,true,true,true)));
    assert!(engine.alarm(Distress::RadioUnavailable, true));
    wait_for(|| engine.operational.status.snapshot().pending == 1 && engine.red.status.snapshot().pending == 1, 3).await;
    sleep(Duration::from_secs(7)).await;
    assert_eq!(engine.operational.status.snapshot().pending, 1);
    assert_eq!(engine.red.status.snapshot().pending, 1);
    broker.restart("allow"); let _ = broker.admin().await;
    wait_for(|| engine.operational.status.snapshot().pending == 0 && engine.red.status.snapshot().pending == 0, 20).await;
    println!("D40 offline recovery: both pending records survived and delivered after broker return");
    engine.shutdown().await; capture.abort();
    let denied = std::fs::read_to_string(root.0.join("broker.log")).unwrap();
    for line in denied.lines().filter(|s| s.contains("Violation")) { println!("D40 broker evidence: {line}"); }
    assert!(denied.contains("Publish Violation"), "broker did not record the publish denial");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated worldport NATS broker"]
async fn broker_both_denied_missing_stream_wrong_stream_and_broken_disk() {
    let root = Scratch::new();
    let password = std::fs::read_to_string("/proc/sys/kernel/random/uuid").unwrap().trim().to_string();
    let mut broker = Broker::start(&root.0, port(), "deny-all", &password);
    let admin = broker.admin().await; streams(&admin).await;
    let spool = root.0.join("spool");
    let mut engine = handle(&spool, &broker, "failure-test");
    assert!(engine.operational.enqueue(record(1,Lane::Operational)));
    assert!(engine.red.enqueue(record(2,Lane::Red)));
    wait_for(|| engine.operational.status.snapshot().failures > 0 && engine.red.status.snapshot().failures > 0, 12).await;
    assert_eq!(engine.operational.status.snapshot().pending,1); assert_eq!(engine.red.status.snapshot().pending,1);
    assert_eq!(engine.operational.status.snapshot().acknowledged,0); assert_eq!(engine.red.status.snapshot().acknowledged,0);
    engine.shutdown().await;
    broker.restart("allow"); let admin = broker.admin().await;
    admin.delete_stream(Lane::Operational.stream()).await.unwrap();
    let mut missing = handle(&root.0.join("missing-stream-spool"), &broker, "missing-stream");
    assert!(missing.operational.enqueue(record(9, Lane::Operational)));
    wait_for(|| missing.operational.status.snapshot().failures > 0, 8).await;
    sleep(Duration::from_secs(3)).await;
    assert_eq!(missing.operational.status.snapshot().pending, 1);
    assert!(missing.operational.status.snapshot().attempts <= 3, "no-stream retries spun");
    missing.shutdown().await;
    let connection = Connection { urls:vec![format!("127.0.0.1:{}",broker.port)],credentials:String::new(),test_user:Some(("test-node".into(),password)) };
    let js = connection.connect("test-node", Lane::Operational).await.unwrap();
    assert!(timeout(IO_TIMEOUT, publish(&js,Lane::Operational,&record(10,Lane::Operational))).await.unwrap().is_err());
    admin.create_stream(jetstream::stream::Config { name:"WRONG_STREAM".into(),subjects:vec![Lane::Operational.subject("test-node")],..Default::default() }).await.unwrap();
    assert!(timeout(IO_TIMEOUT, publish(&js,Lane::Operational,&record(11,Lane::Operational))).await.unwrap().is_err());
    admin.delete_stream("WRONG_STREAM").await.unwrap(); streams(&admin).await;
    // Only routine disk is broken; independent Red store/connection must work.
    let broken = root.0.join("broken"); std::fs::create_dir(&broken).unwrap();
    std::fs::create_dir(broken.join("operational.tmp")).unwrap();
    let mut engine = handle(&broken, &broker, "disk-failure");
    assert!(engine.operational.enqueue(record(12,Lane::Operational)));
    assert!(engine.red.enqueue(record(13,Lane::Red)));
    wait_for(|| engine.operational.status.snapshot().fault == 1 && engine.red.status.snapshot().acknowledged == 1, 8).await;
    assert_eq!(engine.operational.status.snapshot().pending,1);
    sleep(Duration::from_secs(3)).await;
    assert!(engine.operational.status.snapshot().failures <= 4, "disk retries spun");
    std::fs::remove_dir(broken.join("operational.tmp")).unwrap();
    wait_for(|| engine.operational.status.snapshot().acknowledged == 1, 12).await;
    engine.shutdown().await;
    // Even Red's own disk failure must not prevent its live escape path.
    let red_broken = root.0.join("red-broken"); std::fs::create_dir(&red_broken).unwrap();
    std::fs::create_dir(red_broken.join("red.tmp")).unwrap();
    let mut engine = handle(&red_broken, &broker, "red-disk-failure");
    assert!(engine.red.enqueue(record(14,Lane::Red)));
    wait_for(|| engine.red.status.snapshot().acknowledged == 1, 8).await;
    assert_eq!(engine.red.status.snapshot().fault, 1);
    assert!(engine.red.status.snapshot().failures > 0);
    engine.shutdown().await;
    println!("D40 failure matrix: both denied retain; missing/wrong stream refuse; routine disk broken retains while Red delivers; disk repair recovers");
    println!("D40 Red disk failure: live PubAck succeeds with explicit durability fault");
}
