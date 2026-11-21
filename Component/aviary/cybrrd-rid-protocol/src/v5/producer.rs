// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Ledger-first development capture and version-aware test ingress. No sockets.
use super::{codec, decode_unit, hex, model::Observation, schema, Error, Result, MAX_PARCEL_BYTES};
use serde_json::{json, Map, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub identity: Value,
    pub boundary: String,
    pub metadata: Value,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub ledger_record: String,
}
/// Implementations must append the entire event atomically or return an error.
/// This interface does not assert durability of a particular implementation.
pub trait RawLedger {
    fn append(&mut self, event: &Event) -> Result<Receipt>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Interpretation {
    V5(Observation),
    V4(Value),
    HistoricUnversioned(Value),
    Unsupported { reason: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retained {
    pub receipt: Receipt,
    pub event: Event,
    pub interpretation: Interpretation,
}
#[derive(Clone, Copy, Debug)]
pub enum Media {
    Json,
    CborRevision0,
}
pub fn test_ingress(ledger: &mut impl RawLedger, event: Event, media: Media) -> Result<Retained> {
    let receipt = ledger.append(&event)?;
    let parsed = match media {
        Media::Json => codec::parse_json(&event.bytes),
        Media::CborRevision0 => codec::parse_cbor(&event.bytes, schema::contract()?),
    };
    let interpretation = match parsed {
        Err(e) => Interpretation::Unsupported {
            reason: e.to_string(),
        },
        Ok(v) => match v.get("wire_format_version") {
            None if v.is_object() => Interpretation::HistoricUnversioned(v),
            Some(n) if n == 4 => Interpretation::V4(v),
            Some(n) if n == 5 => match Observation::new(v) {
                Ok(o) => Interpretation::V5(o),
                Err(e) => Interpretation::Unsupported {
                    reason: e.to_string(),
                },
            },
            _ => Interpretation::Unsupported {
                reason: "unknown major or envelope shape".into(),
            },
        },
    };
    Ok(Retained {
        receipt,
        event,
        interpretation,
    })
}

#[derive(Clone, Debug)]
pub struct UnitSpan {
    pub offset: usize,
    pub length: usize,
    pub is_pack: bool,
    pub counter: Option<u8>,
}
#[derive(Clone, Debug)]
pub struct Capture {
    pub event: Event,
    pub format: String,
    pub source_kind: String,
    pub adapter_version: String,
    pub completeness: String,
    pub original_length: Option<usize>,
    pub unavailable_reason: Option<String>,
    pub transport_kind: String,
    pub spans: Vec<UnitSpan>,
    /// Supplied cells must preserve the raw metadata supplied at the boundary.
    pub receiver_cells: Map<String, Value>,
    pub transport_cells: Map<String, Value>,
    pub related_captures: Vec<Value>,
}
fn unavailable(s: &Value, status: &str, reason: &str) -> Value {
    json!({"raw":null,"decoded":null,"status":status,"unit":s["properties"]["unit"]["const"],
        "resolution":s["properties"]["resolution"]["const"],"reason":reason})
}
fn exposed(s: &Value, v: Value) -> Value {
    json!({"raw":v,"decoded":v,"status":"known","unit":s["properties"]["unit"]["const"],
        "resolution":s["properties"]["resolution"]["const"],"reason":null})
}
fn measured(cells: &mut Map<String, Value>, name: &str, expected: Value) -> Result<()> {
    if let Some(supplied) = cells.get(name) {
        for member in ["raw", "decoded", "unit", "resolution"] {
            if supplied[member] != expected[member] {
                return Err(Error(format!(
                    "conflicting supplied {name}.{member}; original event retained"
                )));
            }
        }
    } else {
        cells.insert(name.into(), expected);
    }
    Ok(())
}
fn properties(s: &Value) -> Result<&Map<String, Value>> {
    s.get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| Error("contract properties".into()))
}
pub fn issue(code: &str, path: &str, detail: &str) -> Value {
    json!({"code":code,"path":path,"detail":detail})
}
pub(super) fn build(c: &Capture) -> Result<Observation> {
    if c.event.bytes.len() > MAX_PARCEL_BYTES {
        return Err(Error(
            "capture exceeds development interpretation limit".into(),
        ));
    }
    let s = schema::contract()?;
    let receiver_schema = &s["properties"]["receiver"];
    let mut receiver = Map::new();
    for (k, field) in properties(receiver_schema)? {
        receiver.insert(
            k.clone(),
            unavailable(
                field,
                "not_exposed",
                "not supplied at this development adapter boundary",
            ),
        );
    }
    for (k, v) in &c.receiver_cells {
        receiver.insert(k.clone(), v.clone());
    }
    // Replace generated unavailable placeholders, never contradictory input.
    for name in ["raw_frame_bytes", "frame_length", "node_id", "transport"] {
        if !c.receiver_cells.contains_key(name) {
            receiver.remove(name);
        }
    }
    measured(
        &mut receiver,
        "raw_frame_bytes",
        exposed(
            &receiver_schema["properties"]["raw_frame_bytes"],
            json!(hex(&c.event.bytes)),
        ),
    )?;
    measured(
        &mut receiver,
        "frame_length",
        exposed(
            &receiver_schema["properties"]["frame_length"],
            json!({"captured_bytes":c.event.bytes.len(),"original_bytes":c.original_length}),
        ),
    )?;
    measured(
        &mut receiver,
        "node_id",
        exposed(
            &receiver_schema["properties"]["node_id"],
            c.event.identity["node_id"].clone(),
        ),
    )?;
    let transport = match c.transport_kind.as_str() {
        "bt5_primary" | "bt5_secondary" => "bt5_long_range",
        "wifi_nan_sync" | "wifi_nan_sdf" => "wifi_nan",
        x => x,
    };
    measured(
        &mut receiver,
        "transport",
        exposed(
            &receiver_schema["properties"]["transport"],
            json!(transport),
        ),
    )?;
    if !c.receiver_cells.contains_key("parser_version") {
        receiver.insert(
            "parser_version".into(),
            exposed(
                &receiver_schema["properties"]["parser_version"],
                json!("wire-v5-p0-rev0"),
            ),
        );
    }
    let capture_schema = &s["properties"]["captures"]["items"];
    let mut groups = Map::new();
    for (k, group) in properties(&capture_schema["properties"]["transport_fields"])? {
        let active = k == &c.transport_kind;
        let mut fields = Map::new();
        for (name, field) in properties(group)? {
            fields.insert(
                name.clone(),
                unavailable(
                    field,
                    if active || c.transport_kind == "unknown" {
                        "not_exposed"
                    } else {
                        "not_applicable"
                    },
                    if active || c.transport_kind == "unknown" {
                        "not supplied at this development adapter boundary"
                    } else {
                        "different transport frame kind"
                    },
                ),
            );
        }
        if active {
            for (k, v) in &c.transport_cells {
                fields.insert(k.clone(), v.clone());
            }
        }
        groups.insert(k.clone(), Value::Object(fields));
    }
    let mut network = Map::new();
    for (k, field) in properties(&s["properties"]["network_information"])? {
        network.insert(
            k.clone(),
            unavailable(field, "not_applicable", "not_applicable_to_broadcast"),
        );
    }
    let mut units = Vec::new();
    let mut issues = Vec::new();
    // Preserve the entire adapter buffer, including bytes outside RID spans.
    for (i, span) in c.spans.iter().enumerate() {
        let end = span
            .offset
            .checked_add(span.length)
            .ok_or_else(|| Error("span overflow".into()))?;
        let raw = c
            .event
            .bytes
            .get(span.offset..end)
            .ok_or_else(|| Error("span outside capture; exact capture retained".into()))?;
        let payload_name = match c.transport_kind.as_str() {
            "bt4_legacy" => Some("odid_message"),
            "bt5_secondary" => Some("odid_message_pack"),
            "wifi_nan_sdf" => Some("service_info_message_pack"),
            "wifi_beacon" => Some("message_pack"),
            _ => None,
        };
        if let Some(payload_name) = payload_name {
            let active = groups
                .get_mut(&c.transport_kind)
                .and_then(Value::as_object_mut)
                .ok_or_else(|| Error("active transport group".into()))?;
            let field_schema = &capture_schema["properties"]["transport_fields"]["properties"]
                [&c.transport_kind]["properties"];
            let mut exposed_fields = vec![(payload_name, json!(hex(raw)))];
            if let Some(counter) = span.counter {
                exposed_fields.push(("message_counter", json!(counter)));
            }
            for (name, value) in exposed_fields {
                let expected = exposed(&field_schema[name], value);
                if c.spans.len() == 1 && c.transport_cells.contains_key(name) {
                    measured(active, name, expected)?;
                } else if !c.transport_cells.contains_key(name) {
                    if i == 0 {
                        active.insert(name.into(), expected);
                    }
                    if c.spans.len() > 1 {
                        if let Some(cell) = active.get_mut(name) {
                            cell["status"] = json!("ambiguous");
                            cell["reason"] = json!("all ordered instances retained in rid_units");
                        }
                    }
                }
            }
        }
        let u = decode_unit(raw, span.is_pack, i, 0, span.offset, span.counter)?;
        if !cybrrd_rid_core::v5::Unit::new(raw, span.is_pack).conformant_framing() {
            issues.push(issue(
                "nonconforming_framing",
                &format!("rid_units[{i}]"),
                "declared framing retained without correction",
            ));
        }
        if let Some(messages) = u["messages"].as_array() {
            for (j, m) in messages.iter().enumerate() {
                let path = format!("rid_units[{i}].messages[{j}]");
                if m["parse_status"] != "complete" {
                    issues.push(issue(
                        "uninterpreted_message",
                        &path,
                        "exact message retained",
                    ));
                }
                if m.pointer("/location/timestamp/raw") == Some(&json!(36000)) {
                    issues.push(issue(
                        "hour_boundary",
                        &path,
                        "timestamp is the UTC-hour endpoint; no epoch inferred",
                    ));
                }
                if m.pointer("/system/classification_type/raw")
                    .is_some_and(|v| v != 1)
                    && (m.pointer("/system/category/raw").is_some_and(|v| v != 0)
                        || m.pointer("/system/class/raw").is_some_and(|v| v != 0))
                {
                    issues.push(issue(
                        "classification_inconsistency",
                        &path,
                        "category/class with a different classification retained",
                    ));
                }
            }
        }
        units.push(u);
    }
    let record = json!({"wire_format_version":5,"schema_revision":0,"observation_id":c.event.identity,
        "source":{"adapter":c.event.boundary,"adapter_version":c.adapter_version,"kind":c.source_kind},
        "receiver":receiver,"network_information":network,
        "captures":[{"index":0,"capture_id":{"observation_id":c.event.identity,"capture_index":0},
            "transport_kind":c.transport_kind,"related_captures":c.related_captures,"format":c.format,
            "raw_hex":hex(&c.event.bytes),"captured_length":c.event.bytes.len(),"original_length":c.original_length,
            "completeness":c.completeness,"unavailable_reason":c.unavailable_reason,"transport_fields":groups}],
        "rid_units":units,"issues":issues,"extensions":{"development_only":true,"adapter_metadata":c.event.metadata}});
    Observation::new(record)
}
pub fn capture(ledger: &mut impl RawLedger, c: Capture) -> Result<Retained> {
    capture_with(ledger, c, |_| Ok(()))
}
pub(super) fn capture_with(
    ledger: &mut impl RawLedger,
    mut c: Capture,
    extract: impl FnOnce(&mut Capture) -> Result<()>,
) -> Result<Retained> {
    c.event.metadata = json!({"source_metadata":c.event.metadata,"transport_kind":c.transport_kind,"capture_format":c.format,
        "source_kind":c.source_kind,"adapter_version":c.adapter_version,"completeness":c.completeness,
        "original_length":c.original_length,"unavailable_reason":c.unavailable_reason,
        "receiver_cells":c.receiver_cells,"transport_cells":c.transport_cells,"related_captures":c.related_captures,
        "unit_spans":c.spans.iter().map(|s|json!({"offset":s.offset,"length":s.length,"is_pack":s.is_pack,"counter":s.counter})).collect::<Vec<_>>()});
    // Append metadata and bytes before reading a header, validating or deduping.
    let receipt = ledger.append(&c.event)?;
    let interpretation = match extract(&mut c).and_then(|()| build(&c)) {
        Ok(o) => Interpretation::V5(o),
        Err(e) => Interpretation::Unsupported {
            reason: e.to_string(),
        },
    };
    Ok(Retained {
        receipt,
        event: c.event,
        interpretation,
    })
}
