// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! D33: host-observed container manifest identity, bound to one unit invocation.
//! Build metadata is a compile-time assertion, never a runtime ENV fallback.
//! None of these observations alone is a cryptographic provenance attestation.

use serde::Deserialize;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

pub const UPDATE_BLOCKED_REASON: &str = "running_identity_unverified";

#[derive(Clone)]
pub struct RunningIdentity {
    pub engine_version: Option<String>,
    pub image_digest: Option<String>,
    pub build_seq: Option<u64>,
    container_bound: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Handoff {
    schema_version: u8,
    invocation_id: String,
    state: String,
    container_id: Option<String>,
    image_digest: Option<String>,
}

fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn build_metadata(version: Option<&str>, seq: Option<&str>) -> (Option<String>, Option<u64>) {
    let version = version.filter(|v| hex(v, 40)).map(str::to_owned);
    let seq = seq
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0);
    (version, seq)
}

// None = pending/missing; Some(None) = terminal unknown; Some(Some(d)) = known.
fn read_handoff(path: &Path, invocation: &str, owner: u32) -> Option<Option<String>> {
    if !hex(invocation, 32) {
        return Some(None);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.uid() != owner || meta.mode() & 0o022 != 0 || meta.len() > 4096 {
        return Some(None);
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes).ok()?;
    let record: Handoff = serde_json::from_slice(&bytes).ok()?;
    if record.schema_version != 1 || record.invocation_id != invocation {
        return Some(None);
    }
    match record.state.as_str() {
        "pending" => None,
        "known" => {
            let cid = record.container_id?;
            let digest = record.image_digest?;
            if hex(&cid, 64) && digest.strip_prefix("sha256:").is_some_and(|d| hex(d, 64)) {
                Some(Some(digest))
            } else {
                Some(None)
            }
        }
        _ => Some(None),
    }
}

impl RunningIdentity {
    #[cfg(test)]
    pub fn upward_test_fixture() -> Self {
        Self::from_sources(Some(format!("sha256:{}", "a".repeat(64))), Some(&"b".repeat(40)), Some("42"))
    }

    #[cfg(test)]
    pub fn unverified() -> Self {
        Self {
            engine_version: None,
            image_digest: None,
            build_seq: None,
            container_bound: false,
        }
    }

    fn from_sources(digest: Option<String>, version: Option<&str>, seq: Option<&str>) -> Self {
        let (engine_version, build_seq) = build_metadata(version, seq);
        Self {
            container_bound: digest.is_some(),
            image_digest: digest,
            engine_version,
            build_seq,
        }
    }

    pub async fn at_start() -> Self {
        let invocation = std::env::var("BRRDFEEDER_IDENTITY_INVOCATION").unwrap_or_default();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
        let digest = loop {
            if let Some(result) = read_handoff(
                Path::new("/run/brrdfeeder-identity/identity.json"),
                &invocation,
                0,
            ) {
                break result;
            }
            if tokio::time::Instant::now() >= deadline {
                break None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };
        Self::from_sources(
            digest,
            option_env!("BRRDFEEDER_BUILD_GIT_SHA"),
            option_env!("BRRDFEEDER_BUILD_SEQUENCE"),
        )
    }

    pub fn blocked_reason(&self) -> Option<&'static str> {
        self.verified_policy_floor()
            .is_none()
            .then_some(UPDATE_BLOCKED_REASON)
    }

    pub fn log_startup(&self, mut info: impl Write, mut warning: impl Write) -> io::Result<()> {
        writeln!(
            info,
            "[identity] image_digest={} source={} engine_version={} build_seq={}",
            self.image_digest.as_deref().unwrap_or("unknown"),
            if self.container_bound {
                "podman_container_inspect"
            } else {
                "unavailable"
            },
            self.engine_version.as_deref().unwrap_or("unknown"),
            self.build_seq
                .map(|s| s.to_string())
                .unwrap_or_else(|| "unknown".into())
        )?;
        if let Some(reason) = self.blocked_reason() {
            writeln!(warning, "[identity] WARNING reason={reason}; updates refused; running-container identity or binary build metadata unavailable; legacy runtime identity environment ignored")?;
        }
        Ok(())
    }

    // All three inputs are required. A baked sequence alone is not identity.
    pub fn verified_policy_floor(&self) -> Option<u64> {
        if self.container_bound && self.image_digest.is_some() && self.engine_version.is_some() {
            self.build_seq
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(invocation: &str) -> String {
        format!(
            r#"{{"schema_version":1,"invocation_id":"{invocation}","state":"known","container_id":"{}","image_digest":"sha256:{}"}}"#,
            "c".repeat(64),
            "d".repeat(64)
        )
    }

    #[test]
    fn invocation_permissions_and_shape_are_required() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = std::env::temp_dir().join(format!("d33-identity-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("identity.json");
        let inv = "a".repeat(32);
        let owner = unsafe { libc::geteuid() };
        assert_eq!(read_handoff(&path, &inv, owner), None);
        std::fs::write(&path, record(&inv)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            read_handoff(&path, &inv, owner),
            Some(Some(format!("sha256:{}", "d".repeat(64))))
        );
        assert_eq!(
            read_handoff(&path, &"b".repeat(32), owner),
            Some(None),
            "prior invocation accepted"
        );
        assert_eq!(read_handoff(&path, &inv, owner + 1), Some(None));
        for mode in [0o666, 0o664] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(read_handoff(&path, &inv, owner), Some(None));
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let link = root.join("symlink");
        symlink(&path, &link).unwrap();
        assert_eq!(read_handoff(&link, &inv, owner), None);
        for body in [
            record(&inv).replace("sha256:", "image-id:"),
            record(&inv).replace("known", "unverified"),
            record(&inv).replace("\"schema_version\":1", "\"schema_version\":2"),
        ] {
            std::fs::write(&path, body).unwrap();
            assert_eq!(read_handoff(&path, &inv, owner), Some(None));
        }
        std::fs::remove_file(link).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn digest_and_baked_metadata_jointly_control_policy_and_silver() {
        let digest = format!("sha256:{}", "d".repeat(64));
        let sha = "a".repeat(40);
        for (d, v, s) in [
            (None, Some(sha.as_str()), Some("287")),
            (Some(digest.clone()), None, Some("287")),
            (Some(digest.clone()), Some(sha.as_str()), None),
            (Some(digest.clone()), Some("unknown"), Some("287")),
            (Some(digest.clone()), Some(sha.as_str()), Some("0")),
            (Some(digest.clone()), Some(sha.as_str()), Some("-1")),
            (
                Some(digest.clone()),
                Some(sha.as_str()),
                Some("18446744073709551616"),
            ),
        ] {
            let unknown = RunningIdentity::from_sources(d, v, s);
            assert_eq!(
                unknown.verified_policy_floor(),
                None,
                "partial identity authorized policy"
            );
            assert_eq!(unknown.blocked_reason(), Some(UPDATE_BLOCKED_REASON));
        }
        let known = RunningIdentity::from_sources(Some(digest.clone()), Some(&sha), Some("287"));
        assert_eq!(known.verified_policy_floor(), Some(287));
        assert_eq!(known.blocked_reason(), None);
        let (mut info, mut warning) = (Vec::new(), Vec::new());
        known.log_startup(&mut info, &mut warning).unwrap();
        assert!(warning.is_empty());
        let fleet = crate::heartbeat::FleetProprioception {
            identity: known,
            silver: Some(crate::silver::Context {
                configured_position: crate::silver::ConfiguredPosition {
                    latitude: 40.0,
                    longitude: -95.0,
                    elevation_meters: 300.0,
                    source: crate::silver::ConfigSource::ConfigStatic,
                },
                config_hash: Some(format!("sha256:{}", "e".repeat(64))),
                fresh_for_ms: 30_000,
            }),
            channel: "stable".into(),
            time_trust: std::sync::Arc::new(crate::clock_discipline::TimeTrust::new()),
        };
        let hb = crate::heartbeat::build_payload(
            "fixture",
            std::time::Instant::now(),
            &crate::heartbeat::RadioState::new(),
            None,
            None,
            None,
            None,
            Some(&fleet),
        );
        assert_eq!(hb.image_digest.as_deref(), Some(digest.as_str()));
        assert_eq!(hb.engine_version.as_deref(), Some(sha.as_str()));
        assert_eq!(hb.build_seq, Some(287));
        assert!(hb.update_blocked_reason.is_none());
        assert_eq!(
            hb.management_status,
            Some(crate::silver::ManagementStatus::Attention)
        );
        assert!(
            hb.policy_ack.is_none(),
            "identity is not a policy-application receipt"
        );
    }

    #[test]
    fn absent_identity_warns_once_and_has_no_policy_floor() {
        let identity = RunningIdentity::unverified();
        assert!(identity.image_digest.is_none());
        assert!(identity.engine_version.is_none());
        assert!(identity.build_seq.is_none());
        assert_eq!(identity.verified_policy_floor(), None);
        let (mut info, mut warning) = (Vec::new(), Vec::new());
        identity.log_startup(&mut info, &mut warning).unwrap();
        let info = String::from_utf8(info).unwrap();
        let warning = String::from_utf8(warning).unwrap();
        assert_eq!(info.lines().count(), 1);
        assert!(info.contains("image_digest=unknown source=unavailable"));
        assert_eq!(warning.lines().count(), 1);
        assert!(warning.contains(UPDATE_BLOCKED_REASON));
    }

    // Run in an isolated child test process so environment mutation cannot
    // race other engine tests. PRV 5.4: ENV is an assertion, not a receipt.
    #[test]
    fn runtime_environment_never_supplies_identity() {
        const CHILD: &str = "D26_IDENTITY_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let identity = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(RunningIdentity::at_start());
            assert!(
                identity.image_digest.is_none(),
                "stale ENV digest published"
            );
            let baked = build_metadata(
                option_env!("BRRDFEEDER_BUILD_GIT_SHA"),
                option_env!("BRRDFEEDER_BUILD_SEQUENCE"),
            );
            assert_eq!(identity.engine_version, baked.0, "runtime version asserted");
            assert_eq!(identity.build_seq, baked.1, "runtime sequence asserted");
            assert_eq!(identity.verified_policy_floor(), None);
            return;
        }
        for digest in [
            format!("sha256:{}", "a".repeat(64)),
            "malformed".into(),
            String::new(),
        ] {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "identity::tests::runtime_environment_never_supplies_identity",
                ])
                .env(CHILD, "1")
                .env("BRRDFEEDER_IMAGE_DIGEST", digest)
                .env("BRRDFEEDER_GIT_SHA", "a".repeat(40))
                .env("BRRDFEEDER_BUILD_SEQ", "287")
                .env("BRRDFEEDER_BUILD_GIT_SHA", "b".repeat(40))
                .env("BRRDFEEDER_BUILD_SEQUENCE", "999999")
                .env_remove("BRRDFEEDER_IDENTITY_INVOCATION")
                .status()
                .unwrap();
            assert!(status.success(), "runtime ENV must not supply identity");
        }
    }
}
