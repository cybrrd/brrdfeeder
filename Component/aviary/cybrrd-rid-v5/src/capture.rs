// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use crate::{decoder, hex, schema};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObservationId {
    pub node_id: String,
    pub boot_id: String,
    pub capture_sequence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitSpan {
    pub offset: usize,
    pub length: usize,
    pub counter_offset: Option<usize>,
    pub message_pack: bool,
}

/// Exact capture at a declared boundary. A test adapter supplies byte spans;
/// this interface does not discover RF framing or claim hardware exposure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRecord {
    pub observation_id: ObservationId,
    pub source: Value,
    #[serde(with = "hex_bytes")]
    pub bytes: Vec<u8>,
    pub receiver: Map<String, Value>,
    pub transport_kind: String,
    pub transport_fields: Map<String, Value>,
    pub related_captures: Vec<Value>,
    pub format: String,
    pub original_length: Option<usize>,
    pub completeness: String,
    pub unavailable_reason: Option<String>,
    pub units: Vec<UnitSpan>,
    #[serde(default)]
    pub issues: Vec<Value>,
    #[serde(default)]
    pub extensions: Map<String, Value>,
}
pub mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::hex(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        crate::unhex(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Serialize)]
pub struct DecodeResult {
    /// Original interface record survives errors and metadata inconsistencies.
    pub raw_capture: CaptureRecord,
    pub observation: Value,
    pub disposition: String,
    pub diagnostics: Vec<Value>,
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub capture_bytes: usize,
    pub units: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            capture_bytes: 4 * 1024 * 1024,
            units: 256,
        }
    }
}

pub fn decode_capture(record: CaptureRecord) -> DecodeResult {
    decode_with_limits(record, Limits::default())
}
pub fn decode_with_limits(record: CaptureRecord, limits: Limits) -> DecodeResult {
    let mut diagnostics = vec![];
    let mut issue = |code: &str, path: &str, detail: &str| {
        diagnostics.push(json!({"code":code,"path":path,"detail":detail}))
    };
    if record.bytes.len() > limits.capture_bytes || record.units.len() > limits.units {
        issue(
            "parser_budget_exceeded",
            "",
            "Exact input retained without attempting body decoding.",
        );
        return DecodeResult {
            raw_capture: record,
            observation: Value::Null,
            disposition: "quarantine".into(),
            diagnostics,
        };
    }
    let os = schema::observation();
    let rx_shape = &os["properties"]["receiver"]["properties"];
    let mut receiver = record.receiver.clone();
    for (key, s) in rx_shape.as_object().expect("receiver schema") {
        receiver.entry(key.clone()).or_insert_with(|| {
            schema::unavailable(
                s,
                "not_exposed",
                "not supplied by the declared capture interface",
            )
        });
    }
    let exact = hex(&record.bytes);
    let frame_length =
        json!({"captured_bytes":record.bytes.len(),"original_bytes":record.original_length});
    for (key, expected) in [
        ("raw_frame_bytes", json!(exact)),
        ("frame_length", frame_length),
    ] {
        if let Some(c) = record.receiver.get(key) {
            if c["raw"] != expected {
                issue("receiver_capture_mismatch",key,"Supplied metadata disagrees with exact capture; original metadata remains in raw_capture.");
            }
        }
        receiver.insert(key.into(), schema::known(&rx_shape[key], expected));
    }
    let all_fields =
        &os["properties"]["captures"]["items"]["properties"]["transport_fields"]["properties"];
    let known_kind = all_fields.get(&record.transport_kind).is_some();
    if !known_kind && record.transport_kind != "unknown" {
        issue(
            "unsupported_transport_kind",
            "transport_kind",
            "Unknown source kind remains in raw_capture; canonical applicability is unknown.",
        );
    }
    let mut transport_fields = record.transport_fields.clone();
    for (group, shape) in all_fields.as_object().expect("transport schema") {
        let mut fields = transport_fields
            .get(group)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (key, s) in shape["properties"].as_object().expect("transport fields") {
            fields.entry(key.clone()).or_insert_with(|| {
                if known_kind && group != &record.transport_kind {
                    schema::unavailable(s, "not_applicable", "different_transport_frame_kind")
                } else {
                    schema::unavailable(
                        s,
                        "not_exposed",
                        "not supplied by the declared capture interface",
                    )
                }
            });
            let cell = &fields[key];
            if known_kind
                && group != &record.transport_kind
                && (cell["status"] != "not_applicable"
                    || !cell["raw"].is_null()
                    || !cell["decoded"].is_null())
            {
                issue("transport_applicability_mismatch",key,"Opposite transport evidence is preserved but cannot claim valid applicability.");
            }
        }
        transport_fields.insert(group.clone(), json!(fields));
    }
    let network: Map<_, _> = os["properties"]["network_information"]["properties"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, s)| {
            (
                k.clone(),
                schema::unavailable(s, "not_applicable", "not_applicable_to_broadcast"),
            )
        })
        .collect();
    let mut units = vec![];
    for (i, span) in record.units.iter().enumerate() {
        let end = span.offset.checked_add(span.length);
        let Some(bytes) = end.and_then(|end| record.bytes.get(span.offset..end)) else {
            issue(
                "invalid_unit_span",
                &format!("units[{i}]"),
                "Byte span lies outside exact capture; no fabricated bytes.",
            );
            continue;
        };
        let counter = span
            .counter_offset
            .and_then(|o| record.bytes.get(o))
            .copied();
        if span.counter_offset.is_some() && counter.is_none() {
            issue(
                "invalid_counter_span",
                &format!("units[{i}]"),
                "Transport counter is outside capture.",
            );
        }
        units.push(decoder::decode_unit(
            bytes,
            i,
            span.offset,
            counter,
            span.message_pack,
        ));
    }
    let id = json!({"observation_id":record.observation_id,"capture_index":0});
    let capture = json!({"index":0,"capture_id":id,"transport_kind":if known_kind{record.transport_kind.as_str()}else{"unknown"},"related_captures":record.related_captures,
        "format":record.format,"raw_hex":exact,"captured_length":record.bytes.len(),"original_length":record.original_length,
        "completeness":record.completeness,"unavailable_reason":record.unavailable_reason,"transport_fields":transport_fields});
    let observation = json!({"wire_format_version":5,"schema_revision":0,"observation_id":record.observation_id,"source":record.source,
        "receiver":receiver,"network_information":network,"captures":[capture],"rid_units":units,"issues":record.issues,"extensions":record.extensions});
    for (i, u) in units.iter().enumerate() {
        if u["kind"] == "malformed" {
            diagnostics.push(json!({"code":"malformed_pack","path":format!("rid_units[{i}]"),"detail":"Incomplete pack header retained."}));
        }
        if u["kind"] == "message_pack" {
            let size = u["message_size"]["raw"].as_u64();
            let count = u["message_count"]["raw"].as_u64();
            if size != Some(25)
                || !count.is_some_and(|n| (1..=9).contains(&n))
                || count != u["messages"].as_array().map(|m| m.len() as u64)
                || u["trailing_hex"] != ""
                || u["header"]["protocol_version"]["raw"] != 2
            {
                diagnostics.push(json!({"code":"invalid_pack_shape","path":format!("rid_units[{i}]"),"detail":"Declared size/count/version or trailing bytes are nonconforming; evidence retained."}));
            }
        }
        for (j, m) in u["messages"].as_array().unwrap().iter().enumerate() {
            diagnostics.extend(decoder::message_issues(
                m,
                &format!("rid_units[{i}].messages[{j}]"),
            ));
        }
    }
    let disposition = if diagnostics.is_empty() {
        "decoded"
    } else {
        "quarantine"
    }
    .into();
    DecodeResult {
        raw_capture: record,
        observation,
        disposition,
        diagnostics,
    }
}

