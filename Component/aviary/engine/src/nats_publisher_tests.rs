// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Synthetic loopback protocol peer; never contacts a node or production broker.
use super::*;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub(crate) fn fixture_frame(index: u64) -> Vec<u8> {
    use cybrrd_rid_protocol::{astm::parse_message_pack, models::*};
    let mut pack = vec![0u8; 78];
    pack[..3].copy_from_slice(&[0xF2, 25, 3]);
    pack[4] = 0x10;
    pack[5..25].copy_from_slice(b"TEST-ONLY-B06-000001");
    pack[28] = 0x12;
    pack[29] = 0x27;
    pack[33..37].copy_from_slice(&410_000_000_i32.to_le_bytes());
    pack[37..41].copy_from_slice(&(-960_000_000_i32).to_le_bytes());
    pack[43..45].copy_from_slice(&2201_u16.to_le_bytes());
    pack[53] = 0x42;
    pack[55..59].copy_from_slice(&410_000_000_i32.to_le_bytes());
    pack[59..63].copy_from_slice(&(-960_000_000_i32).to_le_bytes());
    pack[71..73].copy_from_slice(&2201_u16.to_le_bytes());
    let mut data = parse_message_pack(&pack, [2, 0, 0, 0, 0, 1], -40).unwrap();
    data.operator_id = Some("TEST-ONLY-OPERATOR".into());
    data.self_id = Some(SelfIdInfo {
        desc_type: 0,
        description: "Synthetic outage test".into(),
    });
    data.wifi_bssid = Some("02:00:00:00:00:02".into());
    data.rx_channel = Some(6);
    data.rx_channel_source = Some(RxChannelSource::Radiotap);
    let node:Node=serde_json::from_value(json!({"id":"test-node","version":"0.9.0-test-only", "location":{"lat":41.0,"lon":-96.0,"alt_m":300.0,"position_source":"gps_live"}})).unwrap();
    serde_json::to_vec(&NormalizedTelemetry {
        wire_format_version: Some(WIRE_FORMAT_VERSION),
        node,
        timestamp_utc: 1_800_000_000_000 + index * 40,
        data,
    })
    .unwrap()
}
struct Fixture {
    root: PathBuf,
    config: Config,
    credentials: String,
    address: std::net::SocketAddr,
    counters: DropCounters,
}
impl Fixture {
    fn new(cap: u64) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::var_os("B06_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "outage-peer-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let credentials = root.join("test-only.creds");
        let key = nkeys::KeyPair::new_user();
        std::fs::write(&credentials,format!("-----BEGIN NATS USER JWT-----\nTEST-ONLY-NOT-A-REAL-JWT\n------END NATS USER JWT------\n\n-----BEGIN USER NKEY SEED-----\n{}\n------END USER NKEY SEED------\n",key.seed().unwrap())).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        Self {
            config: Config {
                directory: root.join("spool"),
                cap_bytes: cap,
            },
            credentials: credentials.to_string_lossy().into_owned(),
            root,
            address,
            counters: DropCounters::new(),
        }
    }
    fn start(&self) -> NatsHandle {
        start(
            vec![format!("nats://{}", self.address)],
            self.credentials.clone(),
            self.counters.clone(),
            None,
            &self.config,
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
struct Peer {
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peer {
    async fn start(address: std::net::SocketAddr, confirm: bool) -> Self {
        let listener = tokio::net::TcpListener::bind(address).await.unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let saved = seen.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    result=listener.accept()=>{
                        let Ok((stream,_))=result else {break;};let seen=saved.clone();
                        connections.spawn(async move {
                            let (rd,mut wr)=stream.into_split();let mut rd=BufReader::new(rd);
                            if wr.write_all(b"INFO {\"server_id\":\"TEST-ONLY-B06\",\"server_name\":\"TEST-ONLY\",\"version\":\"2.11.17\",\"proto\":1,\"host\":\"127.0.0.1\",\"max_payload\":1048576,\"headers\":true}\r\n").await.is_err(){return;}
                            let mut subscriptions=Vec::<(String,String)>::new();
                            loop {
                                let mut line=String::new();if rd.read_line(&mut line).await.unwrap_or(0)==0 {return;}
                                let fields:Vec<_>=line.split_whitespace().collect();
                                match fields.first().copied() {
                                    Some("CONNECT")=>{},
                                    Some("PING")=>{if wr.write_all(b"PONG\r\n").await.is_err(){return;}},
                                    Some("PONG")=>{},
                                    Some("SUB")=>{subscriptions.push((fields[1].into(),fields.last().unwrap().to_string()));},
                                    Some("UNSUB")=>{subscriptions.retain(|(_,sid)|sid!=fields[1]);},
                                    Some("PUB")=>{
                                        let len:usize=fields.last().unwrap().parse().unwrap();let mut raw=vec![0;len+2];
                                        if rd.read_exact(&mut raw).await.is_err(){return;}raw.truncate(len);
                                        if fields[1].starts_with("$JS.API.STREAM.INFO.") {
                                            if !confirm {continue;}
                                            let reply=fields[2];
                                            let Some((_,sid))=subscriptions.iter().find(|(subject,_)|reply.starts_with(subject.trim_end_matches(['>','*']))) else {continue;};
                                            let config=async_nats::jetstream::stream::Config{name:"CYBRRD_CONTROL".into(),..Default::default()};
                                            let body=serde_json::to_vec(&json!({"config":config,"created":"2026-01-01T00:00:00Z","state":{"messages":0,"bytes":0,"first_seq":0,"first_ts":"2026-01-01T00:00:00Z","last_seq":0,"last_ts":"2026-01-01T00:00:00Z","consumer_count":0}})).unwrap();
                                            if wr.write_all(format!("MSG {reply} {sid} {}\r\n",body.len()).as_bytes()).await.is_err(){return;}
                                            if wr.write_all(&body).await.is_err() || wr.write_all(b"\r\n").await.is_err(){return;}
                                        } else {seen.lock().unwrap().push(raw);}
                                    },
                                    _=>panic!("unexpected synthetic NATS operation: {line}"),
                                }
                            }
                        });
                    },
                    _=connections.join_next(), if !connections.is_empty()=>{},
                }
            }
        });
        Self { seen, task }
    }
}
async fn until(mut ready: impl FnMut() -> bool, seconds: u64) {
    let end = tokio::time::Instant::now() + Duration::from_secs(seconds);
    while !ready() {
        assert!(
            tokio::time::Instant::now() < end,
            "synthetic broker condition timed out"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn outage(count: u64, interval_ms: u64) {
    let f = Fixture::new(crate::outage_spool::DEFAULT_CAP);
    let handle = f.start();
    let started = tokio::time::Instant::now();
    for i in 0..count {
        if interval_ms > 0 {
            tokio::time::sleep_until(started + Duration::from_millis(i * interval_ms)).await;
        }
        assert!(
            handle
                .publish(
                    "cybrrd.telemetry.frame.rid.test-node".into(),
                    fixture_frame(i)
                )
                .await,
            "spool refused observation {i}"
        );
        if i > 0 && i % 1500 == 0 {
            println!(
                "OUTAGE accepted={i} elapsed_s={:.1} spool_bytes={}",
                started.elapsed().as_secs_f64(),
                handle.status.spool_bytes.load(Ordering::Relaxed)
            );
        }
    }
    println!(
        "OUTAGE complete accepted={count} elapsed_s={:.3} stored_bytes={} max_append_fsync_us={}",
        started.elapsed().as_secs_f64(),
        handle.status.spool_bytes.load(Ordering::Relaxed),
        handle.status.max_sync_us.load(Ordering::Relaxed)
    );
    assert!(f.counters.loss.lock().unwrap().gaps().is_empty());
    let peer = Peer::start(f.address, true).await;
    // Rotate triggers an immediate reconnect, without modifying credentials.
    assert!(handle.rotate_credentials().await);
    until(
        || handle.status.spool_bytes.load(Ordering::Relaxed) == 0,
        120,
    )
    .await;
    let seen = peer.seen.lock().unwrap();
    assert_eq!(seen.len(), count as usize);
    for (i, raw) in seen.iter().enumerate() {
        assert_eq!(
            *raw,
            fixture_frame(i as u64),
            "payload/order changed at {i}"
        );
    }
    println!(
        "REPLAY PASS accepted={count} received={} byte_identical=true ordered=true dropped=0",
        seen.len()
    );
    assert!(
        handle.status.max_sync_us.load(Ordering::Relaxed) <= 250_000,
        "append/fsync exceeded 250 ms"
    );
}
#[tokio::test]
async fn broker_down_drains_handoff_and_replays_ordered() {
    outage(2500, 0).await;
}
#[tokio::test]
#[ignore = "one hour real-time loopback outage drill; opt in explicitly"]
async fn broker_down_25_per_second_for_60_minutes() {
    outage(90_000, 40).await;
}
#[tokio::test]
async fn full_spool_counts_exact_drops_without_changing_oldest() {
    let f = Fixture::new(1024);
    let handle = f.start();
    assert!(handle.publish("test".into(), vec![1; 900]).await);
    let segment = f.config.directory.join("00000000000000000001.seg");
    let original = std::fs::read(&segment).unwrap();
    for i in 0..137 {
        assert!(
            !handle
                .publish(
                    "test".into(),
                    serde_json::to_vec(&json!({"timestamp_utc":1000+i,"data":"x".repeat(100)}))
                        .unwrap()
                )
                .await
        );
    }
    assert_eq!(std::fs::read(segment).unwrap(), original);
    let ledger = f.counters.loss.lock().unwrap();
    let gap = &ledger.gaps()[0];
    assert_eq!(
        (gap.count, gap.first_capture_ms, gap.last_capture_ms),
        (137, 1000, 1136)
    );
    assert_eq!(
        ledger.pending().iter().map(|g| g.count).collect::<Vec<_>>(),
        vec![1, 10, 100]
    );
}
#[tokio::test]
async fn socket_flush_without_broker_barrier_keeps_saved_batch() {
    let f = Fixture::new(1024 * 1024);
    let handle = f.start();
    for i in 0..10 {
        assert!(handle.publish("test".into(), fixture_frame(i)).await);
    }
    let before = handle.status.spool_bytes.load(Ordering::Relaxed);
    let peer = Peer::start(f.address, false).await;
    assert!(handle.rotate_credentials().await);
    until(|| peer.seen.lock().unwrap().len() == 10, 10).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        handle.status.spool_bytes.load(Ordering::Relaxed),
        before,
        "local flush falsely retired custody"
    );
    drop(peer);
}

#[tokio::test]
#[ignore = "subprocess fixture for SIGKILL test; requires explicit private paths"]
async fn crash_writer_child() {
    let directory = PathBuf::from(std::env::var_os("B06_CHILD_SPOOL").expect("child spool"));
    let credentials = std::env::var("B06_CHILD_CREDS").unwrap();
    let url = std::env::var("B06_CHILD_URL").unwrap();
    assert!(url.starts_with("nats://127.0.0.1:"));
    let ready = PathBuf::from(std::env::var_os("B06_CHILD_READY").unwrap());
    let handle = start(
        vec![url],
        credentials,
        DropCounters::new(),
        None,
        &Config {
            directory,
            cap_bytes: 1024 * 1024 * 16,
        },
    )
    .unwrap();
    for i in 0..1000 {
        assert!(
            handle
                .publish(
                    "cybrrd.telemetry.frame.rid.test-node".into(),
                    fixture_frame(i)
                )
                .await
        );
    }
    std::fs::write(&ready, b"1000").unwrap();
    std::fs::File::open(&ready).unwrap().sync_all().unwrap();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
#[tokio::test]
async fn sigkill_mid_outage_restarts_and_replays_identical_set() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};
    let f = Fixture::new(16 * 1024 * 1024);
    let ready = f.root.join("ready");
    let log = std::fs::File::create(f.root.join("child.log")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "nats_publisher::tests::crash_writer_child",
            "--ignored",
            "--nocapture",
        ])
        .env("B06_CHILD_SPOOL", &f.config.directory)
        .env("B06_CHILD_CREDS", &f.credentials)
        .env("B06_CHILD_URL", format!("nats://{}", f.address))
        .env("B06_CHILD_READY", &ready)
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !ready.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "crash fixture exited early"
        );
        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("crash fixture stalled");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(std::fs::read(&ready).unwrap(), b"1000");
    child.kill().unwrap();
    assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
    let peer = Peer::start(f.address, true).await;
    let handle = f.start();
    until(
        || handle.status.spool_bytes.load(Ordering::Relaxed) == 0,
        30,
    )
    .await;
    let seen = peer.seen.lock().unwrap();
    assert_eq!(seen.len(), 1000);
    for (i, raw) in seen.iter().enumerate() {
        assert_eq!(*raw, fixture_frame(i as u64));
    }
    println!("SIGKILL PASS durable accepted=1000; restart replay=1000; identical ordered set");
}

