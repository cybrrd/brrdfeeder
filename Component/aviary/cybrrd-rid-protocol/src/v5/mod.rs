// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Development-only v5 observation path. Does not alter v4 or publish to a broker.
pub mod adapters;
pub mod assembly;
pub mod codec;
pub mod model;
pub mod producer;
pub mod schema;

use cybrrd_rid_core::v5 as core;
use serde_json::{json, Map, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
pub const MAX_PARCEL_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_DEPTH: usize = 96;

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    s
}
pub fn unhex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        return Err(Error("odd hex length".into()));
    }
    fn nibble(b: u8) -> Result<u8> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err(Error("noncanonical hex".into())),
        }
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|c| Ok(nibble(c[0])? * 16 + nibble(c[1])?))
        .collect()
}
pub fn decimal(coefficient: i64, scale: u8) -> String {
    if coefficient == 0 {
        return "0".into();
    }
    let digits = coefficient.unsigned_abs().to_string();
    let scale = usize::from(scale);
    if scale == 0 {
        return coefficient.to_string();
    }
    let mut s = if digits.len() <= scale {
        format!("0.{}{}", "0".repeat(scale - digits.len()), digits)
    } else {
        format!(
            "{}.{}",
            &digits[..digits.len() - scale],
            &digits[digits.len() - scale..]
        )
    };
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if coefficient < 0 {
        s.insert(0, '-');
    }
    s
}
fn value(v: core::Value<'_>) -> Value {
    match v {
        core::Value::Null => Value::Null,
        core::Value::Integer(i) => json!(i),
        core::Value::Decimal { coefficient, scale } => json!(decimal(coefficient, scale)),
        core::Value::Bytes(b) => json!(hex(b)),
        core::Value::Text(s) => json!(s),
    }
}
pub fn cell(c: core::Cell<'_>) -> Value {
    let mut v = json!({"raw":value(c.raw),"decoded":value(c.decoded),"status":c.status,
        "unit":c.unit,"resolution":c.resolution,"reason":c.reason});
    if let Some(a) = c.arithmetic_value {
        v["arithmetic_value"] = value(a);
    }
    v
}
pub fn decode_message(raw: &[u8], index: usize) -> Result<Value> {
    let m = core::decode_message(raw).map_err(|_| Error("message model capacity".into()))?;
    let mut out = json!({"index":index,"raw_hex":hex(raw),"parse_status":m.parse_status});
    if let (Some(t), Some(v)) = (m.message_type, m.protocol_version) {
        out["header"] = json!({"message_type":cell(t),"protocol_version":cell(v)});
    }
    if let Some(name) = m.body_name {
        let fields: Map<String, Value> = m
            .fields
            .iter()
            .map(|f| (f.name.into(), cell(f.cell)))
            .collect();
        out[name] = Value::Object(fields);
    }
    Ok(out)
}

pub fn decode_unit(
    raw: &[u8],
    is_pack: bool,
    index: usize,
    capture_index: usize,
    capture_offset: usize,
    counter: Option<u8>,
) -> Result<Value> {
    if raw.len() > MAX_PARCEL_BYTES {
        return Err(Error(
            "unit exceeds development limit; retain raw parcel".into(),
        ));
    }
    let u = core::Unit::new(raw, is_pack);
    let mut messages = Vec::new();
    let mut ids = Vec::new();
    // Unsupported pack versions do not authorize even a v2 submessage decode.
    let supported = !is_pack || u.header == Some(0xf2);
    for (i, b) in u.messages().enumerate() {
        let mut m = decode_message(b, i)?;
        if !supported {
            let header = m.get("header").cloned();
            m = json!({"index":i,"raw_hex":hex(b),"parse_status":"unsupported_version"});
            if let Some(h) = header {
                m["header"] = h;
            }
        }
        if let Some(body) = m.get("basic_id").and_then(Value::as_object) {
            let mut id = body.clone();
            id.insert("message_index".into(), json!(i));
            ids.push(Value::Object(id));
        }
        messages.push(m);
    }
    let mut out = json!({"index":index,"capture_index":capture_index,"capture_offset":capture_offset,
        "raw_hex":hex(raw),"kind":if is_pack {"message_pack"} else {"single_message"},
        "messages":messages,"basic_ids":ids,"trailing_hex":hex(u.trailing())});
    if is_pack {
        if let Some(h) = u.header {
            out["header"] = json!({"message_type":cell(core::code(h>>4,if h>>4==15 {"known"} else {"reserved"})),"protocol_version":cell(core::version(h&15))});
        }
        if let Some(s) = u.size {
            let mut c = cell(core::code(
                s,
                if s == 25 { "known" } else { "out_of_range" },
            ));
            c["unit"] = json!("byte");
            out["message_size"] = c;
        }
        if let Some(n) = u.count {
            let mut c = cell(core::code(
                n,
                if (1..=9).contains(&n) {
                    "known"
                } else {
                    "out_of_range"
                },
            ));
            c["unit"] = json!("message");
            out["message_count"] = c;
        }
    }
    if let Some(c) = counter {
        out["message_counter"] = cell(core::code(c, "known"));
    }
    Ok(out)
}
