// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use crate::{
    hex,
    numeric::{self, Number},
    schema,
};
use serde_json::{json, Map, Value};

struct Body {
    shape: &'static Value,
    fields: Map<String, Value>,
}
impl Body {
    fn new(name: &str) -> Self {
        Self {
            shape: &schema::message()["properties"][name],
            fields: Map::new(),
        }
    }
    fn put(
        &mut self,
        key: &str,
        raw: impl Into<Value>,
        decoded: impl Into<Value>,
        status: &str,
        reason: Option<&str>,
    ) {
        self.fields.insert(
            key.into(),
            schema::cell(
                &self.shape["properties"][key],
                raw.into(),
                decoded.into(),
                status,
                reason,
            ),
        );
    }
    fn known(&mut self, key: &str, raw: impl Into<Value>) {
        let raw = raw.into();
        self.put(key, raw.clone(), raw, "known", None);
    }
    fn number(&mut self, key: &str, raw: impl Into<Value>, n: Number) {
        self.put(key, raw, n.decimal(), n.status, n.reason);
    }
    fn enumeration(
        &mut self,
        key: &str,
        raw: u8,
        unknown_zero: bool,
        reserved: std::ops::RangeInclusive<u8>,
        private: std::ops::RangeInclusive<u8>,
    ) {
        let status = if private.contains(&raw) {
            "private_use"
        } else if reserved.contains(&raw) {
            "reserved"
        } else if unknown_zero && raw == 0 {
            "unknown"
        } else {
            "known"
        };
        self.put(key, raw, raw, status, None);
    }
    fn reserved(&mut self, key: &str, raw: impl Into<Value>) {
        let raw = raw.into();
        self.put(key, raw.clone(), raw, "reserved", None);
    }
    fn text(&mut self, key: &str, bytes: &[u8], empty_reason: &str) {
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let prefix = &bytes[..end];
        let (decoded, status, reason) = if bytes.iter().all(|&b| b == 0) {
            (Some(String::new()), "unknown", Some(empty_reason))
        } else if prefix.iter().any(|&b| !(32..=126).contains(&b)) {
            (None, "out_of_range", Some("invalid_ascii_text"))
        } else if end < bytes.len() && bytes[end..].iter().any(|&b| b != 0) {
            (
                Some(String::from_utf8_lossy(prefix).into_owned()),
                "out_of_range",
                Some("nonzero_after_nul"),
            )
        } else if prefix.iter().all(|&b| b == b' ') {
            (
                Some(String::from_utf8_lossy(prefix).into_owned()),
                "unknown",
                Some("whitespace_only_text"),
            )
        } else {
            (
                Some(String::from_utf8_lossy(prefix).into_owned()),
                "known",
                None,
            )
        };
        self.put(key, hex(bytes), json!(decoded), status, reason);
    }
    fn na(&mut self, key: &str) {
        self.fields.insert(
            key.into(),
            schema::unavailable(
                &self.shape["properties"][key],
                "not_applicable",
                "opposite_authentication_page_shape",
            ),
        );
    }
    fn finish(self) -> Value {
        Value::Object(self.fields)
    }
}

pub fn header(byte: u8) -> Value {
    let s = &schema::message()["properties"]["header"]["properties"];
    json!({"message_type":schema::known(&s["message_type"],json!(byte>>4)),
        "protocol_version":schema::cell(&s["protocol_version"],json!(byte&15),json!(byte&15),
        match byte&15{2=>"known",15=>"private_use",_=>"unsupported_version"},None)})
}

