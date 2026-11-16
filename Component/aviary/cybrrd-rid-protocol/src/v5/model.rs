// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Validated, lossless observation tree. Additive members remain in the tree.
use super::{codec, decode_message, decode_unit, schema, unhex, Error, Result};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    record: Value,
}
impl Observation {
    pub fn new(record: Value) -> Result<Self> {
        validate(&record)?;
        Ok(Self { record })
    }
    pub fn record(&self) -> &Value {
        &self.record
    }
    pub fn into_record(self) -> Value {
        self.record
    }
}
pub fn validate_message(m: &Value) -> Result<()> {
    schema::validate(m, schema::message_contract()?)?;
    let raw = unhex(string(m, "raw_hex")?)?;
    let index = usize_field(m, "index")?;
    let expected = decode_message(&raw, index)?;
    schema::required_equal(&expected, m, "message")?;
    for name in [
        "basic_id",
        "location",
        "auth",
        "self_id",
        "system",
        "operator_id",
    ] {
        if expected.get(name).is_none() && m.get(name).is_some() {
            return Err(Error(format!("unexpected interpreted body {name}")));
        }
    }
    Ok(())
}
pub fn validate(record: &Value) -> Result<()> {
    codec::domain(record, 0)?;
    schema::validate(record, schema::contract()?)?;
    if record["wire_format_version"] != 5 || record["schema_revision"] != 0 {
        return Err(Error("unsupported observation revision".into()));
    }
    let id = &record["observation_id"];
    string(id, "capture_sequence")?
        .parse::<u64>()
        .map_err(|_| Error("capture sequence exceeds u64".into()))?;
    let captures = array(record, "captures")?;
    if captures.len() != 1 {
        return Err(Error("one event per observation".into()));
    }
    let cap = &captures[0];
    if cap["index"] != 0
        || cap["capture_id"]["capture_index"] != 0
        || cap["capture_id"]["observation_id"] != *id
    {
        return Err(Error("capture identity".into()));
    }
    let raw = unhex(string(cap, "raw_hex")?)?;
    if usize_field(cap, "captured_length")? != raw.len() {
        return Err(Error("capture length".into()));
    }
    let receiver = &record["receiver"];
    if receiver["raw_frame_bytes"]["raw"] != cap["raw_hex"]
        || receiver["raw_frame_bytes"]["decoded"] != cap["raw_hex"]
    {
        return Err(Error("raw frame mismatch".into()));
    }
    for (i, edge) in array(cap, "related_captures")?.iter().enumerate() {
        if usize_field(edge, "index")? != i || edge["source_capture_id"] != cap["capture_id"] {
            return Err(Error("association source/order".into()));
        }
        if edge["target_capture_id"] == cap["capture_id"] {
            return Err(Error("self association".into()));
        }
        let source_kind = match edge["relationship"].as_str() {
            Some("bt5_primary_to_secondary") => "bt5_primary",
            Some("bt5_secondary_to_primary") => "bt5_secondary",
            Some("nan_sync_to_sdf") => "wifi_nan_sync",
            Some("nan_sdf_to_sync") => "wifi_nan_sdf",
            _ => return Err(Error("association relationship".into())),
        };
        if cap["transport_kind"] != source_kind {
            return Err(Error("association relationship/source kind".into()));
        }
        if edge["order_basis"] == "receiver_capture_sequence" {
            let source = &edge["source_capture_id"]["observation_id"];
            let target = &edge["target_capture_id"]["observation_id"];
            if source["node_id"] != target["node_id"]
                || source["boot_id"] != target["boot_id"]
                || edge["source_receive_order"] != source["capture_sequence"]
                || edge["target_receive_order"] != target["capture_sequence"]
            {
                return Err(Error("association receive order".into()));
            }
        }
    }
    for (i, u) in array(record, "rid_units")?.iter().enumerate() {
        if usize_field(u, "index")? != i || usize_field(u, "capture_index")? != 0 {
            return Err(Error("unit index".into()));
        }
        let unit_raw = unhex(string(u, "raw_hex")?)?;
        let offset = usize_field(u, "capture_offset")?;
        let end = offset
            .checked_add(unit_raw.len())
            .ok_or_else(|| Error("unit range overflow".into()))?;
        if raw.get(offset..end) != Some(unit_raw.as_slice()) {
            return Err(Error("unit capture span".into()));
        }
        let counter = match u.get("message_counter") {
            Some(c) => Some(
                u8::try_from(
                    c["raw"]
                        .as_u64()
                        .ok_or_else(|| Error("counter raw".into()))?,
                )
                .map_err(|_| Error("counter range".into()))?,
            ),
            None => None,
        };
        let kind = string(u, "kind")?;
        if kind == "malformed" {
            if !array(u, "messages")?.is_empty() || !array(u, "basic_ids")?.is_empty() {
                return Err(Error("malformed unit has interpreted messages".into()));
            }
            continue;
        }
        let expected = decode_unit(&unit_raw, kind == "message_pack", i, 0, offset, counter)?;
        schema::required_equal(&expected, u, "unit")?;
        // Prevent extra body interpretations while retaining true extension members.
        for (m, e) in array(u, "messages")?
            .iter()
            .zip(array(&expected, "messages")?)
        {
            for name in [
                "basic_id",
                "location",
                "auth",
                "self_id",
                "system",
                "operator_id",
            ] {
                if e.get(name).is_none() && m.get(name).is_some() {
                    return Err(Error(format!("unexpected interpreted body {name}")));
                }
            }
        }
    }
    Ok(())
}
pub fn array<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    v.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| Error(format!("{key}: array required")))
}
pub fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error(format!("{key}: string required")))
}
pub fn usize_field(v: &Value, key: &str) -> Result<usize> {
    v.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| Error(format!("{key}: index required")))
}
