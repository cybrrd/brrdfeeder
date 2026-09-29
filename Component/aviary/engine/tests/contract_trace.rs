// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! `contract-trace` — the Executable Contracts CI gate (schema 2, profile-scoped).
//!
//! The metapractice says: **no claimed capability without a single-sourced,
//! CI-refutable contract.** This test is the mechanism that makes that
//! enforceable rather than aspirational. Without it, `capabilities.toml` and
//! `requirements/*.yaml` are documentation that drifts; with it, drift fails the
//! build.
//!
//! ## Why this file was rewritten (aviary migration, 2026-07-19)
//!
//! The schema-1 version parsed `status = "..."` as a SCALAR under each capability
//! section. Schema 2 moved status into a per-profile sub-table
//! (`[rid_transport.wifi_beacon.status]` with `soho = "..."`, `opensource = "..."`).
//!
//! Carried forward unchanged, the old parser found no capability statuses at all —
//! only the incidental `status` keys on `[profile.*]` — and since none of those read
//! `"implemented"`, **the rule passed vacuously while enforcing nothing.** It was
//! green. It was checking air.
//!
//! That is the worst failure mode available to a contract gate, so RULE 0 below
//! exists specifically to make vacuity impossible: the gate asserts that it actually
//! found things to check before it reports success. A test that cannot fail is not a
//! test.
//!
//! Four rules, all enforcing:
//!
//! 0. **Anti-vacuity.** The parse must yield profiles, capabilities, and a non-zero
//!    number of per-profile status assertions. Silence is treated as breakage.
//! 1. **No shelf-ware requirements.** Every REQ with `status: bound` must name at
//!    least one artifact in `verify:` that EXISTS — a Rust test present in the
//!    source, or a file present in the repo.
//! 2. **No uncontracted claims.** Every capability `implemented` on ANY profile must
//!    be referenced by some requirement, or listed in `[contract_debt]` with a
//!    reason. Debt is allowed; SILENT debt is not.
//! 3. **No undefined profile claims.** Every capability must declare a status for
//!    EVERY known profile. An omitted profile is an unanswered question about what
//!    ships, and unanswered questions are how BLE leaks into the public cut.
//!
//! Deliberately dependency-free (hand-parsed): adding a parser crate would alter the
//! dependency tree and invalidate the SBOM published to Dependency-Track. These are
//! our own files in a controlled format, so line parsing is honest here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <repo>/engine
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("engine/ must have a parent")
        .to_path_buf()
}

/// Every `.rs` file in the workspace (excluding build output and any archive).
fn rust_sources(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if p.is_dir() {
                if matches!(name, "target" | ".git" | "archive") {
                    continue;
                }
                walk(&p, out);
            } else if name.ends_with(".rs") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// Minimal reader for our REQ YAML: id, status, and the `verify:` list items.
struct Req {
    id: String,
    status: String,
    verify: Vec<String>,
    blob: String,
}

fn load_reqs(root: &Path) -> Vec<Req> {
    let dir = root.join("requirements");
    let mut reqs = Vec::new();
    for entry in fs::read_dir(&dir).expect("requirements/ must exist").flatten() {
        let p = entry.path();
        let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !(fname.starts_with("REQ-") && fname.ends_with(".yaml")) {
            continue;
        }
        let text = fs::read_to_string(&p).expect("REQ file must be readable");
        let mut id = String::new();
        let mut status = String::new();
        let mut verify = Vec::new();
        let mut in_verify = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(v) = trimmed.strip_prefix("id:") {
                id = v.trim().trim_matches('"').to_string();
            }
            if let Some(v) = trimmed.strip_prefix("status:") {
                status = v.trim().trim_matches('"').to_string();
            }
            if trimmed.starts_with("verify:") {
                in_verify = true;
                continue;
            }
            if in_verify {
                if let Some(item) = trimmed.strip_prefix("- ") {
                    verify.push(item.trim().trim_matches('"').to_string());
                } else if !line.starts_with(' ') && trimmed.ends_with(':') {
                    in_verify = false;
                }
            }
        }
        assert!(!id.is_empty(), "{fname}: missing `id:`");
        assert!(!status.is_empty(), "{id}: missing `status:`");
        reqs.push(Req { id, status, verify, blob: text.to_lowercase() });
    }
    assert!(!reqs.is_empty(), "no REQ files found — the ledger cannot be empty");
    reqs
}

