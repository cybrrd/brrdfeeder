// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Derived authentication reconstruction. No verification or automatic identity
//! inference from an eight-bit transport counter. Source pages remain unchanged.
use super::{hex, Error, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Page {
    pub observation_id: Value,
    pub unit_index: usize,
    pub message_index: usize,
    pub raw: [u8; 25],
    pub source_mac: String,
    pub counter: u8,
    /// Explicit grouping evidence, scoped to node/boot/MAC/version/counter.
    /// None permits only pages in the same observation and unit.
    pub association: Option<String>,
    pub transport_kind: String,
}
pub fn reconstruct(pages: &[Page]) -> Result<Value> {
    let first = pages.first().ok_or_else(|| Error("no pages".into()))?;
    let mut refs = Vec::new();
    let mut numbered = BTreeMap::<u8, &Page>::new();
    let mut issues = Vec::new();
    let mut state = "complete";
    for p in pages {
        refs.push(json!({"observation_id":p.observation_id,"unit_index":p.unit_index,"message_index":p.message_index,
            "raw_hex":hex(&p.raw),"counter_raw":p.counter,"source_mac":p.source_mac,"association":p.association}));
        let same_context = p.source_mac == first.source_mac
            && p.counter == first.counter
            && p.raw[0] == first.raw[0]
            && p.raw[1] >> 4 == first.raw[1] >> 4
            && p.observation_id["node_id"] == first.observation_id["node_id"]
            && p.observation_id["boot_id"] == first.observation_id["boot_id"];
        let same_unit =
            p.observation_id == first.observation_id && p.unit_index == first.unit_index;
        let associated = p.association.as_ref().is_some_and(|a| !a.is_empty())
            && p.association == first.association;
        if !same_context || (!same_unit && !associated) {
            state = "ambiguous";
            issues.push("insufficient_or_conflicting_association");
        }
        if p.raw[0] != 0x22 {
            state = "ambiguous";
            issues.push("unsupported_page_header");
        }
        let page = p.raw[1] & 15;
        if let Some(old) = numbered.get(&page) {
            if old.raw != p.raw {
                if state != "ambiguous" {
                    state = "conflicting";
                }
                issues.push("conflicting_page");
            } else {
                issues.push("duplicate_page");
            }
        } else {
            numbered.insert(page, p);
        }
        if p.raw[1] >> 4 == 3 && (!same_unit || p.transport_kind == "bt4_legacy") {
            issues.push("message_set_transport_constraint");
        }
    }
    let mut out = json!({"state":state,"page_refs":refs,"counter_raw":first.counter,"auth_type_raw":first.raw[1]>>4,
        "last_page_raw":null,"payload_length_raw":null,"timestamp_raw":null,"payload_hex":null,
        "additional_length_raw":null,"additional_data_hex":null,"padding_hex":null,"issues":issues,
        "verification":"not_performed"});
    if state == "ambiguous" || state == "conflicting" {
        return Ok(out);
    }
    let Some(zero) = numbered.get(&0) else {
        out["state"] = json!("incomplete");
        return Ok(out);
    };
    let last = zero.raw[2] & 15;
    let length = usize::from(zero.raw[3]);
    out["last_page_raw"] = json!(last);
    out["payload_length_raw"] = json!(length);
    out["timestamp_raw"] = json!(u32::from_le_bytes([
        zero.raw[4],
        zero.raw[5],
        zero.raw[6],
        zero.raw[7]
    ]));
    if zero.raw[2] >> 4 != 0 {
        issues.push("nonzero_page0_reserved");
    }
    if numbered.keys().any(|&n| n > last) {
        state = "conflicting";
        issues.push("page_beyond_declared_last");
    }
    let mut data = Vec::new();
    for n in 0..=last {
        let Some(p) = numbered.get(&n) else {
            state = "incomplete";
            issues.push("missing_page");
            break;
        };
        data.extend_from_slice(if n == 0 { &p.raw[8..] } else { &p.raw[2..] });
    }
    if state == "complete" && data.len() >= length {
        out["payload_hex"] = json!(hex(&data[..length]));
        if data.len() > length {
            let n = usize::from(data[length]);
            out["additional_length_raw"] = json!(n);
            let end = length + 1 + n;
            if end > data.len() {
                state = "incomplete";
                issues.push("truncated_additional_data");
            } else {
                out["additional_data_hex"] = json!(hex(&data[length + 1..end]));
                out["padding_hex"] = json!(hex(&data[end..]));
                if data[end..].iter().any(|&b| b != 0) {
                    issues.push("nonzero_padding");
                }
            }
        } else {
            out["additional_data_hex"] = json!("");
            out["padding_hex"] = json!("");
        }
    } else if data.len() < length {
        state = "incomplete";
        issues.push("truncated_payload");
    }
    if first.raw[1] >> 4 == 4 && data.iter().any(|&b| b != 0) {
        issues.push("network_auth_nonzero_data");
    }
    if first.raw[1] >> 4 == 5 {
        issues.push(match data.first() {
            Some(0) => "specific_method_reserved",
            Some(225..=255) => "specific_method_private_source_conflict",
            Some(_) => "specific_method_profile_required",
            None => "specific_method_missing",
        });
    }
    out["state"] = json!(state);
    out["issues"] = json!(issues);
    Ok(out)
}
