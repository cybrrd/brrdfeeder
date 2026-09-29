// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! E-CNP-001 Blue/Red policy verification — Phase 3, Drop 1: the cryptographic
//! membrane. The edge VERIFIES a System-5-signed policy before it would ever
//! act. THIS MODULE VERIFIES ONLY — no podman, no restart, no execution.
//!
//! The signature is over the E-CNP-001 §4.1 FULL-FIELD byte-string (NOT JSON):
//! every semantic field, fixed order, joined by "||", strings raw UTF-8, ints
//! decimal-ASCII, bool "true"/"false". The golden-vector test proves our bytes
//! reproduce the S5 (Go) signer's exactly — if it fails, the build fails and we
//! do not touch the wire.
//!
//! Edge is a hostile environment: every parse/verify path is panic-free
//! (#179 deny-lints), so a malformed packet is REJECTED, never a crash.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// S5 Ed25519 verification keyring (E-CNP-001 §2.2 — 2 slots).
/// Slot 0 = the production signer pubkey `1d75eca3…` (compile-pinned).
/// Slot 1 = rollover, not yet provisioned (all-zero → skipped).
pub const KEYRING: [[u8; 32]; 2] = [
    [
        0x1d, 0x75, 0xec, 0xa3, 0xb0, 0x45, 0x2b, 0x54, 0xa5, 0x30, 0xdf, 0xdf, 0x8b, 0xa4, 0x26,
        0x5c, 0x1c, 0x18, 0xe9, 0xf0, 0x9e, 0x71, 0x8d, 0xb7, 0x0e, 0xa1, 0x9c, 0xab, 0xe4, 0x71,
        0x2f, 0x1d,
    ],
    [0u8; 32],
];

/// The verification outcome. Distinct variants so the log/metrics name the exact
/// gate that failed (E-CNP-001 §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Verified,
    RejectedNodeMismatch,
    RejectedSig,
    RejectedReplay,
    RejectedDowngrade,
    RejectedMalformed,
    RejectedRunningIdentityUnverified,
}

/// cybrrd.blue.policy.v1 (E-CNP-001 §4). Unknown fields are ignored for
/// forward-compat; missing/ill-typed fields fail deserialization (→ rejected,
/// never panic).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BluePolicy {
    pub schema: String,
    pub node_id: String,
    pub channel: String,
    pub target_image: String,
    pub target_build_seq: u64,
    pub min_version_floor_digest: String,
    pub cadence: String,
    pub rollback: bool,
    pub effective_after_unix_ms: i64,
    pub grace_deadline_unix_ms: i64,
    pub issued_by: String,
    pub issued_unix_ms: i64,
    pub nonce: String,
    pub sig: String,
}

impl BluePolicy {
    /// E-CNP-001 §4.1 signed byte-string. MUST match the Go signer byte-for-byte
    /// (locked by the golden-vector test). Rust `u64/i64::to_string` →
    /// decimal-ASCII no leading zeros, `bool::to_string` → "true"/"false" — both
    /// match Go `strconv.FormatInt`/`FormatBool`.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let fields = [
            self.schema.clone(),
            self.node_id.clone(),
            self.channel.clone(),
            self.target_image.clone(),
            self.target_build_seq.to_string(),
            self.min_version_floor_digest.clone(),
            self.cadence.clone(),
            self.rollback.to_string(),
            self.effective_after_unix_ms.to_string(),
            self.grace_deadline_unix_ms.to_string(),
            self.issued_by.clone(),
            self.issued_unix_ms.to_string(),
            self.nonce.clone(),
        ];
        fields.join("||").into_bytes()
    }
}

/// Parse a Blue policy from raw bytes. Panic-free: any malformed input (bad
/// JSON, missing field, wrong type) returns Err — the caller logs REJECTED.
pub fn parse(raw: &[u8]) -> Result<BluePolicy, serde_json::Error> {
    serde_json::from_slice::<BluePolicy>(raw)
}

/// Verify the Ed25519 signature against a keyring (any non-zero slot may match).
/// Panic-free: bad hex, wrong length, or a malformed key all return false.
pub fn verify_with(policy: &BluePolicy, keyring: &[[u8; 32]]) -> bool {
    use ed25519_dalek::{Signature, VerifyingKey};
    let sig_bytes = match hex::decode(policy.sig.as_bytes()) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let sig_arr: [u8; 64] = match sig_bytes.try_into() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let sig = Signature::from_bytes(&sig_arr);
    let msg = policy.canonical_bytes();
    for key in keyring {
        if key.iter().all(|&b| b == 0) {
            continue; // unprovisioned slot
        }
        if let Ok(vk) = VerifyingKey::from_bytes(key) {
            // verify_strict rejects malleable / non-canonical signatures.
            if vk.verify_strict(&msg, &sig).is_ok() {
                return true;
            }
        }
    }
    false
}

