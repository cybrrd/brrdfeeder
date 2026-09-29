// Substrate-truth capability verification (Wave 7.1, 2026-05-10).
//
// The engine performs two privileged operations directly, without
// subprocess shell-out:
//
//   • CAP_NET_ADMIN — for nl80211 NL80211_CMD_SET_CHANNEL and
//     NL80211_CMD_GET_SURVEY (the Kittler Substrate Defense Hunter:
//     5GHz channel rotation + per-channel survey telemetry).
//
//   • CAP_NET_RAW   — for libpcap raw monitor-mode packet capture
//     (the substrate-truth packet-receive path).
//
// These caps must be granted via filesystem capabilities on the
// engine binary itself:
//
//     sudo setcap cap_net_admin,cap_net_raw=ep /path/to/engine
//
// The 2026-05-07 iproute2 incident demonstrated the cost of an
// unaudited capability assumption: caps silently absent → operations
// silently fail → dark deployment, drone overflight unwitnessed. This
// module makes that failure mode loud and fast: the engine refuses to
// start blind.
//
// Lineage: Kittler Substrate Defense — the trail must be witnessed,
// and the witness must be empowered to witness. A brick that cannot
// hop channels because of a missing capability is a courier reading
// letters on a hijacked horse.

use caps::{CapSet, Capability};

#[derive(Debug)]
pub struct CapabilityCheckFailure {
    pub missing: Vec<Capability>,
}

impl std::fmt::Display for CapabilityCheckFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "Required Linux capabilities are absent from this process's effective set:"
        )?;
        for cap in &self.missing {
            writeln!(f, "  • missing: {:?}", cap)?;
        }
        writeln!(f)?;
        writeln!(f, "Fix: grant capabilities to the engine binary via filesystem caps:")?;
        writeln!(
            f,
            "    sudo setcap cap_net_admin,cap_net_raw=ep <path-to-engine-binary>"
        )?;
        writeln!(f)?;
        writeln!(f, "Verify with:  getcap <path-to-engine-binary>")?;
        writeln!(
            f,
            "Context:      docs/src/substrate-device-identity.md  (Kittler Substrate Defense)"
        )?;
        Ok(())
    }
}

impl std::error::Error for CapabilityCheckFailure {}

/// Verify that the running process holds the Linux capabilities
/// required for the Kittler Substrate Defense Hunter to operate.
///
/// Returns Ok(()) when both CAP_NET_ADMIN and CAP_NET_RAW are present
/// in the effective capability set; returns Err with a list of missing
/// capabilities otherwise.
///
/// Note: a process running as root has the full effective set by
/// definition, so this check passes trivially under root. The check
/// is meaningful primarily for the production path where the engine
/// runs as an unprivileged user with file-cap grants on the binary.
pub fn check_required() -> Result<(), CapabilityCheckFailure> {
    let required = [Capability::CAP_NET_ADMIN, Capability::CAP_NET_RAW];
    let mut missing = Vec::new();

    for cap in required {
        match caps::has_cap(None, CapSet::Effective, cap) {
            Ok(true) => {}
            Ok(false) => missing.push(cap),
            Err(e) => {
                eprintln!("[capabilities] probe error for {:?}: {}", cap, e);
                // Substrate-honest: probe error means we can't confirm
                // the cap is present, so treat as absent. Fail loud.
                missing.push(cap);
            }
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(CapabilityCheckFailure { missing })
    }
}