/// Decode exactly one message boundary. Partial/oversized/unknown messages
/// retain every byte and never acquire padded or speculative body fields.
pub fn decode_message(bytes: &[u8], index: usize) -> Value {
    let mut out = json!({"index":index,"raw_hex":hex(bytes),"parse_status":"truncated"});
    let Some(&first) = bytes.first() else {
        return out;
    };
    out["header"] = header(first);
    if first & 15 != 2 {
        out["parse_status"] = json!("unsupported_version");
        return out;
    }
    if bytes.len() != 25 {
        out["parse_status"] = json!(if bytes.len() < 25 {
            "truncated"
        } else {
            "invalid_length"
        });
        return out;
    }
    let u16_at = |o| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32_at = |o| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let i32_at = |o| i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let kind = first >> 4;
    let name = match kind {
        0 => "basic_id",
        1 => "location",
        2 => "auth",
        3 => "self_id",
        4 => "system",
        5 => "operator_id",
        _ => {
            out["parse_status"] = json!("unsupported_type");
            return out;
        }
    };
    let mut b = Body::new(name);
    // Empty ranges are represented by a singleton outside the nibble domain.
    match kind {
        0 => {
            let id = bytes[1] >> 4;
            let raw = &bytes[2..22];
            let data = hex(raw);
            b.enumeration("id_type", id, true, 5..=15, 255..=255);
            b.enumeration("ua_type", bytes[1] & 15, true, 255..=255, 255..=255);
            b.reserved("reserved", hex(&bytes[22..]));
            if id == 1 || id == 2 {
                b.text("uas_id", raw, "empty_identifier");
            } else {
                let empty = raw.iter().all(|&x| x == 0);
                let (status, reason) = if id == 0 {
                    (
                        "unknown",
                        Some(if empty {
                            "id_type_none_empty"
                        } else {
                            "id_type_none_with_bytes"
                        }),
                    )
                } else if empty {
                    ("unknown", Some("empty_identifier"))
                } else if id >= 5 {
                    ("reserved", Some("reserved_id_type_opaque_bytes"))
                } else if id == 4 {
                    match raw[0] {
                        0 => ("out_of_range", Some("session_method_reserved")),
                        225..=255 => ("private_use", Some("session_method_private")),
                        _ => ("known", Some("session_method_profile_required")),
                    }
                } else {
                    ("known", None)
                };
                b.put("uas_id", data.clone(), data, status, reason);
            }
        }
        1 => {
            let flags = bytes[1];
            b.enumeration("operational_status", flags >> 4, true, 5..=15, 255..=255);
            b.reserved("status_reserved", (flags >> 3) & 1);
            b.known("height_type", (flags >> 2) & 1);
            b.known("ew_direction_segment", (flags >> 1) & 1);
            b.known("speed_multiplier", flags & 1);
            b.number(
                "track_direction",
                bytes[2],
                numeric::direction(bytes[2], flags & 2 != 0),
            );
            b.number("speed", bytes[3], numeric::speed(bytes[3], flags & 1 != 0));
            b.number(
                "vertical_speed",
                i64::from(bytes[4] as i8),
                numeric::vertical(bytes[4] as i8),
            );
            b.number(
                "latitude",
                i32_at(5),
                numeric::coordinate(i32_at(5), i32_at(9), true),
            );
            b.number(
                "longitude",
                i32_at(9),
                numeric::coordinate(i32_at(9), i32_at(5), false),
            );
            for (key, offset) in [
                ("pressure_altitude", 13),
                ("geodetic_altitude_hae", 15),
                ("height", 17),
            ] {
                b.number(key, u16_at(offset), numeric::altitude(u16_at(offset)));
            }
            for (key, raw, max) in [
                ("vertical_accuracy", bytes[19] >> 4, 6),
                ("horizontal_accuracy", bytes[19] & 15, 12),
                ("baro_altitude_accuracy", bytes[20] >> 4, 6),
                ("speed_accuracy", bytes[20] & 15, 4),
            ] {
                b.put(key, raw, raw, numeric::accuracy_status(raw, max), None);
            }
            b.number("timestamp", u16_at(21), numeric::hour_timestamp(u16_at(21)));
            b.number(
                "timestamp_accuracy",
                bytes[23] & 15,
                numeric::timestamp_accuracy(bytes[23] & 15),
            );
            b.reserved("timestamp_reserved", bytes[23] >> 4);
            b.reserved("reserved", hex(&bytes[24..]));
        }
        2 => {
            let typ = bytes[1] >> 4;
            let page = bytes[1] & 15;
            b.enumeration("auth_type", typ, true, 6..=9, 10..=15);
            b.known("data_page_number", page);
            let (key, data) = if page == 0 {
                b.reserved("page0_reserved", bytes[2] >> 4);
                b.known("last_page", bytes[2] & 15);
                b.known("length", bytes[3]);
                b.number("timestamp", u32_at(4), numeric::epoch_timestamp(u32_at(4)));
                b.na("auth_data_pages_1_15");
                ("auth_data_page0", &bytes[8..])
            } else {
                for k in [
                    "page0_reserved",
                    "last_page",
                    "length",
                    "timestamp",
                    "auth_data_page0",
                ] {
                    b.na(k);
                }
                ("auth_data_pages_1_15", &bytes[2..])
            };
            let zero = data.iter().all(|&v| v == 0);
            let (status, reason) = match (typ, zero) {
                (0, true) => ("unknown", Some("auth_type_none_empty")),
                (0, false) => ("out_of_range", Some("auth_type_none_with_data")),
                (4, true) => ("known", Some("network_auth_requires_null_data")),
                (4, false) => ("out_of_range", Some("network_auth_nonzero_data")),
                _ => ("known", None),
            };
            b.put(key, hex(data), hex(data), status, reason);
        }
        3 => {
            b.enumeration("description_type", bytes[1], false, 3..=200, 201..=255);
            b.text("description", &bytes[2..], "empty_description");
        }
        4 => {
            b.reserved("flags_reserved", bytes[1] >> 5);
            b.enumeration(
                "classification_type",
                (bytes[1] >> 2) & 7,
                true,
                2..=7,
                255..=255,
            );
            b.enumeration(
                "operator_location_altitude_source_type",
                bytes[1] & 3,
                false,
                3..=3,
                255..=255,
            );
            b.number(
                "operator_latitude",
                i32_at(2),
                numeric::coordinate(i32_at(2), i32_at(6), true),
            );
            b.number(
                "operator_longitude",
                i32_at(6),
                numeric::coordinate(i32_at(6), i32_at(2), false),
            );
            b.put(
                "area_count",
                u16_at(10),
                u16_at(10),
                if u16_at(10) > 65000 {
                    "out_of_range"
                } else {
                    "known"
                },
                None,
            );
            b.put(
                "area_radius",
                bytes[12],
                (u16::from(bytes[12]) * 10).to_string(),
                if bytes[12] > 250 {
                    "out_of_range"
                } else {
                    "known"
                },
                None,
            );
            for (key, offset) in [
                ("area_ceiling_hae", 13),
                ("area_floor_hae", 15),
                ("operator_altitude_hae", 18),
            ] {
                b.number(key, u16_at(offset), numeric::altitude(u16_at(offset)));
            }
            b.enumeration("category", bytes[17] >> 4, true, 4..=15, 255..=255);
            b.enumeration("class", bytes[17] & 15, true, 8..=15, 255..=255);
            b.number(
                "timestamp",
                u32_at(20),
                numeric::epoch_timestamp(u32_at(20)),
            );
            b.reserved("reserved", hex(&bytes[24..]));
        }
        5 => {
            b.enumeration("operator_id_type", bytes[1], false, 1..=200, 201..=255);
            b.text("operator_id", &bytes[2..22], "empty_operator_identifier");
            b.reserved("reserved", hex(&bytes[22..]));
        }
        _ => unreachable!(),
    }
    out[name] = b.finish();
    out["parse_status"] = json!("complete");
    out
}