/// Run the full E-CNP-001 edge gate against a parsed policy. Order is cheap →
/// expensive: identity filter, then Ed25519, then the monotonic anti-replay /
/// anti-downgrade checks (both keyed on caller-supplied persisted state).
pub fn check(
    policy: &BluePolicy,
    self_node_id: &str,
    last_applied_unix_ms: i64,
    last_applied_build_seq: u64,
    keyring: &[[u8; 32]],
) -> Verdict {
    if policy.node_id != self_node_id {
        return Verdict::RejectedNodeMismatch;
    }
    if !verify_with(policy, keyring) {
        return Verdict::RejectedSig;
    }
    // Anti-replay (§5.2): the policy must be strictly newer than the last applied.
    if policy.issued_unix_ms <= last_applied_unix_ms {
        return Verdict::RejectedReplay;
    }
    // Anti-downgrade (§5.3): no lower build_seq unless an explicit signed rollback.
    if policy.target_build_seq <= last_applied_build_seq && !policy.rollback {
        return Verdict::RejectedDowngrade;
    }
    Verdict::Verified
}

/// Persisted idempotency anchor (E-CNP-001 §5.2/§5.3). Advanced ONLY after a
/// policy is actually applied (Drop 2) — never on mere verification — so the
/// watermark reflects the real state on the iron and survives reboot / power loss.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyState {
    pub last_applied_unix_ms: i64,
    pub last_applied_build_seq: u64,
}

impl PolicyState {
    /// Load persisted state. Absent (first boot) or unreadable → zero watermark
    /// (the correct baseline). Corruption is logged, non-fatal, never panics.
    pub fn load(path: &Path) -> PolicyState {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("[blue] policy_state corrupt ({e}); resetting watermark to zero");
                PolicyState::default()
            }),
            Err(_) => PolicyState::default(),
        }
    }

    /// Atomically persist: write a temp file, fsync it, rename over the target
    /// (atomic on POSIX), then fsync the dir. A power loss mid-write leaves the
    /// PREVIOUS state intact — the anchor can never be half-written. Never panics.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let fname = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("policy_state");
        let tmp = dir.join(format!(".{fname}.tmp"));
        let json = serde_json::to_vec(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?; // fsync the data before the rename
        }
        std::fs::rename(&tmp, path)?; // atomic swap
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all(); // best-effort dir durability for the rename
        }
        Ok(())
    }
}

/// The outcome of running a raw control packet through the Drop 1 membrane.
pub struct ProcessOutcome {
    pub verdict: Verdict,
    pub policy: Option<BluePolicy>,
}

/// Drop 1 pipeline: parse → gate against the persisted watermark → Verdict.
/// VERIFY ONLY — it does NOT advance/persist the watermark (no execution in
/// Drop 1; the executor advances it after a successful apply, Drop 2).
pub fn process_blue(
    raw: &[u8],
    self_node_id: &str,
    state: &PolicyState,
    running_build_seq: Option<u64>,
    keyring: &[[u8; 32]],
) -> ProcessOutcome {
    let Some(running_build_seq) = running_build_seq else {
        return ProcessOutcome { verdict: Verdict::RejectedRunningIdentityUnverified, policy: None };
    };
    match parse(raw) {
        Ok(p) => {
            // Anti-downgrade floor = the HIGHER of the persisted watermark and
            // the running image's own build_seq. A node manually imaged to
            // build N has watermark 0 but is running N — without this, a signed
            // Blue for any 0<M<N would pass and silently downgrade it. Fold the
            // floor into the build_seq arg `check` compares against.
            let floor_seq = state.last_applied_build_seq.max(running_build_seq);
            let verdict = check(
                &p,
                self_node_id,
                state.last_applied_unix_ms,
                floor_seq,
                keyring,
            );
            ProcessOutcome { verdict, policy: Some(p) }
        }
        Err(_) => ProcessOutcome { verdict: Verdict::RejectedMalformed, policy: None },
    }
}

/// The tenant→host draft (Condo-Tenancy, Drop 2): the engine writes the full
/// signed Blue here; the host-side `.path`/updater re-verifies + pulls + pins +
/// restarts. Lives beside policy_state.json on the state volume.
pub const PENDING_UPDATE_FILENAME: &str = "pending_update.json";

