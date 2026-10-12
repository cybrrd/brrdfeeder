// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! HTTPS renewal is independent of NATS authentication. Trust comes from the
//! authenticated, read-only installation credential, never the HTTPS response.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use nkeys::KeyPair;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

const DAY: i64 = 86400;
pub const ACTIVE: &str = "/var/lib/brrdfeeder/credentials/brrdfeeder.creds";
const BOOTSTRAP: &str = "/etc/brrdfeeder/secrets/brrdfeeder.creds";
const ENDPOINT: &str = "https://ingest.cybrrd.com/v1/renew";
static RETRY_AFTER: AtomicU64 = AtomicU64::new(0);
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema: String,
    pub state: String,
    pub expires_at: Option<i64>,
    pub last_success_at: Option<i64>,
}
static STATUS: LazyLock<Mutex<Report>> = LazyLock::new(|| {
    Mutex::new(Report {
        schema: "cybrrd.credential-renewal.v1".into(),
        state: "starting".into(),
        ..Report::default()
    })
});
pub fn snapshot() -> Report {
    STATUS.lock().map(|v| v.clone()).unwrap_or_default()
}
pub fn unavailable() {
    report("trust_unavailable", None, false);
}
static ROTATION: LazyLock<tokio::sync::watch::Sender<u64>> =
    LazyLock::new(|| tokio::sync::watch::channel(0).0);
pub fn signal_upward_rotation() {
    ROTATION.send_modify(|n| *n = n.saturating_add(1));
}
pub fn upward_rotation() -> tokio::sync::watch::Receiver<u64> {
    ROTATION.subscribe()
}
fn report(state: &str, expiry: Option<i64>, success: bool) {
    if let Ok(mut s) = STATUS.lock() {
        s.state = state.into();
        if expiry.is_some() {
            s.expires_at = expiry;
        }
        if success {
            s.last_success_at = Some(now());
        }
    }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs() as i64)
}
pub fn alert() -> bool {
    let s = snapshot();
    s.expires_at.is_some_and(|v| v - now() <= 14 * DAY)
        || matches!(
            s.state.as_str(),
            "clock_untrusted"
                | "owner_required"
                | "verification_failed"
                | "storage_unavailable"
                | "trust_unavailable"
                | "renewal_pending"
        )
}
#[derive(Clone)]
struct Trust {
    account: String,
    issuer: String,
}
#[derive(Deserialize)]
struct Header {
    typ: String,
    alg: String,
}
#[derive(Deserialize)]
struct Claims {
    sub: String,
    name: String,
    iss: String,
    exp: i64,
    iat: i64,
    #[serde(default)]
    nbf: Option<i64>,
    nats: UserClaims,
}
#[derive(Deserialize)]
struct UserClaims {
    issuer_account: String,
    #[serde(rename = "type")]
    kind: String,
    version: u64,
}

