// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use super::*;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p =
            std::env::temp_dir().join(format!("renew-test-{}", KeyPair::new_user().public_key()));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn token(signer: &KeyPair, value: &Value) -> String {
    let body = format!(
        "{}.{}",
        B64.encode(br#"{"typ":"JWT","alg":"ed25519-nkey"}"#),
        B64.encode(serde_json::to_vec(value).unwrap())
    );
    format!(
        "{}.{}",
        body,
        B64.encode(signer.sign(body.as_bytes()).unwrap())
    )
}
fn claims(user: &KeyPair, signer: &KeyPair, account: &KeyPair, utc: i64) -> Value {
    json!({"sub":user.public_key(),"name":"lab-001","iss":signer.public_key(),"iat":utc,"exp":utc+90*DAY,"nats":{"type":"user","version":2,"issuer_account":account.public_key()}})
}
fn armor(jwt: &str, key: &KeyPair) -> String {
    format!("-----BEGIN NATS USER JWT-----\n{jwt}\n------END NATS USER JWT------\n\n-----BEGIN USER NKEY SEED-----\n{}\n------END USER NKEY SEED------\n",key.seed().unwrap())
}
struct Fixture {
    utc: i64,
    _root: Scratch,
    renewal: Renewal,
    signer: KeyPair,
    new: KeyPair,
    old: Claims,
    raw: String,
    value: Value,
}
impl Fixture {
    fn new() -> Self {
        // One clock value binds fixture construction, validation and commit.
        let utc = now();
        let root = Scratch::new();
        let signer = KeyPair::new_account();
        let account = KeyPair::new_account();
        let old_key = KeyPair::new_user();
        let new = KeyPair::new_user();
        let mut old_value = claims(&old_key, &signer, &account, utc - 89 * DAY);
        old_value["exp"] = json!(utc + DAY);
        let raw = armor(&token(&signer, &old_value), &old_key);
        let old = credentials(&raw, None, "lab-001").unwrap().1;
        let renewal = Renewal {
            path: root.0.join("active.creds"),
            node: "lab-001".into(),
            trust: Trust {
                account: account.public_key(),
                issuer: signer.public_key(),
            },
        };
        atomic_write(&renewal.path, raw.as_bytes()).unwrap();
        let value = claims(&new, &signer, &account, utc);
        Self {
            utc,
            _root: root,
            renewal,
            signer,
            new,
            old,
            raw,
            value,
        }
    }
    fn response(&self, value: &Value) -> Response {
        Response {
            node_id: "lab-001".into(),
            public_key: value["sub"].as_str().unwrap().into(),
            expires_at: value["exp"].as_i64().unwrap_or(0),
            superseded_key: self.old.sub.clone(),
            jwt: token(&self.signer, value),
        }
    }
    fn commit(&self, res: &Response, trusted: bool) -> Result<i64> {
        self.renewal
            .commit_response(res, &self.new, &self.old, &self.raw, self.utc, trusted)
    }
}

#[test]
fn verified_atomic_swap_and_permissions() {
    let f = Fixture::new();
    let res = f.response(&f.value);
    assert!(f.commit(&res, true).is_ok());
    let raw = read_private(&f.renewal.path).unwrap();
    let (key, c) = credentials(&raw, Some(&f.renewal.trust), "lab-001").unwrap();
    assert_eq!(key.public_key(), f.new.public_key());
    assert_eq!(c.exp, res.expires_at);
    assert_eq!(fs::metadata(&f.renewal.path).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        fs::read_to_string(f._root.0.join("previous.creds")).unwrap(),
        f.raw
    );
}

#[test]
fn iat_skew_boundary_uses_one_clock_for_validation_and_commit() {
    for (skew, accepted) in [(301, false), (300, true), (-301, false), (-300, true)] {
        let f = Fixture::new();
        let mut value = f.value.clone();
        value["iat"] = json!(f.utc + skew);
        // Keep the lifetime valid so only the exact clock-skew boundary decides.
        value["exp"] = json!(f.utc + skew + 90 * DAY);
        let res = f.response(&value);
        let validation = validate_response(
            &res, &f.new, &f.old, &f.renewal.trust, "lab-001", f.utc, true,
        );
        let commit = f.commit(&res, true);
        if accepted {
            assert!(validation.is_ok(), "skew {skew}: validation rejected boundary");
            assert_eq!(commit.unwrap(), res.expires_at);
            let (_, installed) = credentials(
                &read_private(&f.renewal.path).unwrap(), Some(&f.renewal.trust), "lab-001",
            ).unwrap();
            assert_eq!(installed.sub, f.new.public_key());
            assert_eq!(fs::read_to_string(f._root.0.join("previous.creds")).unwrap(), f.raw);
        } else {
            assert_eq!(validation.err().as_deref(), Some("response_time_invalid"));
            // The commit interface deliberately maps validation errors.
            assert_eq!(commit.unwrap_err(), "verification_failed");
            assert_eq!(fs::read_to_string(&f.renewal.path).unwrap(), f.raw);
            assert!(!f._root.0.join("previous.creds").exists());
        }
    }
}

// This synthetic wire peer only implements the NATS handshake/PUB/PING pieces
// used by the real async-nats client. It is not a replacement for the native
// pinned-broker canary gate. It verifies key possession on every connection.
struct Peer {
    url: String,
    seen: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    connections: Arc<AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peer {
    async fn start(trust: Trust) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("nats://127.0.0.1:{port}");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicU64::new(0));
        let (s, c) = (seen.clone(), connections.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (s, c, trust) = (s.clone(), c.clone(), trust.clone());
                tokio::spawn(async move {
                    let (rd, mut wr) = stream.into_split();
                    let mut rd = BufReader::new(rd);
                    wr.write_all(format!("INFO {{\"server_id\":\"TEST-ONLY\",\"server_name\":\"TEST-ONLY\",\"version\":\"2.11.17\",\"proto\":1,\"host\":\"127.0.0.1\",\"port\":{port},\"auth_required\":true,\"nonce\":\"fixture\",\"max_payload\":1048576}}\r\n").as_bytes()).await.unwrap();
                    let mut subject = String::new();
                    loop {
                        let mut line = String::new();
                        if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                            break;
                        }
                        if let Some(raw) = line.strip_prefix("CONNECT ") {
                            let v: Value = serde_json::from_str(raw.trim()).unwrap();
                            let jwt = v["jwt"].as_str().unwrap();
                            subject = decode(jwt, Some(&trust), "lab-001").unwrap().sub;
                            KeyPair::from_public_key(&subject)
                                .unwrap()
                                .verify(
                                    b"fixture",
                                    &B64.decode(v["sig"].as_str().unwrap()).unwrap(),
                                )
                                .unwrap();
                            c.fetch_add(1, Ordering::SeqCst);
                        } else if line == "PING\r\n" {
                            if wr.write_all(b"PONG\r\n").await.is_err() {
                                break;
                            }
                        } else if line.starts_with("PUB ") {
                            let len = line
                                .split_whitespace()
                                .last()
                                .unwrap()
                                .parse::<usize>()
                                .unwrap();
                            let mut raw = vec![0; len + 2];
                            rd.read_exact(&mut raw).await.unwrap();
                            raw.truncate(len);
                            s.lock().unwrap().push((subject.clone(), raw));
                        }
                    }
                });
            }
        });
        Self {
            url,
            seen,
            connections,
            task,
        }
    }
}
async fn until(mut check: impl FnMut() -> bool) {
    let end = tokio::time::Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(
            tokio::time::Instant::now() < end,
            "synthetic peer condition timed out"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn rejected_jwts_leave_file_and_live_connection_unchanged() {
    let mut f = Fixture::new();
    let original_exp = f.old.exp;
    let peer = Peer::start(f.renewal.trust.clone()).await;
    let handle = crate::nats_publisher::start(
        vec![peer.url.clone()],
        f.renewal.path().into(),
        crate::audit::DropCounters::new(),
        None,
    );
    until(|| {
        peer.connections.load(Ordering::SeqCst) == 1
            && handle.status.connection_state() == "connected"
    })
    .await;
    for mode in [
        "subject",
        "issuer",
        "account",
        "signature",
        "expired",
        "missing_expiry",
        "nonadvancing",
        "lifetime",
        "stale_iat",
        "future_iat",
        "nbf",
        "wrong_name",
        "metadata",
        "clock",
        "malformed",
    ] {
        if std::env::var("RENEW_NEGATIVE")
            .ok()
            .is_some_and(|v| v != mode)
        {
            continue;
        }
        f.old.exp = original_exp;
        let mut v = f.value.clone();
        let rogue = KeyPair::new_account();
        match mode {
            "subject" => v["sub"] = json!(KeyPair::new_user().public_key()),
            "issuer" => v["iss"] = json!(rogue.public_key()),
            "account" => v["nats"]["issuer_account"] = json!(rogue.public_key()),
            "expired" => v["exp"] = json!(f.utc - 1),
            "missing_expiry" => {
                v.as_object_mut().unwrap().remove("exp");
            }
            "nonadvancing" => {
                f.old.exp = f.utc + 89 * DAY;
                v["exp"] = json!(f.utc + 60 * DAY);
            }
            "lifetime" => v["exp"] = json!(f.utc + 91 * DAY),
            "stale_iat" => v["iat"] = json!(f.utc - 301),
            "future_iat" => v["iat"] = json!(f.utc + 301),
            "nbf" => v["nbf"] = json!(f.utc + 300),
            "wrong_name" => v["name"] = json!("other-001"),
            _ => {}
        }
        let mut res = f.response(&v);
        if mode == "issuer" {
            res.jwt = token(&rogue, &v);
        }
        if mode == "signature" {
            let mut parts: Vec<String> = res.jwt.split('.').map(String::from).collect();
            let mut sig = B64.decode(&parts[2]).unwrap();
            sig[0] ^= 1;
            parts[2] = B64.encode(sig);
            res.jwt = parts.join(".");
        }
        if mode == "metadata" {
            res.node_id = "other-001".into();
        }
        if mode == "malformed" {
            res.jwt = "not.jwt".into();
        }
        if mode == "missing_expiry" {
            assert!(
                decode(&res.jwt, Some(&f.renewal.trust), "lab-001").is_err(),
                "missing expiry accepted by decoder"
            );
        }
        assert!(
            validate_response(
                &res,
                &f.new,
                &f.old,
                &f.renewal.trust,
                "lab-001",
                f.utc,
                mode != "clock"
            )
            .is_err(),
            "negative guard {mode} accepted by validator"
        );
        assert!(
            f.commit(&res, mode != "clock").is_err(),
            "negative guard {mode} accepted"
        );
        assert_eq!(
            fs::read_to_string(&f.renewal.path).unwrap(),
            f.raw,
            "active changed: {mode}"
        );
        assert!(!f._root.0.join("previous.creds").exists());
        handle.publish("test.frames".into(), mode.as_bytes().to_vec());
        let count = peer.seen.lock().unwrap().len();
        until(|| peer.seen.lock().unwrap().len() > count).await;
        assert_eq!(
            peer.connections.load(Ordering::SeqCst),
            1,
            "connection changed: {mode}"
        );
        assert!(peer
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|(key, _)| key == &f.old.sub));
    }
    f.old.exp = original_exp;
    f.commit(&f.response(&f.value), true).unwrap();
    assert!(handle.rotate_credentials().await);
    handle.publish("test.frames".into(), b"rotated".to_vec());
    until(|| {
        peer.seen
            .lock()
            .unwrap()
            .iter()
            .any(|(key, raw)| key == &f.new.public_key() && raw == b"rotated")
    })
    .await;
    assert_eq!(peer.connections.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rotation_rereads_file_and_retains_offline_queue_in_order() {
    let f = Fixture::new();
    // Force the real supervisor into reconnect/backoff while frames queue.
    fs::remove_file(&f.renewal.path).unwrap();
    let peer = Peer::start(f.renewal.trust.clone()).await;
    let handle = crate::nats_publisher::start(
        vec![peer.url.clone()],
        f.renewal.path(),
        crate::audit::DropCounters::new(),
        None,
    );
    for i in 0..9 {
        handle.publish("test.frames".into(), vec![i]);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
    f.commit(&f.response(&f.value), true).unwrap();
    assert!(
        handle.rotate_credentials().await,
        "reconnect never acknowledged"
    );
    until(|| {
        peer.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == &f.new.public_key())
            .count()
            == 9
    })
    .await;
    let frames: Vec<_> = peer
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(k, _)| k == &f.new.public_key())
        .map(|(_, v)| v[0])
        .collect();
    assert_eq!(frames, (0..9).collect::<Vec<_>>());
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn untrusted_clock_pauses_without_network_then_recovers() {
    let f = Fixture::new();
    let clock = crate::clock_discipline::TimeTrust::new();
    let client = reqwest::Client::new();
    assert_eq!(
        f.renewal.attempt(&client, &clock).await.unwrap_err(),
        "clock_untrusted"
    );
    let policy = crate::node_config::ClockYaml {
        consistent_fixes: 1,
        ..Default::default()
    };
    crate::clock_discipline::discipline_from_gps(now() * 1000, &clock, &policy);
    assert!(clock.is_trusted());
    assert!(f.commit(&f.response(&f.value), clock.is_trusted()).is_ok());
}

#[test]
fn canonical_signature_and_storage_failclosed() {
    let f = Fixture::new();
    let req = Request {
        node_id: "lab-001".into(),
        current_key: f.old.sub.clone(),
        new_key: f.new.public_key(),
    };
    let c = Challenge {
        challenge_id: "a".repeat(64),
        nonce: "b".repeat(64),
        issued_at: 1700000000,
        expires_at: 1700000300,
    };
    let body = canonical(&req, &c);
    assert_eq!(
        body,
        format!(
            "node-self-renew-v1\n{}\n{}\nlab-001\n{}\n{}\n1700000000\n",
            "a".repeat(64),
            "b".repeat(64),
            req.current_key,
            req.new_key
        )
    );
    let sig = f.new.sign(body.as_bytes()).unwrap();
    assert!(f.new.verify(body.as_bytes(), &sig).is_ok());
    assert!(f.new.verify(format!("{body}x").as_bytes(), &sig).is_err());
    let _lock = lock(&f._root.0).unwrap();
    assert!(lock(&f._root.0).is_err());
    let target = f._root.0.join("blocked");
    fs::create_dir(&target).unwrap();
    assert!(atomic_write(&target, b"new").is_err());
    assert_eq!(fs::read_to_string(&f.renewal.path).unwrap(), f.raw);
    fs::set_permissions(&f._root.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(atomic_write(&f.renewal.path, b"new").is_err());
    fs::set_permissions(&f._root.0, fs::Permissions::from_mode(0o700)).unwrap();
    let symlink = f._root.0.join("symlink");
    std::os::unix::fs::symlink(&f.renewal.path, &symlink).unwrap();
    assert!(read_private(&symlink).is_err());
    assert!(jitter("lab-001") < 21600);
    assert_eq!(jitter("lab-001"), jitter("lab-001"));
}

#[tokio::test]
#[ignore = "requires the synthetic Go flock TLS harness; no installed-node paths"]
async fn flock_lab_transport() {
    let root = PathBuf::from(std::env::var("FLOCK_LAB_ROOT").expect("synthetic fixture directory"));
    let spec: Value =
        serde_json::from_slice(&fs::read(root.join("fixture.json")).unwrap()).unwrap();
    let endpoint = spec["endpoint"].as_str().unwrap();
    let url = reqwest::Url::parse(endpoint).unwrap();
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    let node = spec["node_id"].as_str().unwrap();
    let path = root.join("active.creds");
    let old_raw = read_private(&path).unwrap();
    let (_, old) = credentials(&old_raw, None, node).unwrap();
    assert!(
        old.exp < now(),
        "fixture must have expired NATS credentials"
    );
    let trust = Trust {
        issuer: old.iss,
        account: old.nats.issuer_account,
    };
    let renewal = Renewal {
        path: path.clone(),
        node: node.into(),
        trust,
    };
    let cert = reqwest::Certificate::from_der(&fs::read(root.join("server.der")).unwrap()).unwrap();
    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .add_root_certificate(cert)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let clock = crate::clock_discipline::TimeTrust::new();
    crate::clock_discipline::discipline_from_gps(
        now() * 1000,
        &clock,
        &crate::node_config::ClockYaml {
            consistent_fixes: 1,
            ..Default::default()
        },
    );
    assert!(clock.is_trusted());
    let result = renewal.attempt_at(&client, &clock, endpoint).await;
    if spec["expected"] == "renewed" {
        assert!(result.unwrap().unwrap() > now() + 30 * DAY);
        let raw = read_private(&path).unwrap();
        let (_, claims) = credentials(&raw, Some(&renewal.trust), node).unwrap();
        assert_ne!(claims.sub, old.sub);
    } else {
        assert_eq!(result.unwrap_err(), "owner_required");
        assert_eq!(read_private(&path).unwrap().as_str(), old_raw.as_str());
    }
}