pub fn pending_update_path(state_path: &Path) -> std::path::PathBuf {
    state_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(PENDING_UPDATE_FILENAME)
}

/// Atomically write the draft (temp+fsync+rename). The host `systemd.path` unit
/// fires on the rename-into-place (inotify) — never on a half-written file.
#[cfg(test)] // legacy fixtures only: no production Blue-to-updater writer
pub fn write_pending_update(state_path: &Path, raw_blue: &[u8]) -> std::io::Result<()> {
    let path = pending_update_path(state_path);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(".pending_update.{}.{}.tmp", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()));
    {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(raw_blue)?;
        f.sync_all()?;
    }
    // D44 shared single-slot mailbox: never replace a draft the host may be
    // reading. Poll and Blue both publish via create-if-absent hard link.
    let result = std::fs::hard_link(&tmp, &path);
    let _ = std::fs::remove_file(&tmp);
    result?;
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// On boot, reconcile any pending update: if a valid, self-addressed Blue's
/// `target_build_seq` == the build we ARE now, we are the applied result —
/// advance the anti-replay watermark to that decree and consume the draft.
/// If we are NOT yet the target (updater hasn't run / pull failed), leave the
/// draft so the host can act. Returns the applied build_seq when reconciled.
#[cfg(test)] // legacy fixtures only; host owns health-confirmed commits
pub fn reconcile_pending_update(
    state_path: &Path,
    running_build_seq: Option<u64>,
    self_node_id: &str,
    keyring: &[[u8; 32]],
) -> Option<u64> {
    // D44: restart is not health. The host owns watermark advancement while a
    // durable transaction is being evaluated, including after a power loss.
    if state_path.parent()?.join("update_transaction.json").exists() {
        return None;
    }
    let running_build_seq = running_build_seq?;
    let pending = pending_update_path(state_path);
    let raw = std::fs::read(&pending).ok()?;
    let p = parse(&raw).ok()?;
    if p.node_id != self_node_id || !verify_with(&p, keyring) {
        return None; // foreign or unsigned — never consume
    }
    if p.target_build_seq != running_build_seq {
        return None; // not yet applied; leave the draft for the host updater
    }
    // We ARE this build → advance the watermark (anti-replay) and consume.
    let mut st = PolicyState::load(state_path);
    st.last_applied_unix_ms = st.last_applied_unix_ms.max(p.issued_unix_ms);
    st.last_applied_build_seq = st.last_applied_build_seq.max(p.target_build_seq);
    if let Err(e) = st.save(state_path) {
        eprintln!("[blue] reconcile: failed to persist watermark: {e}");
        return None;
    }
    let _ = std::fs::remove_file(&pending);
    Some(p.target_build_seq)
}

/// The `verify-blue <file>` subcommand — the host updater's cryptographic
/// pre-flight (Condo-Tenancy §4). Parses the draft and verifies its Ed25519
/// signature against the COMPILE-PINNED key + digest-pinning, returning a
/// process exit code (0 = valid signed Blue). It deliberately does NOT check
/// node_id (the engine did that when it wrote the draft); this is purely the
/// "did S5 sign this, and is the target content-addressed" guard.
pub fn verify_blue_file(path: Option<&str>) -> i32 {
    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("verify-blue: missing <file> argument");
            return 2;
        }
    };
    let raw = match std::fs::read(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("verify-blue: cannot read {path}: {e}");
            return 2;
        }
    };
    verify_blue_bytes(&raw, &KEYRING)
}