/// Issues are separate from field cells: complete extraction is not conformance.
pub fn message_issues(m: &Value, path: &str) -> Vec<Value> {
    let mut out = vec![];
    let mut issue = |code: &str, field: &str, detail: &str| {
        out.push(json!({"code":code,"path":format!("{path}{field}"),"detail":detail}))
    };
    if m["parse_status"] != "complete" {
        issue(
            "message_not_decoded",
            "",
            "Exact message retained; body is not interpreted under this profile.",
        );
        return out;
    }
    if m["location"]["timestamp"]["raw"] == 36000 {
        issue(
            "timestamp_hour_boundary",
            ".location.timestamp",
            "Raw 36000 is retained at the UTC-hour boundary.",
        );
    }
    if let Some(s) = m.get("system") {
        if s["classification_type"]["raw"] != 1
            && (s["category"]["raw"] != 0 || s["class"]["raw"] != 0)
        {
            issue(
                "classification_inconsistent",
                ".system",
                "Category/class are present outside the EU classification profile.",
            );
        }
    }
    for body in [
        "basic_id",
        "location",
        "auth",
        "self_id",
        "system",
        "operator_id",
    ] {
        if let Some(fields) = m[body].as_object() {
            for (k, c) in fields {
                if k.contains("reserved") && c["status"] != "not_applicable" {
                    let nonzero = c["raw"]
                        .as_i64()
                        .map(|v| v != 0)
                        .or_else(|| c["raw"].as_str().map(|s| s.bytes().any(|v| v != b'0')))
                        .unwrap_or(false);
                    if nonzero {
                        issue(
                            "reserved_bits_nonzero",
                            &format!(".{body}.{k}"),
                            "Reserved bytes/bits are retained without correction.",
                        );
                    }
                }
            }
        }
    }
    out
}