/// The parsed capability manifest (schema 2).
struct Manifest {
    /// Declared profiles, e.g. {"soho", "enterprise", "opensource", "hummingbrrd"}.
    profiles: BTreeSet<String>,
    /// capability path -> (profile -> status)
    caps: BTreeMap<String, BTreeMap<String, String>>,
    /// keys declared in `[contract_debt]`
    debt: Vec<String>,
}

impl Manifest {
    /// Total number of per-profile status assertions parsed. If this is zero the
    /// gate is checking nothing (see RULE 0).
    fn assertion_count(&self) -> usize {
        self.caps.values().map(|m| m.len()).sum()
    }
}

fn load_manifest(root: &Path) -> Manifest {
    let text = fs::read_to_string(root.join("capabilities.toml"))
        .expect("capabilities.toml must exist — it is the SoT");

    let mut profiles = BTreeSet::new();
    let mut caps: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut debt = Vec::new();

    // Section kinds we care about:
    //   [profile.<name>]              -> declares a profile
    //   [<group>.<cap>.status]        -> per-profile statuses for that capability
    //   [contract_debt]               -> declared debt allowlist
    enum Sect {
        CapStatus(String),
        Debt,
        Other,
    }
    let mut current = Sect::Other;

    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            let name = t.trim_matches(['[', ']'].as_ref()).to_string();
            current = if let Some(p) = name.strip_prefix("profile.") {
                profiles.insert(p.to_string());
                Sect::Other
            } else if let Some(cap) = name.strip_suffix(".status") {
                caps.entry(cap.to_string()).or_default();
                Sect::CapStatus(cap.to_string())
            } else if name == "contract_debt" {
                Sect::Debt
            } else {
                // a bare capability section, e.g. [rid_transport.wifi_beacon]
                if name.contains('.') {
                    caps.entry(name.clone()).or_default();
                }
                Sect::Other
            };
            continue;
        }
        let Some((k, v)) = t.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim().trim_matches('"'));
        match &current {
            Sect::CapStatus(cap) => {
                caps.entry(cap.clone()).or_default().insert(k.to_string(), v.to_string());
            }
            Sect::Debt => debt.push(k.to_string()),
            Sect::Other => {}
        }
    }

    Manifest { profiles, caps, debt }
}

/// RULE 0 — the gate must actually be checking something.
///
/// This exists because the schema-1 parser, run against schema 2, silently found
/// zero capability statuses and passed. A contract gate that cannot fail is a lie
/// told with a green check mark.
#[test]
fn gate_is_not_vacuous() {
    let m = load_manifest(&repo_root());

    assert!(
        !m.profiles.is_empty(),
        "no [profile.*] sections parsed — capabilities.toml is not schema 2, or the \
         parser has drifted from the file format"
    );
    assert!(
        !m.caps.is_empty(),
        "no capability sections parsed from capabilities.toml"
    );
    assert!(
        m.assertion_count() >= m.caps.len(),
        "parsed {} capabilities but only {} per-profile status assertions — the gate \
         would run without checking most claims. Expected every capability to carry a \
         [<cap>.status] table.",
        m.caps.len(),
        m.assertion_count()
    );
}