/// Convert a synthetic contract observation to a raw-interface input. Expected
/// message bodies and Basic ID indexes are deliberately not used as inputs.
pub fn fixture_input(v: &Value) -> Result<CaptureRecord, String> {
    let c = v["captures"]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or("one capture required")?
        .first()
        .unwrap();
    let bytes = crate::unhex(c["raw_hex"].as_str().ok_or("capture raw hex")?)?;
    let mut units = vec![];
    for u in v["rid_units"].as_array().ok_or("units")? {
        let offset = u["capture_offset"].as_u64().ok_or("offset")? as usize;
        units.push(UnitSpan {
            offset,
            length: u["raw_hex"].as_str().ok_or("unit raw")?.len() / 2,
            counter_offset: if u.get("message_counter").is_some() {
                offset.checked_sub(1)
            } else {
                None
            },
            message_pack: u["kind"] == "message_pack",
        });
    }
    Ok(CaptureRecord {
        observation_id: serde_json::from_value(v["observation_id"].clone())
            .map_err(|e| e.to_string())?,
        source: v["source"].clone(),
        bytes,
        receiver: v["receiver"].as_object().ok_or("receiver")?.clone(),
        transport_kind: c["transport_kind"].as_str().ok_or("kind")?.into(),
        transport_fields: c["transport_fields"]
            .as_object()
            .ok_or("transport fields")?
            .clone(),
        related_captures: c["related_captures"]
            .as_array()
            .ok_or("relationships")?
            .clone(),
        format: c["format"].as_str().ok_or("format")?.into(),
        original_length: c["original_length"].as_u64().map(|v| v as usize),
        completeness: c["completeness"].as_str().ok_or("completeness")?.into(),
        unavailable_reason: c["unavailable_reason"].as_str().map(str::to_owned),
        units,
        issues: v["issues"].as_array().ok_or("issues")?.clone(),
        extensions: v["extensions"].as_object().ok_or("extensions")?.clone(),
    })
}