/// Testable core of verify-blue: 0 = valid digest-pinned signed Blue.
fn verify_blue_bytes(raw: &[u8], keyring: &[[u8; 32]]) -> i32 {
    let p = match parse(raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("verify-blue: malformed: {e}");
            return 3;
        }
    };
    if !p.target_image.contains("@sha256:") {
        eprintln!("verify-blue: target_image is not digest-pinned (refusing)");
        return 3;
    }
    if verify_with(&p, keyring) {
        println!("verify-blue: OK node={} target={} build_seq={}", p.node_id, p.target_image, p.target_build_seq);
        0
    } else {
        eprintln!("verify-blue: SIGNATURE INVALID against pinned key");
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, VerifyingKey};

    // E-CNP-001 §4.2 vector with a generic fixture node identifier.
    // Re-signed with the published ephemeral test seed below; never a production key.
    const GOLDEN_PUBKEY_HEX: &str =
        "c252b980a6efdd931abfb38896a608d4d1ddd1e12acb663ae70c96358f007693";
    const GOLDEN_SIG_HEX: &str = "65f2fb1e9c450866a30fbba99f4c9c6d92ccef7a5b7bf5635833d2325cb56560f5075d33dc39742cd0844698d248a0e636b0aa9f2e47df21eacb5a1685b69406";

    fn golden() -> BluePolicy {
        BluePolicy {
            schema: "cybrrd.blue.policy.v1".into(),
            node_id: "bf-00000001".into(),
            channel: "rc".into(),
            target_image: "ghcr.io/macawi-ai/brrdfeeder-open@sha256:c696bae13182868b699c78b0baa563cf997b28260c63daf0d9ad895c7ff5d50e".into(),
            target_build_seq: 1042,
            min_version_floor_digest: "sha256:5e63a18e00000000000000000000000000000000000000000000000000000000".into(),
            cadence: "off_hours".into(),
            rollback: false,
            effective_after_unix_ms: 1781800000000,
            grace_deadline_unix_ms: 1782059200000,
            issued_by: "command.cybrrd.com".into(),
            issued_unix_ms: 1781799000000,
            nonce: "a7b3d9f2e4c1a8b6".into(),
            sig: GOLDEN_SIG_HEX.into(),
        }
    }

    fn golden_keyring() -> [[u8; 32]; 1] {
        let mut k = [0u8; 32];
        k.copy_from_slice(&hex::decode(GOLDEN_PUBKEY_HEX).unwrap());
        [k]
    }

    /// The §4.1 canonical bytes must match the independently signed fixture exactly.
    #[test]
    fn test_e_cnp_001_canonicalization() {
        let want = "cybrrd.blue.policy.v1||bf-00000001||rc||ghcr.io/macawi-ai/brrdfeeder-open@sha256:c696bae13182868b699c78b0baa563cf997b28260c63daf0d9ad895c7ff5d50e||1042||sha256:5e63a18e00000000000000000000000000000000000000000000000000000000||off_hours||false||1781800000000||1782059200000||command.cybrrd.com||1781799000000||a7b3d9f2e4c1a8b6";
        assert_eq!(String::from_utf8(golden().canonical_bytes()).unwrap(), want);
    }

    /// And the Ed25519 math holds: the golden sig verifies with the golden key.
    #[test]
    fn test_golden_vector_verifies() {
        let p = golden();
        let sig_arr: [u8; 64] = hex::decode(&p.sig).unwrap().try_into().unwrap();
        let vk = VerifyingKey::from_bytes(
            &hex::decode(GOLDEN_PUBKEY_HEX).unwrap().try_into().unwrap(),
        )
        .unwrap();
        assert!(vk
            .verify_strict(&p.canonical_bytes(), &Signature::from_bytes(&sig_arr))
            .is_ok());
        // And through our own helper:
        assert!(verify_with(&p, &golden_keyring()));
    }

    #[test]
    fn test_wrong_key_rejects() {
        // The production keyring (1d75eca3…) must NOT verify the test-key sig.
        assert!(!verify_with(&golden(), &KEYRING));
    }

    #[test]
    fn test_full_gate_verified() {
        assert_eq!(
            check(&golden(), "bf-00000001", 0, 0, &golden_keyring()),
            Verdict::Verified
        );
    }

    #[test]
    fn test_node_mismatch_rejected_before_crypto() {
        assert_eq!(
            check(&golden(), "some-other-node", 0, 0, &golden_keyring()),
            Verdict::RejectedNodeMismatch
        );
    }

    #[test]
    fn test_replay_rejected() {
        // last_applied at/after this policy's issued time → replay.
        assert_eq!(
            check(&golden(), "bf-00000001", 1781799000000, 0, &golden_keyring()),
            Verdict::RejectedReplay
        );
    }

    #[test]
    fn test_downgrade_rejected() {
        // last_applied seq >= target, no rollback → downgrade.
        assert_eq!(
            check(&golden(), "bf-00000001", 0, 1042, &golden_keyring()),
            Verdict::RejectedDowngrade
        );
    }

    // The golden seed (== Go signer_test.go) lets us SIGN test policies in Rust,
    // so the full gate can be exercised with real signatures.
    fn sign_with_golden(p: &mut BluePolicy) {
        use ed25519_dalek::{Signer, SigningKey};
        let seed: [u8; 32] = *b"ecologee-e-cnp-001-golden-seed!!";
        let sk = SigningKey::from_bytes(&seed);
        p.sig = hex::encode(sk.sign(&p.canonical_bytes()).to_bytes());
    }

    /// Rust signing must reproduce the independently computed fixture signature
    /// (Python cryptography Ed25519, using only the published test seed).
    #[test]
    fn test_rust_signing_reproduces_fixture_signature() {
        let mut p = golden();
        sign_with_golden(&mut p);
        assert_eq!(p.sig, GOLDEN_SIG_HEX);
    }

    #[test]
    fn test_rollback_permits_lower_seq() {
        let mut p = golden();
        p.rollback = true;
        p.target_build_seq = 5; // below last_applied (1042) — only rollback may pass
        sign_with_golden(&mut p);
        assert_eq!(
            check(&p, "bf-00000001", 0, 1042, &golden_keyring()),
            Verdict::Verified
        );
    }

    #[test]
    fn test_malformed_input_rejected_not_panicked() {
        assert!(parse(b"not json at all").is_err());
        assert!(parse(b"{\"schema\":\"x\"}").is_err()); // missing fields
        assert!(parse(b"").is_err());
    }

    #[test]
    fn test_state_roundtrip_atomic() {
        let path = std::env::temp_dir().join("bp_state_roundtrip.json");
        let _ = std::fs::remove_file(&path);
        assert_eq!(PolicyState::load(&path).last_applied_build_seq, 0); // absent → zero
        let s = PolicyState { last_applied_unix_ms: 1781799000000, last_applied_build_seq: 1042 };
        s.save(&path).unwrap();
        // no leftover temp file after the atomic rename
        assert!(!std::env::temp_dir().join(".bp_state_roundtrip.json.tmp").exists());
        let back = PolicyState::load(&path);
        assert_eq!(back.last_applied_unix_ms, 1781799000000);
        assert_eq!(back.last_applied_build_seq, 1042);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_corrupt_state_resets_to_zero() {
        let path = std::env::temp_dir().join("bp_state_corrupt.json");
        std::fs::write(&path, b"{garbage not json").unwrap();
        assert_eq!(PolicyState::load(&path).last_applied_build_seq, 0); // non-fatal
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_process_blue_verified() {
        let bytes = serde_json::to_vec(&golden()).unwrap();
        let out = process_blue(
            &bytes,
            "bf-00000001",
            &PolicyState::default(),
            Some(0),
            &golden_keyring(),
        );
        assert_eq!(out.verdict, Verdict::Verified);
        assert!(out.policy.is_some());
    }

    // D26 / PRV 5.4: a valid signature authorizes a requested target; it does
    // not prove what is currently running. Unknown must not become floor zero.
    #[test]
    fn unknown_identity_refuses_signed_update_and_rollback() {
        use ed25519_dalek::{Signer, SigningKey};
        let key = SigningKey::from_bytes(&[42; 32]); // synthetic test key only
        let keyring = [key.verifying_key().to_bytes()];
        for rollback in [false, true] {
            let mut policy = golden();
            policy.rollback = rollback;
            policy.sig = hex::encode(key.sign(&policy.canonical_bytes()).to_bytes());
            let bytes = serde_json::to_vec(&policy).unwrap();
            let state = PolicyState::default();
            assert_eq!(process_blue(&bytes, &policy.node_id, &state, Some(0), &keyring).verdict, Verdict::Verified);
            let refused = process_blue(&bytes, &policy.node_id, &state, None, &keyring);
            assert_eq!(refused.verdict, Verdict::RejectedRunningIdentityUnverified);
            assert!(refused.policy.is_none(), "refused policy must not reach draft writer");
        }
    }

    #[test]
    fn unknown_identity_preserves_pending_draft_and_watermark() {
        let dir = std::env::temp_dir().join(format!("d26-unknown-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("policy_state.json");
        PolicyState { last_applied_unix_ms: 7, last_applied_build_seq: 41 }.save(&state).unwrap();
        let before = std::fs::read(&state).unwrap();
        let bytes = serde_json::to_vec(&golden()).unwrap();
        write_pending_update(&state, &bytes).unwrap();
        assert_eq!(reconcile_pending_update(&state, None, "bf-00000001", &golden_keyring()), None);
        assert_eq!(std::fs::read(&state).unwrap(), before, "unverified boot advanced watermark");
        assert_eq!(std::fs::read(pending_update_path(&state)).unwrap(), bytes, "unverified boot consumed draft");
        std::fs::remove_file(pending_update_path(&state)).unwrap();
        std::fs::remove_file(&state).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn test_process_blue_malformed() {
        let out = process_blue(b"}{bad", "x", &PolicyState::default(), Some(0), &golden_keyring());
        assert_eq!(out.verdict, Verdict::RejectedMalformed);
        assert!(out.policy.is_none());
    }

    #[test]
    fn test_process_blue_replay_via_persisted_state() {
        let bytes = serde_json::to_vec(&golden()).unwrap();
        let st = PolicyState { last_applied_unix_ms: 1781799000000, last_applied_build_seq: 0 };
        let out = process_blue(&bytes, "bf-00000001", &st, Some(0), &golden_keyring());
        assert_eq!(out.verdict, Verdict::RejectedReplay);
    }

    #[test]
    fn test_running_build_seq_blocks_downgrade() {
        // Fresh watermark (0) but the running image is build 2000. A signed Blue
        // for seq 1042 must be rejected as a downgrade — the floor is the running
        // image, not the (zero) watermark.
        let bytes = serde_json::to_vec(&golden()).unwrap(); // golden target_build_seq = 1042
        let out = process_blue(
            &bytes,
            "bf-00000001",
            &PolicyState::default(),
            Some(2000),
            &golden_keyring(),
        );
        assert_eq!(out.verdict, Verdict::RejectedDowngrade);
    }

    #[test]
    fn test_verify_blue_bytes_paths() {
        let bytes = serde_json::to_vec(&golden()).unwrap();
        assert_eq!(verify_blue_bytes(&bytes, &golden_keyring()), 0); // valid (test key)
        assert_eq!(verify_blue_bytes(&bytes, &KEYRING), 4); // wrong (production) key
        assert_eq!(verify_blue_bytes(b"}{", &golden_keyring()), 3); // malformed
        let mut tag = golden();
        tag.target_image = "ghcr.io/cybrrd/brrdfeeder:latest".into();
        let tb = serde_json::to_vec(&tag).unwrap();
        assert_eq!(verify_blue_bytes(&tb, &golden_keyring()), 3); // not digest-pinned
    }

    #[test]
    fn test_pending_update_reconcile_roundtrip() {
        let dir = std::env::temp_dir().join("bp_drop2_reconcile");
        let _ = std::fs::create_dir_all(&dir);
        let state = dir.join("policy_state.json");
        let _ = std::fs::remove_file(&state);
        let bytes = serde_json::to_vec(&golden()).unwrap();
        write_pending_update(&state, &bytes).unwrap();
        assert!(pending_update_path(&state).exists());
        // running == target (1042): we ARE the applied build → reconcile + advance.
        let seq =
            reconcile_pending_update(&state, Some(1042), "bf-00000001", &golden_keyring());
        assert_eq!(seq, Some(1042));
        let st = PolicyState::load(&state);
        assert_eq!(st.last_applied_build_seq, 1042);
        assert_eq!(st.last_applied_unix_ms, 1781799000000);
        assert!(!pending_update_path(&state).exists()); // consumed
        let _ = std::fs::remove_file(&state);
    }

    #[test]
    fn d44_host_health_gate_owns_reconcile_and_mailbox() {
        let dir = std::env::temp_dir().join(format!("d44-blue-{}",std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("policy_state.json");
        let raw = serde_json::to_vec(&golden()).unwrap();
        write_pending_update(&state, &raw).unwrap();
        assert_eq!(write_pending_update(&state, b"replacement").unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        std::fs::write(dir.join("update_transaction.json"), br#"{"active":true}"#).unwrap();
        assert_eq!(reconcile_pending_update(&state, Some(1042), "bf-00000001", &golden_keyring()), None);
        assert!(!state.exists(), "startup advanced watermark before host health gate");
        assert_eq!(std::fs::read(pending_update_path(&state)).unwrap(), raw);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn test_reconcile_leaves_draft_when_not_target() {
        let dir = std::env::temp_dir().join("bp_drop2_nottarget");
        let _ = std::fs::create_dir_all(&dir);
        let state = dir.join("policy_state.json");
        let bytes = serde_json::to_vec(&golden()).unwrap();
        write_pending_update(&state, &bytes).unwrap();
        // running != target → not yet applied → leave the draft for the host updater.
        let seq =
            reconcile_pending_update(&state, Some(41), "bf-00000001", &golden_keyring());
        assert_eq!(seq, None);
        assert!(pending_update_path(&state).exists());
        let _ = std::fs::remove_file(pending_update_path(&state));
    }
}