/// RULE 1 — a bound requirement may not cite verification that does not exist.
#[test]
fn bound_requirements_name_artifacts_that_exist() {
    let root = repo_root();
    let corpus: String = rust_sources(&root)
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .collect::<Vec<_>>()
        .join("\n");

    let mut failures = Vec::new();

    for req in load_reqs(&root).iter().filter(|r| r.status == "bound") {
        assert!(
            !req.verify.is_empty(),
            "{}: status is `bound` but `verify:` is empty — a bound requirement \
             must name how it is refuted",
            req.id
        );

        let mut satisfied = 0usize;
        for entry in &req.verify {
            // A cited Rust test: `...::tests::name` or `...::name`
            if let Some(idx) = entry.rfind("::") {
                let tail: String = entry[idx + 2..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if tail.len() > 3
                    && tail.chars().all(|c| c.is_lowercase() || c.is_numeric() || c == '_')
                {
                    if corpus.contains(&format!("fn {tail}(")) {
                        satisfied += 1;
                    } else {
                        failures.push(format!(
                            "{}: cites test `{}` which does not exist in any .rs source",
                            req.id, tail
                        ));
                    }
                    continue;
                }
            }
            // A cited repo file (e.g. a udev rule)
            if let Some(tok) = entry.split_whitespace().find(|t| {
                t.contains('/')
                    && (t.ends_with(".rules") || t.ends_with(".toml") || t.ends_with(".yaml"))
            }) {
                if root.join(tok).exists() {
                    satisfied += 1;
                } else {
                    failures.push(format!("{}: cites file `{}` which does not exist", req.id, tok));
                }
            }
        }

        if satisfied == 0 {
            failures.push(format!(
                "{}: status `bound` but no `verify:` entry resolves to an existing test or file",
                req.id
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "contract-trace RULE 1 failed — bound requirements cite missing verification:\n  {}",
        failures.join("\n  ")
    );
}

/// RULE 2 — a capability `implemented` on ANY profile must be contracted, or its
/// debt declared.
#[test]
fn implemented_capabilities_are_contracted_or_declared_debt() {
    let root = repo_root();
    let m = load_manifest(&root);
    let reqs = load_reqs(&root);

    let mut failures = Vec::new();
    let mut checked = 0usize;

    for (path, by_profile) in &m.caps {
        // `rid_transport.wifi_beacon` -> `wifi_beacon`
        let short = path.rsplit('.').next().unwrap_or(path);

        for (profile, status) in by_profile {
            if status != "implemented" {
                continue;
            }
            checked += 1;

            let covered = reqs
                .iter()
                .any(|r| r.blob.contains(short) || r.blob.contains(&short.replace('_', " ")));
            let declared = m.debt.iter().any(|d| d == short || d == path);

            if !covered && !declared {
                failures.push(format!(
                    "capability `{path}` is `implemented` on profile `{profile}` but no \
                     requirement references it and it is not listed in [contract_debt]"
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "contract-trace RULE 2 failed — capabilities claimed without contracts:\n  {}\n\
         Fix by authoring a REQ, or by declaring the gap explicitly in \
         capabilities.toml [contract_debt] with a reason. Debt is allowed; \
         SILENT debt is not.",
        failures.join("\n  ")
    );

    // Anti-vacuity, scoped to this rule: if nothing anywhere is `implemented`, the
    // rule proved nothing and should say so rather than report success.
    assert!(
        checked > 0,
        "RULE 2 examined zero `implemented` capabilities. Either the manifest claims \
         nothing at all, or the parser is not seeing statuses — both mean this gate is \
         not protecting anything."
    );
}

/// RULE 3 — every capability must declare a status for every known profile.
///
/// An omitted profile is an unanswered question about what ships. Unanswered
/// questions are exactly how a commercial-only capability leaks into the public cut.
#[test]
fn every_capability_covers_every_profile() {
    let m = load_manifest(&repo_root());
    let mut failures = Vec::new();

    for (path, by_profile) in &m.caps {
        for profile in &m.profiles {
            if !by_profile.contains_key(profile) {
                failures.push(format!(
                    "capability `{path}` declares no status for profile `{profile}`"
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "contract-trace RULE 3 failed — capabilities with undefined profile claims:\n  {}\n\
         Every capability must say `implemented`, `planned`, or `none` for every \
         profile. Silence is not an answer.",
        failures.join("\n  ")
    );
}