fn decode(token: &str, trust: Option<&Trust>, node: &str) -> Result<Claims> {
    if token.len() > 16384 {
        return Err("invalid_jwt".into());
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("invalid_jwt".into());
    }
    let bytes = |s: &str| B64.decode(s).map_err(|_| "invalid_jwt".to_string());
    let header: Header = serde_json::from_slice(&bytes(parts[0])?).map_err(|_| "invalid_jwt")?;
    let c: Claims = serde_json::from_slice(&bytes(parts[1])?).map_err(|_| "invalid_jwt")?;
    if header.typ != "JWT"
        || header.alg != "ed25519-nkey"
        || c.nats.kind != "user"
        || c.nats.version != 2
        || !c.sub.starts_with('U')
        || !c.iss.starts_with('A')
        || !c.nats.issuer_account.starts_with('A')
        || c.name != node
    {
        return Err("invalid_jwt".into());
    }
    KeyPair::from_public_key(&c.sub).map_err(|_| "invalid_subject")?;
    KeyPair::from_public_key(&c.nats.issuer_account).map_err(|_| "invalid_account")?;
    if trust.is_some_and(|t| c.iss != t.issuer || c.nats.issuer_account != t.account) {
        return Err("untrusted_issuer".into());
    } // RENEW MUTATE issuer
    let signer = KeyPair::from_public_key(&c.iss).map_err(|_| "invalid_issuer")?;
    signer
        .verify(
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &bytes(parts[2])?,
        )
        .map_err(|_| "invalid_signature")?; // RENEW MUTATE signature
    Ok(c)
}
fn armor_part(raw: &str, begin: &str, end: &str) -> Result<String> {
    let (_, rest) = raw.split_once(begin).ok_or("invalid_credentials")?;
    let (v, _) = rest.split_once(end).ok_or("invalid_credentials")?;
    Ok(v.trim().into())
}
fn credentials(raw: &str, trust: Option<&Trust>, node: &str) -> Result<(KeyPair, Claims)> {
    let token = armor_part(
        raw,
        "-----BEGIN NATS USER JWT-----",
        "------END NATS USER JWT------",
    )?;
    let seed = Zeroizing::new(armor_part(
        raw,
        "-----BEGIN USER NKEY SEED-----",
        "------END USER NKEY SEED------",
    )?);
    if !seed.starts_with("SU") {
        return Err("invalid_seed".into());
    }
    let kp = KeyPair::from_seed(&seed).map_err(|_| "invalid_seed")?;
    let c = decode(&token, trust, node)?;
    if kp.public_key() != c.sub {
        return Err("credential_key_mismatch".into());
    }
    Ok((kp, c))
}
fn read_private(path: &Path) -> Result<Zeroizing<String>> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "storage_unavailable")?;
    let m = f.metadata().map_err(|_| "storage_unavailable")?;
    if !m.is_file()
        || m.len() > 32768
        || m.nlink() != 1
        || m.mode() & 0o027 != 0
        || (m.uid() != 0 && m.uid() != unsafe { libc::geteuid() })
    {
        return Err("unsafe_credentials".into());
    }
    let mut raw = Zeroizing::new(String::new());
    f.take(32769)
        .read_to_string(&mut raw)
        .map_err(|_| "storage_unavailable")?;
    Ok(raw)
}
fn private_dir(dir: &Path) -> Result<()> {
    match fs::create_dir(dir) {
        Ok(()) => fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|_| "storage_unavailable")?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("storage_unavailable".into()),
    }
    let m = fs::symlink_metadata(dir).map_err(|_| "storage_unavailable")?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err("unsafe_directory".into());
    }
    Ok(())
}
pub(crate) fn atomic_write(path: &Path, raw: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or("unsafe_path")?;
    private_dir(dir)?;
    let temp = dir.join(format!(".renew-{}", KeyPair::new_user().public_key()));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)
            .map_err(|_| "storage_unavailable")?;
        f.write_all(raw).map_err(|_| "storage_unavailable")?;
        f.sync_all().map_err(|_| "storage_unavailable")?;
        fs::rename(&temp, path).map_err(|_| "storage_unavailable")?;
        File::open(dir)
            .and_then(|f| f.sync_all())
            // Rename already happened. Retrying must reconnect to the active
            // file, not treat this as a pre-swap rejection or restore old keys.
            .map_err(|_| "renewal_pending")?;
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    result
}
struct DiskLock(File);
impl Drop for DiskLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
use std::os::fd::AsRawFd;
fn lock(dir: &Path) -> Result<DiskLock> {
    private_dir(dir)?;
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(".renew.lock"))
        .map_err(|_| "storage_unavailable")?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("renewal_pending".into());
    }
    Ok(DiskLock(f))
}
pub struct Renewal {
    path: PathBuf,
    node: String,
    trust: Trust,
}
// OTA compatibility: the existing state mount is already writable by the
// engine. Provision only a private child; the bootstrap secret stays read-only.
pub fn prepare(configured: &str, node: &str) -> Result<Renewal> {
    if configured != BOOTSTRAP && configured != ACTIVE {
        report("trust_unavailable", None, false);
        return Err("unsupported_credential_path".into());
    }
    let raw = read_private(Path::new(BOOTSTRAP))?;
    let (_, claims) = credentials(&raw, None, node)?;
    let trust = Trust {
        issuer: claims.iss,
        account: claims.nats.issuer_account,
    };
    let path = PathBuf::from(ACTIVE);
    let dir = path.parent().ok_or("unsafe_path")?;
    let _guard = lock(dir)?;
    if !path.try_exists().map_err(|_| "storage_unavailable")? {
        atomic_write(&path, raw.as_bytes())?;
    }
    let active = read_private(&path)?;
    let (_, c) = credentials(&active, Some(&trust), node)?;
    report("ready", Some(c.exp), false);
    Ok(Renewal {
        path,
        node: node.into(),
        trust,
    })
}
pub fn active_path(configured: &str) -> String {
    if configured == BOOTSTRAP || configured == ACTIVE {
        ACTIVE.into()
    } else {
        configured.into()
    }
}
impl Renewal {
    pub fn path(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}
#[derive(Serialize, Clone)]
struct Request {
    node_id: String,
    current_key: String,
    new_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Challenge {
    challenge_id: String,
    nonce: String,
    issued_at: i64,
    expires_at: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    node_id: String,
    public_key: String,
    expires_at: i64,
    superseded_key: String,
    jwt: String,
}
fn canonical(req: &Request, c: &Challenge) -> String {
    format!(
        "node-self-renew-v1\n{}\n{}\n{}\n{}\n{}\n{}\n",
        c.challenge_id, c.nonce, req.node_id, req.current_key, req.new_key, c.issued_at
    )
}
fn validate_response(
    res: &Response,
    key: &KeyPair,
    old: &Claims,
    trust: &Trust,
    node: &str,
    utc: i64,
    trusted: bool,
) -> Result<Claims> {
    if !trusted {
        return Err("clock_untrusted".into());
    }
    let c = decode(&res.jwt, Some(trust), node)?;
    if c.sub != key.public_key()
        || res.public_key != c.sub
        || res.node_id != node
        || res.superseded_key != old.sub
        || res.expires_at != c.exp
    {
        return Err("response_identity_mismatch".into());
    } // RENEW MUTATE subject
    if c.exp <= old.exp
        || c.exp <= utc + 30 * DAY
        || c.exp <= c.iat
        || c.exp.checked_sub(c.iat).is_none_or(|ttl| ttl > 90 * DAY)
        || c.iat.abs_diff(utc) > 300
        || c.nbf.is_some_and(|v| v > utc)
    {
        return Err("response_time_invalid".into());
    } // RENEW MUTATE expiry
    Ok(c)
}
async fn post<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    body: &impl Serialize,
) -> Result<T> {
    let mut res = client
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|_| "network_unavailable")?;
    if !res.status().is_success() {
        let code = res.status().as_u16();
        // Retry-After may only increase the normal backoff; no hot retry loop.
        if code == 429 {
            let delay = res
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(3600);
            RETRY_AFTER.store(delay.min(86400), Ordering::Relaxed);
            return Err("rate_limited".into());
        }
        let mut body = Vec::new();
        while let Some(chunk) = res.chunk().await.map_err(|_| "network_unavailable")? {
            if body.len() + chunk.len() > 32768 {
                return Err("invalid_response".into());
            }
            body.extend_from_slice(&chunk);
        }
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        if code == 403 || (code == 410 && error["error"] == "outside_grace") {
            return Err("owner_required".into());
        }
        return Err("renewal_pending".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|_| "network_unavailable")? {
        if bytes.len() + chunk.len() > 32768 {
            return Err("invalid_response".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid_response".into())
}
impl Renewal {
    async fn attempt(
        &self,
        client: &reqwest::Client,
        trust_clock: &crate::clock_discipline::TimeTrust,
    ) -> Result<Option<i64>> {
        self.attempt_at(client, trust_clock, ENDPOINT).await
    }
    // Only the test module supplies a loopback TLS endpoint. Production has no
    // environment/config endpoint override or downgrade to HTTP.
    async fn attempt_at(
        &self,
        client: &reqwest::Client,
        trust_clock: &crate::clock_discipline::TimeTrust,
        endpoint: &str,
    ) -> Result<Option<i64>> {
        if !trust_clock.is_trusted() {
            return Err("clock_untrusted".into());
        }
        let dir = self.path.parent().ok_or("unsafe_path")?;
        let _guard = lock(dir)?;
        let old_raw = read_private(&self.path)?;
        let (old_key, old) = credentials(&old_raw, Some(&self.trust), &self.node)?;
        if snapshot().state != "renewal_pending" {
            report("ready", Some(old.exp), false);
        }
        if old.exp - now() > 30 * DAY {
            return Ok(None);
        }
        let new = KeyPair::new_user();
        let req = Request {
            node_id: self.node.clone(),
            current_key: old.sub.clone(),
            new_key: new.public_key(),
        };
        let c: Challenge = post(client, &format!("{endpoint}/challenge"), &req).await?;
        if c.challenge_id.len() != 64
            || c.nonce.len() != 64
            || !c
                .challenge_id
                .bytes()
                .chain(c.nonce.bytes())
                .all(|v| v.is_ascii_hexdigit() && !v.is_ascii_uppercase())
            || c.issued_at.abs_diff(now()) > 300
            || c.expires_at != c.issued_at + 300
        {
            return Err("invalid_challenge".into());
        }
        let body = canonical(&req, &c);
        let a = old_key
            .sign(body.as_bytes())
            .map_err(|_| "signing_failed")?;
        let b = new.sign(body.as_bytes()).map_err(|_| "signing_failed")?;
        let res:Response=post(client,&format!("{endpoint}/complete"),&serde_json::json!({"node_id":req.node_id,"current_key":req.current_key,"new_key":req.new_key,"challenge_id":c.challenge_id,"nonce":c.nonce,"issued_at":c.issued_at,"current_signature":B64.encode(a),"new_signature":B64.encode(b)})).await?;
        self.commit_response(&res, &new, &old, &old_raw, now(), trust_clock.is_trusted())
            .map(Some)
    }
    fn commit_response(
        &self,
        res: &Response,
        new: &KeyPair,
        old: &Claims,
        old_raw: &str,
        utc: i64,
        trusted: bool,
    ) -> Result<i64> {
        let claims = validate_response(res, new, old, &self.trust, &self.node, utc, trusted)
            .map_err(|_| "verification_failed")?;
        let seed = Zeroizing::new(new.seed().map_err(|_| "signing_failed")?);
        if KeyPair::from_seed(&seed)
            .map_err(|_| "invalid_seed")?
            .public_key()
            != claims.sub
        {
            return Err("credential_key_mismatch".into());
        }
        let creds=Zeroizing::new(format!("-----BEGIN NATS USER JWT-----\n{}\n------END NATS USER JWT------\n\n-----BEGIN USER NKEY SEED-----\n{}\n------END USER NKEY SEED------\n",res.jwt,seed.as_str()));
        let dir = self.path.parent().ok_or("unsafe_path")?;
        atomic_write(&dir.join("previous.creds"), old_raw.as_bytes())?;
        atomic_write(&self.path, creds.as_bytes())?;
        // Never reactivate previous.creds automatically (may be revoked).
        Ok(claims.exp)
    }
}
fn jitter(node: &str) -> u64 {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(node.as_bytes());
    u16::from_be_bytes([h[0], h[1]]) as u64 % (6 * 3600)
}
pub fn start(
    configured: String,
    node: String,
    clock: Arc<crate::clock_discipline::TimeTrust>,
    nats: crate::nats_publisher::NatsHandle,
) {
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
        {
            Ok(v) => v,
            Err(_) => {
                report("trust_unavailable", None, false);
                return;
            }
        };
        let mut failures = 0u32;
        let mut prepared: Option<Renewal> = None;
        loop {
            if prepared.is_none() {
                match prepare(&configured, &node) {
                    Ok(v) => prepared = Some(v),
                    Err(reason) => {
                        report(&reason, None, false);
                        tokio::time::sleep(Duration::from_secs(300)).await;
                        continue;
                    }
                }
            }
            let renewal = prepared.as_ref().expect("prepared above");
            let result = renewal.attempt(&client, &clock).await;
            let delay = match result {
                Ok(Some(exp)) => {
                    report("renewal_pending", Some(exp), false);
                    if nats.rotate_credentials().await {
                        report("renewed", Some(exp), true);
                        failures = 0;
                        86400 + jitter(&renewal.node)
                    } else {
                        failures += 1;
                        300
                    }
                }
                Ok(None) => {
                    if snapshot().state == "renewal_pending" {
                        if nats.rotate_credentials().await {
                            report("renewed", None, true);
                            failures = 0;
                            86400 + jitter(&renewal.node)
                        } else {
                            300
                        }
                    } else {
                        failures = 0;
                        86400 + jitter(&renewal.node)
                    }
                }
                Err(reason) => {
                    report(&reason, None, false);
                    failures = failures.saturating_add(1);
                    if reason == "clock_untrusted" {
                        60
                    } else {
                        (300u64.saturating_mul(1u64 << failures.min(8))
                            + jitter(&renewal.node).min(300))
                        .min(86400)
                    }
                }
            };
            let delay = delay.max(RETRY_AFTER.swap(0, Ordering::Relaxed));
            tokio::time::sleep(Duration::from_secs(delay.min(86400 + 6 * 3600))).await;
        }
    });
}

/// Explicit root-host recovery notification. Register even if automatic
/// preparation failed, so SIGUSR1 never terminates the engine by default.
pub fn listen_for_owner_rotation(
    nats: crate::nats_publisher::NatsHandle,
    configured: String,
    node: String,
) -> std::io::Result<()> {
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1())?;
    tokio::spawn(async move {
        while signal.recv().await.is_some() {
            if let Err(reason) = prepare(&configured, &node) {
                report(&reason, None, false);
                continue;
            }
            report("renewal_pending", None, false);
            if nats.rotate_credentials().await {
                report("renewed", None, true);
            }
        }
    });
    Ok(())
}

/// Runs without radio capabilities inside the installed image, not in the
/// capture process. The host CLI requires root + synchronized time first.
pub async fn owner_recover(configured: &str, node: &str, hostname: &str) -> Result<()> {
    if hostname.is_empty()
        || hostname.len() > 32
        || !hostname
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("invalid_hostname".into());
    }
    let renewal = prepare(configured, node)?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| "network_unavailable")?;
    const ISSUER: &str = "https://hospitality.cybrrd.com";
    const CLIENT: &str = "376855240068104243";
    let auth = client
        .post(format!("{ISSUER}/oauth/v2/device_authorization"))
        .form(&[
            ("client_id", CLIENT),
            ("scope", "openid profile offline_access"),
        ])
        .send()
        .await
        .map_err(|_| "network_unavailable")?;
    if !auth.status().is_success() {
        return Err("device_authorization_failed".into());
    }
    let auth: serde_json::Value = bounded_json(auth).await?;
    let code = Zeroizing::new(
        auth["device_code"]
            .as_str()
            .ok_or("invalid_device_response")?
            .to_owned(),
    );
    let uri = auth["verification_uri_complete"]
        .as_str()
        .or(auth["verification_uri"].as_str())
        .ok_or("invalid_device_response")?;
    if !uri.starts_with("https://hospitality.cybrrd.com/") {
        return Err("invalid_device_response".into());
    }
    println!(
        "Recovering installed node {node}. Open {uri}\nConfirm code: {}",
        auth["user_code"]
            .as_str()
            .ok_or("invalid_device_response")?
    );
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs(auth["expires_in"].as_u64().unwrap_or(300).min(900));
    let mut interval = auth["interval"].as_u64().unwrap_or(5).clamp(1, 60);
    let token = loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err("device_code_expired_retry_command".into());
        }
        let res = client
            .post(format!("{ISSUER}/oauth/v2/token"))
            .form(&[
                ("client_id", CLIENT),
                ("device_code", code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|_| "network_unavailable")?;
        let success = res.status().is_success();
        let value: serde_json::Value = bounded_json(res).await?;
        if success {
            break Zeroizing::new(
                value["access_token"]
                    .as_str()
                    .ok_or("invalid_token_response")?
                    .to_owned(),
            );
        }
        match value["error"].as_str() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = (interval + 5).min(60),
            Some("access_denied") => return Err("owner_authorization_denied".into()),
            _ => return Err("device_login_failed_retry_command".into()),
        }
    };
    // Lock only after interactive approval; automatic renewal can run meanwhile.
    let dir = renewal.path.parent().ok_or("unsafe_path")?;
    let _guard = lock(dir)?;
    let old_raw = read_private(&renewal.path)?;
    let (_, old) = credentials(&old_raw, Some(&renewal.trust), node)?;
    let res = client
        .post("https://ingest.cybrrd.com/v1/enroll/oauth")
        .bearer_auth(token.as_str())
        .json(&serde_json::json!({"node_hostname":hostname,"renew_node_id":node}))
        .send()
        .await
        .map_err(|_| "network_unavailable")?;
    if !res.status().is_success() {
        return Err(if res.status().as_u16() == 403 {
            "owner_or_operator_authorization_required"
        } else {
            "owner_exchange_failed"
        }
        .into());
    }
    if res
        .headers()
        .get("X-Cybrrd-Node-Id")
        .and_then(|v| v.to_str().ok())
        != Some(node)
    {
        return Err("owner_identity_mismatch".into());
    }
    let raw = Zeroizing::new(
        String::from_utf8(bounded_bytes(res).await?).map_err(|_| "invalid_credentials")?,
    );
    let (key, c) = credentials(&raw, Some(&renewal.trust), node)?;
    let jwt = armor_part(
        &raw,
        "-----BEGIN NATS USER JWT-----",
        "------END NATS USER JWT------",
    )?;
    let response = Response {
        node_id: node.into(),
        public_key: c.sub.clone(),
        expires_at: c.exp,
        superseded_key: old.sub.clone(),
        jwt,
    };
    validate_response(&response, &key, &old, &renewal.trust, node, now(), true)?;
    if c.sub == old.sub {
        return Err("replacement_key_required".into());
    }
    atomic_write(&dir.join("previous.creds"), old_raw.as_bytes())?;
    atomic_write(&renewal.path, raw.as_bytes())?;
    println!("Node {node}: replacement verified and installed; expiry UTC unix {}. Identity and captured state preserved.",c.exp);
    Ok(())
}
async fn bounded_bytes(mut res: reqwest::Response) -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|_| "network_unavailable")? {
        if raw.len() + chunk.len() > 32768 {
            return Err("invalid_response".into());
        }
        raw.extend_from_slice(&chunk);
    }
    Ok(raw)
}
async fn bounded_json(res: reqwest::Response) -> Result<serde_json::Value> {
    serde_json::from_slice(&bounded_bytes(res).await?).map_err(|_| "invalid_response".into())
}

#[cfg(test)]
#[path = "credential_renewal_tests.rs"]
mod tests;
