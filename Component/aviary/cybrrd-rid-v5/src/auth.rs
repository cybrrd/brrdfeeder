// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Stateless, append-only assembly. Callers retain every source observation and
//! assign a new revision; counter equality alone is never grouping authority.
use crate::{
    capture::{hex_bytes, ObservationId},
    hex,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageEvidence {
    pub observation_id: ObservationId,
    pub unit_index: usize,
    pub message_index: usize,
    #[serde(with = "hex_bytes")]
    pub bytes: Vec<u8>,
    pub source_mac: Option<String>,
    pub transport: String,
    pub counter: Option<u8>,
    /// Explicit identity/clock/capture-association evidence, not an 8-bit counter.
    pub association: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct AssemblyResult {
    pub pages: Vec<PageEvidence>,
    pub derived: Value,
}

pub fn assemble(
    pages: Vec<PageEvidence>,
    assembly_id: &str,
    revision: u64,
    version: &str,
) -> AssemblyResult {
    let mut out = json!({"assembly_id":assembly_id,"revision":revision,"state":"incomplete","page_refs":[],"counter_raw":null,"auth_type_raw":null,"last_page_raw":null,
        "payload_length_raw":null,"timestamp_raw":null,"payload_hex":null,"additional_length_raw":null,"additional_data_hex":null,"padding_hex":null,"issues":[],"assembler_version":version});
    let mut issues = vec![];
    let mut refs = vec![];
    let mut by_page: BTreeMap<u8, &PageEvidence> = BTreeMap::new();
    let mut conflict = false;
    let mut ambiguous = false;
    let mut valid = vec![];
    for p in &pages {
        if p.bytes.len() != 25 || p.bytes[0] != 0x22 {
            issues.push(json!({"code":"unsupported_auth_page","path":"pages","detail":"Exact malformed/unsupported page retained without interpreting its body."}));
            continue;
        }
        let number = p.bytes[1] & 15;
        let mut role = "contributing";
        if let Some(old) = by_page.get(&number) {
            if old.bytes == p.bytes {
                role = "duplicate";
            } else {
                role = "conflicting";
                conflict = true;
            }
        } else {
            by_page.insert(number, p);
        }
        refs.push(json!({"observation_id":p.observation_id,"unit_index":p.unit_index,"message_index":p.message_index,"page_number":number,"role":role}));
        valid.push(p);
    }
    if let Some(&first) = valid.first() {
        for &p in &valid {
            let same_event = p.observation_id == first.observation_id;
            let associated = first
                .association
                .as_ref()
                .is_some_and(|a| !a.is_empty() && Some(a) == p.association.as_ref())
                && first.source_mac.is_some()
                && p.source_mac == first.source_mac;
            if p.observation_id.node_id != first.observation_id.node_id
                || p.observation_id.boot_id != first.observation_id.boot_id
                || p.transport != first.transport
                || p.counter != first.counter
                || p.source_mac != first.source_mac
                || p.bytes[0] != first.bytes[0]
                || p.bytes[1] >> 4 != first.bytes[1] >> 4
                || !(same_event || associated)
            {
                ambiguous = true;
            }
        }
        if !ambiguous {
            out["counter_raw"] = json!(first.counter);
            out["auth_type_raw"] = json!(first.bytes[1] >> 4);
        }
    }
    if ambiguous {
        issues.push(json!({"code":"ambiguous_page_group","path":"page_refs","detail":"Counter/type equality cannot establish identity across capture generations, sources, boots or versions."}));
    }
    if conflict {
        issues.push(json!({"code":"conflicting_duplicate_page","path":"page_refs","detail":"Contradictory pages remain linked; no single payload is selected."}));
    }
    if !ambiguous && !conflict {
        if let Some(p0) = by_page.get(&0) {
            let last = p0.bytes[2] & 15;
            let length = usize::from(p0.bytes[3]);
            let typ = p0.bytes[1] >> 4;
            out["last_page_raw"] = json!(last);
            out["payload_length_raw"] = json!(length);
            out["timestamp_raw"] = json!(u32::from_le_bytes(p0.bytes[4..8].try_into().unwrap()));
            if p0.bytes[2] >> 4 != 0 {
                issues.push(json!({"code":"reserved_bits_nonzero","path":"page0.byte2","detail":if version=="design-oracle-0"{
                    format!("Synthetic page retains reserved high nibble {}; assembly completeness is not conformance or authentication verification.", p0.bytes[2] >> 4)
                }else{"Reserved high nibble is retained; completeness is not conformance or authentication verification.".to_owned()}}));
            }
            let mut joined = Vec::new();
            let mut missing = false;
            for page in 0..=last {
                if let Some(p) = by_page.get(&page) {
                    joined.extend_from_slice(&p.bytes[if page == 0 { 8 } else { 2 }..]);
                } else {
                    missing = true;
                }
            }
            if by_page.keys().any(|&n| n > last) {
                issues.push(json!({"code":"page_beyond_last","path":"last_page_raw","detail":"Extra pages are retained and prevent an unambiguous assembly."}));
                ambiguous = true;
            }
            if !missing && !ambiguous && length <= joined.len() {
                out["payload_hex"] = json!(hex(&joined[..length]));
                let mut end = length;
                if let Some(&additional) = joined.get(end) {
                    out["additional_length_raw"] = json!(additional);
                    end += 1;
                    let available = (end + usize::from(additional)).min(joined.len());
                    out["additional_data_hex"] = json!(hex(&joined[end..available]));
                    if end + usize::from(additional) > joined.len() {
                        missing = true;
                    }
                    end = available;
                } else {
                    out["additional_data_hex"] = json!("");
                }
                out["padding_hex"] = json!(hex(&joined[end..]));
                if joined[end..].iter().any(|&b| b != 0) {
                    issues.push(json!({"code":"nonzero_auth_padding","path":"padding_hex","detail":"Nonzero padding retained."}));
                }
                if !missing {
                    out["state"] = json!("complete");
                }
            } else if length > joined.len() {
                missing = true;
            }
            if missing {
                issues.push(json!({"code":"incomplete_auth_data","path":"page_refs","detail":"Missing page or payload/additional length exceeds available capacity; bytes are not padded."}));
            }
            if typ == 3 {
                let same_unit = valid.iter().all(|p| {
                    p.observation_id == p0.observation_id && p.unit_index == p0.unit_index
                });
                if !same_unit
                    || !matches!(
                        p0.transport.as_str(),
                        "bt5_long_range" | "wifi_nan" | "wifi_beacon"
                    )
                {
                    issues.push(json!({"code":"auth_type3_transport_constraint","path":"page_refs","detail":"Type 3 requires all pages together on a supporting transport."}));
                }
            }
            if typ == 4 && joined.iter().any(|&b| b != 0) {
                issues.push(json!({"code":"network_auth_nonzero_data","path":"page_refs","detail":"Type 4 data must be all null; nonzero bytes retained."}));
            }
            if typ == 5 {
                if let Some(&method) = joined.first() {
                    issues.push(json!({"code":method_status(method),"path":"payload_hex","detail":"Method selector is retained; no cryptographic verification was performed."}));
                }
            }
        }
    }
    if conflict {
        out["state"] = json!("conflicting");
    } else if ambiguous {
        out["state"] = json!("ambiguous");
    }
    out["page_refs"] = json!(refs);
    out["issues"] = json!(issues);
    AssemblyResult {
        pages,
        derived: out,
    }
}
pub fn method_status(method: u8) -> &'static str {
    match method {
        0 => "auth_method_reserved",
        1..=224 => "auth_method_registered_profile_required",
        225..=255 => "auth_method_private",
    }
}
