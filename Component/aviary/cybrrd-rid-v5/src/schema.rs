// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use serde_json::{json, Value};
use std::sync::OnceLock;

pub fn observation() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        serde_json::from_str(include_str!("../contract/observation.schema.json"))
            .expect("pinned schema")
    })
}
pub fn assembly() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../contract/authentication-assembly.schema.json"
        ))
        .expect("pinned schema")
    })
}
pub fn message() -> &'static Value {
    &observation()["properties"]["rid_units"]["items"]["properties"]["messages"]["items"]
}
pub fn unit() -> &'static Value {
    &observation()["properties"]["rid_units"]["items"]
}
pub fn resolved(s: &Value) -> &Value {
    s.get("anyOf")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| x["type"] != "null"))
        .unwrap_or(s)
}
pub fn cell(s: &Value, raw: Value, decoded: Value, status: &str, reason: Option<&str>) -> Value {
    let mut c = json!({"raw":raw,"decoded":decoded,"status":status,
        "unit":s["properties"]["unit"]["const"],"resolution":s["properties"]["resolution"]["const"],"reason":reason});
    if s["properties"].get("arithmetic_value").is_some() {
        c["arithmetic_value"] = c["decoded"].clone();
    }
    c
}
pub fn unavailable(s: &Value, status: &str, reason: &str) -> Value {
    cell(s, Value::Null, Value::Null, status, Some(reason))
}
pub fn known(s: &Value, raw: Value) -> Value {
    cell(s, raw.clone(), raw, "known", None)
}

/// Exact evidence comparison, with a first divergent JSON pointer. This is a
/// verification boundary, not schema validation or a conformance declaration.
pub fn verify_expected(expected: &Value, actual: &Value) -> Result<(), String> {
    fn compare(e: &Value, a: &Value, path: &str) -> Result<(), String> {
        match (e, a) {
            (Value::Object(e), Value::Object(a)) => {
                for (k, v) in e {
                    let p = format!("{}/{}", path, k.replace('~', "~0").replace('/', "~1"));
                    compare(v, a.get(k).ok_or_else(|| format!("missing {p}"))?, &p)?;
                }
                if let Some(k) = a.keys().find(|k| !e.contains_key(*k)) {
                    return Err(format!("unexpected {path}/{k}"));
                }
                Ok(())
            }
            (Value::Array(e), Value::Array(a)) => {
                if e.len() != a.len() {
                    return Err(format!("array length {path}"));
                }
                for (i, (e, a)) in e.iter().zip(a).enumerate() {
                    compare(e, a, &format!("{path}/{i}"))?;
                }
                Ok(())
            }
            _ if e == a => Ok(()),
            _ => Err(format!("value {path}")),
        }
    }
    compare(expected, actual, "")
}