#[tokio::test]
async fn full_producer_handoff_backpressures_without_drop() {
    let counters = DropCounters::new();
    let (tx, mut rx) = mpsc::channel(1);
    let (rotation, _) = watch::channel(0);
    let (_, rotated) = watch::channel(0);
    let (accepted, _) = oneshot::channel();
    assert!(tx
        .try_send(Ingest {
            message: Outbound::new("first".into(), vec![]),
            retained_by_caller: false,
            accepted,
        })
        .is_ok());
    let handle = NatsHandle {
        rotation,
        rotated,
        tx,
        counters: counters.clone(),
        status: Arc::new(NatsStatus::default()),
    };
    let pending = tokio::spawn(async move { handle.publish("second".into(), vec![2]).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !pending.is_finished(),
        "full handoff dropped instead of waiting"
    );
    assert_eq!(counters.publish_failed.load(Ordering::Relaxed), 0);
    assert!(counters.loss.lock().unwrap().gaps().is_empty());
    assert_eq!(rx.recv().await.unwrap().message.subject, "first");
    let second = rx.recv().await.unwrap();
    assert_eq!(second.message.subject, "second");
    second.accepted.send(true).unwrap();
    assert!(pending.await.unwrap());
}

#[tokio::test]
#[ignore = "requires the pinned local NATS fixture; run engine/tests/run-broker-tests.py"]
async fn broker_real_stream_info_confirms_ordered_batch() {
    use futures::StreamExt;
    struct Broker(std::process::Child);
    impl Drop for Broker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let f = Fixture::new(1024 * 1024);
    let config = f.root.join("broker.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({
            "host":"127.0.0.1", "port":f.address.port(),
            "jetstream":{"store_dir":f.root.join("jetstream")}
        }))
        .unwrap(),
    )
    .unwrap();
    let _broker = Broker(
        std::process::Command::new(std::env::var("D40_NATS_SERVER").unwrap())
            .arg("-c")
            .arg(config)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let client = timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(c) = async_nats::connect(format!("nats://{}", f.address)).await {
                break c;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        broker_barrier(&client, None).await.is_err(),
        "missing control stream falsely confirmed"
    );
    async_nats::jetstream::new(client.clone())
        .create_stream(async_nats::jetstream::stream::Config {
            name: "CYBRRD_CONTROL".into(),
            subjects: vec!["test.control.>".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    let mut received = client.subscribe("test.telemetry").await.unwrap();
    broker_barrier(&client, None).await.unwrap();
    let mut disk = Spool::open(&f.config).unwrap();
    for i in 0..1000 {
        assert!(disk
            .append(&Outbound::new("test.telemetry".into(), fixture_frame(i)))
            .unwrap());
    }
    let disk = Arc::new(Mutex::new(disk));
    let status = Arc::new(NatsStatus::default());
    status
        .spool_bytes
        .store(disk.lock().unwrap().bytes(), Ordering::Relaxed);
    let (rotation, mut changed) = watch::channel(0);
    let (task_disk, task_status) = (disk.clone(), status.clone());
    let task = tokio::spawn(async move {
        pump(
            &client,
            &task_disk,
            &Notify::new(),
            &task_status,
            &mut changed,
            None,
        )
        .await
    });
    for i in 0..1000 {
        let message = timeout(Duration::from_secs(10), received.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(message.payload.as_ref(), fixture_frame(i));
    }
    until(|| status.spool_bytes.load(Ordering::Relaxed) == 0, 10).await;
    rotation.send(1).unwrap();
    assert!(matches!(task.await.unwrap(), PumpExit::Rotate));
    assert!(disk.lock().unwrap().peek(1).unwrap().messages.is_empty());
    println!(
        "REAL BROKER PASS: 1000 byte-identical ordered records, retired after stream-info reply"
    );
}
