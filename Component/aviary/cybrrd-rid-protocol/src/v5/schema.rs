// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Validator for the pinned contract's deliberately bounded schema vocabulary.
use super::{Error, Result, MAX_DEPTH};
use serde_json::Value;
use std::sync::OnceLock;

pub fn contract() -> Result<&'static Value> {
    static S: OnceLock<std::result::Result<Value, String>> = OnceLock::new();
    match S.get_or_init(|| {
        serde_json::from_str(include_str!("contract/observation.schema.json"))
            .map_err(|e| e.to_string())
    }) {
        Ok(v) => Ok(v),
        Err(e) => Err(Error(e.clone())),
    }
}
pub fn message_contract() -> Result<&'static Value> {
    contract()?
        .pointer("/properties/rid_units/items/properties/messages/items")
        .ok_or_else(|| Error("missing message schema".into()))
}
pub fn is_hex_schema(s: &Value) -> bool {
    s.get("pattern").and_then(Value::as_str) == Some("^(?:[0-9a-f]{2})*$")
        || s.get("anyOf")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(is_hex_schema))
}
pub fn child<'a>(s: &'a Value, key: &str) -> Option<&'a Value> {
    s.get("properties").and_then(|p| p.get(key)).or_else(|| {
        s.get("anyOf")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find_map(|s| child(s, key)))
    })
}
pub fn item(s: &Value) -> Option<&Value> {
    s.get("items").or_else(|| {
        s.get("anyOf")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find_map(item))
    })
}

pub fn canonical_decimal(s: &str) -> bool {
    if s == "0" {
        return true;
    }
    let unsigned = s.strip_prefix('-').unwrap_or(s);
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (unsigned, None),
    };
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
    {
        return false;
    }
    if let Some(f) = fraction {
        !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()) && !f.ends_with('0')
    } else {
        whole != "0"
    }
}
fn pattern(s: &str, p: &str) -> bool {
    match p {
        "^(?:[0-9a-f]{2})*$" => {
            s.len() % 2 == 0
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        "^(?:0|[1-9][0-9]*)$" => {
            s == "0"
                || (!s.is_empty() && !s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit()))
        }
        "^(?:0|-?[1-9][0-9]*)(?:\\.[0-9]*[1-9])?$|^-0\\.[0-9]*[1-9]$" => canonical_decimal(s),
        _ => false,
    }
}
pub fn validate(v: &Value, s: &Value) -> Result<()> {
    check(v, s, "$", 0)
}
fn check(v: &Value, s: &Value, path: &str, depth: usize) -> Result<()> {
    let fail = |rule: &str| Error(format!("{path}: {rule}"));
    if depth > MAX_DEPTH {
        return Err(fail("schema depth"));
    }
    if s == &Value::Bool(true) {
        return Ok(());
    }
    if s == &Value::Bool(false) {
        return Err(fail("false schema"));
    }
    if let Some(t) = s.get("type").and_then(Value::as_str) {
        let ok = match t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "integer" => v.as_i64().is_some() || v.as_u64().is_some(),
            "null" => v.is_null(),
            "boolean" => v.is_boolean(),
            _ => false,
        };
        if !ok {
            return Err(fail(t));
        }
    }
    if let Some(c) = s.get("const") {
        if v != c {
            return Err(fail("const"));
        }
    }
    if let Some(a) = s.get("enum").and_then(Value::as_array) {
        if !a.contains(v) {
            return Err(fail("enum"));
        }
    }
    if let Some(a) = s.get("anyOf").and_then(Value::as_array) {
        if !a.iter().any(|sub| check(v, sub, path, depth + 1).is_ok()) {
            return Err(fail("anyOf"));
        }
    }
    if let Some(a) = s.get("allOf").and_then(Value::as_array) {
        for sub in a {
            check(v, sub, path, depth + 1)?;
        }
    }
    if let Some(n) = s.get("not") {
        if check(v, n, path, depth + 1).is_ok() {
            return Err(fail("not"));
        }
    }
    if let Some(i) = s.get("if") {
        let branch = if check(v, i, path, depth + 1).is_ok() {
            "then"
        } else {
            "else"
        };
        if let Some(b) = s.get(branch) {
            check(v, b, path, depth + 1)?;
        }
    }
    if let Some(o) = v.as_object() {
        if let Some(req) = s.get("required").and_then(Value::as_array) {
            for k in req {
                let k = k.as_str().ok_or_else(|| fail("schema required"))?;
                if !o.contains_key(k) {
                    return Err(fail(&format!("missing {k}")));
                }
            }
        }
        for (k, x) in o {
            if let Some(sub) = child(s, k) {
                check(x, sub, &format!("{path}.{k}"), depth + 1)?;
            } else if s.get("additionalProperties") == Some(&Value::Bool(false)) {
                return Err(fail("additional property"));
            }
        }
    }
    if let Some(a) = v.as_array() {
        if s.get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|n| a.len() < (n as usize))
            || s.get("maxItems")
                .and_then(Value::as_u64)
                .is_some_and(|n| a.len() > (n as usize))
        {
            return Err(fail("array length"));
        }
        if let Some(sub) = item(s) {
            for (i, x) in a.iter().enumerate() {
                check(x, sub, &format!("{path}[{i}]"), depth + 1)?;
            }
        }
    }
    if let Some(text) = v.as_str() {
        let len = text.chars().count();
        if s.get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|n| len < (n as usize))
            || s.get("maxLength")
                .and_then(Value::as_u64)
                .is_some_and(|n| len > (n as usize))
        {
            return Err(fail("string length"));
        }
        if let Some(p) = s.get("pattern").and_then(Value::as_str) {
            if !pattern(text, p) {
                return Err(fail("pattern"));
            }
        }
    }
    if let Some(n) = v.as_i64() {
        if s.get("minimum")
            .and_then(Value::as_i64)
            .is_some_and(|a| n < a)
            || s.get("maximum")
                .and_then(Value::as_i64)
                .is_some_and(|a| n > a)
        {
            return Err(fail("integer range"));
        }
    } else if let Some(n) = v.as_u64() {
        if s.get("maximum")
            .and_then(Value::as_u64)
            .is_some_and(|a| n > a)
        {
            return Err(fail("integer range"));
        }
    }
    Ok(())
}

/// Require every interpreted member while permitting additive extensions.
pub fn required_equal(expected: &Value, actual: &Value, path: &str) -> Result<()> {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (k, v) in e {
                required_equal(
                    v,
                    a.get(k)
                        .ok_or_else(|| Error(format!("{path}.{k}: missing")))?,
                    &format!("{path}.{k}"),
                )?;
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            if e.len() != a.len() {
                return Err(Error(format!("{path}: array length")));
            }
            for (i, (x, y)) in e.iter().zip(a).enumerate() {
                required_equal(x, y, &format!("{path}[{i}]"))?;
            }
        }
        _ => {
            if expected != actual {
                return Err(Error(format!("{path}: raw/interpretation mismatch")));
            }
        }
    }
    Ok(())
}