pub fn decode_unit(
    bytes: &[u8],
    index: usize,
    offset: usize,
    counter: Option<u8>,
    pack: bool,
) -> Value {
    let mut out = json!({"index":index,"capture_index":0,"capture_offset":offset,"raw_hex":hex(bytes),"kind":if pack{"message_pack"}else{"single_message"},"messages":[],"basic_ids":[],"trailing_hex":""});
    if let Some(c) = counter {
        out["message_counter"] =
            schema::known(&schema::unit()["properties"]["message_counter"], json!(c));
    }
    let mut messages = vec![];
    if !pack {
        messages.push(decode_message(bytes, 0));
    } else {
        if let Some(&first) = bytes.first() {
            out["header"] = header(first);
        }
        if let Some(&size) = bytes.get(1) {
            out["message_size"] =
                schema::known(&schema::unit()["properties"]["message_size"], json!(size));
        }
        if let Some(&count) = bytes.get(2) {
            out["message_count"] =
                schema::known(&schema::unit()["properties"]["message_count"], json!(count));
        }
        if bytes.len() < 3 {
            out["kind"] = json!("malformed");
        } else if bytes[0] != 0xf2 {
            out["trailing_hex"] = json!(hex(&bytes[3..]));
        } else {
            let size = usize::from(bytes[1]);
            let count = usize::from(bytes[2]);
            let mut start = 3;
            if size != 0 {
                for i in 0..count {
                    if start >= bytes.len() {
                        break;
                    }
                    let end = (start + size).min(bytes.len());
                    let mut m = decode_message(&bytes[start..end], i);
                    if size != 25 {
                        m = json!({"index":i,"raw_hex":hex(&bytes[start..end]),"parse_status":"invalid_declared_size","header":header(bytes[start])});
                    }
                    messages.push(m);
                    start = end;
                }
            }
            out["trailing_hex"] = json!(hex(&bytes[start..]));
        }
    }
    let ids: Vec<_> = messages
        .iter()
        .filter_map(|m| {
            m.get("basic_id").map(|b| {
                let mut id = b.clone();
                id["message_index"] = m["index"].clone();
                id
            })
        })
        .collect();
    out["messages"] = json!(messages);
    out["basic_ids"] = json!(ids);
    out
}
