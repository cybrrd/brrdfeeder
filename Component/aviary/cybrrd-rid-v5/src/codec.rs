// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Revision-0 canonical codecs. Invalid parcels remain the caller's exact bytes;
//! `inspect_parcel` packages those bytes on every outcome, including errors.
use crate::{hex, schema, unhex};
use serde::de::{Deserialize, Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, fmt, sync::OnceLock};
const MAX_SAFE: i64 = 9_007_199_254_740_991;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_NODES: usize = 1_000_000;
const MAX_DEPTH: usize = 64;

fn dictionary() -> &'static BTreeMap<String, u64> {
    static DICT: OnceLock<BTreeMap<String, u64>> = OnceLock::new();
    DICT.get_or_init(|| {
        let v: Value = serde_json::from_str(include_str!("../contract/cbor-keys.json"))
            .expect("pinned dictionary");
        serde_json::from_value(v["keys"].clone()).expect("dictionary keys")
    })
}
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unique-key integer-only JSON")
            }
            fn visit_bool<E: Error>(self, v: bool) -> Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_unit<E: Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_i64<E: Error>(self, v: i64) -> Result<Unique, E> {
                if !(-MAX_SAFE..=MAX_SAFE).contains(&v) {
                    Err(E::custom("unsafe integer"))
                } else {
                    Ok(Unique(json!(v)))
                }
            }
            fn visit_u64<E: Error>(self, v: u64) -> Result<Unique, E> {
                if v > MAX_SAFE as u64 {
                    Err(E::custom("unsafe integer"))
                } else {
                    Ok(Unique(json!(v)))
                }
            }
            fn visit_f64<E: Error>(self, _: f64) -> Result<Unique, E> {
                Err(E::custom("floating point outside domain"))
            }
            fn visit_str<E: Error>(self, v: &str) -> Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_string<E: Error>(self, v: String) -> Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut out = vec![];
                while let Some(v) = a.next_element::<Unique>()? {
                    if out.len() >= MAX_NODES {
                        return Err(A::Error::custom("array budget"));
                    }
                    out.push(v.0);
                }
                Ok(Unique(json!(out)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut out = Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if out.contains_key(&k) {
                        return Err(A::Error::custom("duplicate JSON key"));
                    }
                    if out.len() >= MAX_NODES {
                        return Err(A::Error::custom("map budget"));
                    }
                    out.insert(k, a.next_value::<Unique>()?.0);
                }
                Ok(Unique(Value::Object(out)))
            }
        }
        d.deserialize_any(V)
    }
}
fn check_domain(v: &Value, depth: usize, nodes: &mut usize) -> Result<(), String> {
    *nodes += 1;
    if depth > MAX_DEPTH || *nodes > MAX_NODES {
        return Err("logical tree budget".into());
    }
    match v {
        Value::Number(n) => {
            if !n
                .as_i64()
                .is_some_and(|n| (-MAX_SAFE..=MAX_SAFE).contains(&n))
            {
                return Err("non-safe-integer number".into());
            }
        }
        Value::Array(a) => {
            for v in a {
                check_domain(v, depth + 1, nodes)?;
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                check_domain(v, depth + 1, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn decode_json(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_BYTES {
        return Err("parcel byte budget".into());
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let v = Unique::deserialize(&mut d).map_err(|e| e.to_string())?.0;
    d.end().map_err(|e| e.to_string())?;
    check_domain(&v, 0, &mut 0)?;
    Ok(v)
}
pub fn canonical_json(v: &Value) -> Result<Vec<u8>, String> {
    check_domain(v, 0, &mut 0)?;
    // serde_json's default Map is a BTreeMap. UTF-8 ordering of valid scalar
    // strings is Unicode-code-point ordering; arrays remain ordered.
    let bytes = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("parcel byte budget".into());
    }
    Ok(bytes)
}
fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    if n < 24 {
        out.push(major * 32 + n as u8);
    } else if n <= u8::MAX as u64 {
        out.extend([major * 32 + 24, n as u8]);
    } else if n <= u16::MAX as u64 {
        out.push(major * 32 + 25);
        out.extend((n as u16).to_be_bytes());
    } else if n <= u32::MAX as u64 {
        out.push(major * 32 + 26);
        out.extend((n as u32).to_be_bytes());
    } else {
        out.push(major * 32 + 27);
        out.extend(n.to_be_bytes());
    }
}
fn byte_path(s: &Value) -> bool {
    schema::resolved(s)["x-parquet"] == "BYTE_ARRAY(hex decoded)"
}
fn encode(v: &Value, s: &Value, out: &mut Vec<u8>) -> Result<(), String> {
    let s = schema::resolved(s);
    match v {
        Value::Null => out.push(0xf6),
        Value::Bool(v) => out.push(if *v { 0xf5 } else { 0xf4 }),
        Value::Number(n) => {
            let n = n.as_i64().ok_or("integer")?;
            if n >= 0 {
                head(out, 0, n as u64);
            } else {
                head(out, 1, (-1 - n) as u64);
            }
        }
        Value::String(text) => {
            if byte_path(s) {
                let b = unhex(text)?;
                head(out, 6, 60000);
                head(out, 2, b.len() as u64);
                out.extend(b);
            } else {
                head(out, 3, text.len() as u64);
                out.extend(text.as_bytes());
            }
        }
        Value::Array(a) => {
            head(out, 4, a.len() as u64);
            for v in a {
                encode(v, &s["items"], out)?;
            }
        }
        Value::Object(o) => {
            let mut entries = vec![];
            for (k, v) in o {
                let mut encoded = vec![];
                if let Some(&code) = dictionary().get(k) {
                    head(&mut encoded, 0, code);
                } else {
                    head(&mut encoded, 3, k.len() as u64);
                    encoded.extend(k.as_bytes());
                }
                entries.push((encoded, k, v));
            }
            entries.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
            head(out, 5, entries.len() as u64);
            for (key, k, v) in entries {
                out.extend(key);
                encode(v, &s["properties"][k], out)?;
            }
        }
    }
    Ok(())
}
pub fn canonical_cbor(v: &Value, s: &Value) -> Result<Vec<u8>, String> {
    check_domain(v, 0, &mut 0)?;
    let mut out = vec![];
    encode(v, s, &mut out)?;
    if out.len() > MAX_BYTES {
        return Err("parcel byte budget".into());
    }
    Ok(out)
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    nodes: usize,
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(n).ok_or("length overflow")?;
        let b = self.bytes.get(self.at..end).ok_or("truncated CBOR")?;
        self.at = end;
        Ok(b)
    }
    fn head(&mut self) -> Result<(u8, u64), String> {
        let first = self.take(1)?[0];
        let major = first >> 5;
        let ai = first & 31;
        if ai < 24 {
            return Ok((major, u64::from(ai)));
        }
        let width = match ai {
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return Err("indefinite/reserved length".into()),
        };
        let mut n = 0_u64;
        for &b in self.take(width)? {
            n = (n << 8) | u64::from(b);
        }
        let min = match width {
            1 => 24,
            2 => 256,
            4 => 65536,
            _ => 4294967296,
        };
        if n < min {
            return Err("non-shortest argument".into());
        }
        Ok((major, n))
    }
    fn text(&mut self, n: u64) -> Result<String, String> {
        let n = usize::try_from(n).map_err(|_| "length overflow")?;
        Ok(std::str::from_utf8(self.take(n)?)
            .map_err(|_| "invalid UTF-8")?
            .to_owned())
    }
    fn value(&mut self, s: &Value, depth: usize) -> Result<Value, String> {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return Err("CBOR tree budget".into());
        }
        let s = schema::resolved(s);
        let (major, n) = self.head()?;
        match major {
            0 if n <= MAX_SAFE as u64 => Ok(json!(n)),
            1 if n < MAX_SAFE as u64 => Ok(json!(-1 - (n as i64))),
            3 => {
                if byte_path(s) {
                    return Err("byte path requires tag 60000".into());
                }
                Ok(json!(self.text(n)?))
            }
            4 => {
                if n > MAX_NODES as u64 || n > (self.bytes.len() - self.at) as u64 {
                    return Err("array length budget".into());
                }
                let mut a = vec![];
                for _ in 0..n {
                    a.push(self.value(&s["items"], depth + 1)?);
                }
                Ok(json!(a))
            }
            5 => {
                if n > MAX_NODES as u64 || n > (self.bytes.len() - self.at) as u64 / 2 {
                    return Err("map length budget".into());
                }
                let mut o = Map::new();
                let mut previous: Option<Vec<u8>> = None;
                for _ in 0..n {
                    let start = self.at;
                    let (km, kn) = self.head()?;
                    let key = match km {
                        0 => dictionary()
                            .iter()
                            .find_map(|(k, &v)| (v == kn).then(|| k.clone()))
                            .ok_or("unknown dictionary code")?,
                        3 => {
                            let k = self.text(kn)?;
                            if dictionary().contains_key(&k) {
                                return Err("known field encoded as text alias".into());
                            }
                            k
                        }
                        _ => return Err("map key type".into()),
                    };
                    let encoded = self.bytes[start..self.at].to_vec();
                    if let Some(p) = previous {
                        if (p.len(), &p) >= (encoded.len(), &encoded) {
                            return Err("duplicate or unordered key".into());
                        }
                    }
                    previous = Some(encoded);
                    let v = self.value(&s["properties"][&key], depth + 1)?;
                    if o.insert(key, v).is_some() {
                        return Err("duplicate logical key".into());
                    }
                }
                Ok(Value::Object(o))
            }
            6 if n == 60000 && byte_path(s) => {
                let (major, n) = self.head()?;
                if major != 2 {
                    return Err("byte tag payload".into());
                }
                let n = usize::try_from(n).map_err(|_| "length overflow")?;
                Ok(json!(hex(self.take(n)?)))
            }
            7 => match n {
                20 => Ok(json!(false)),
                21 => Ok(json!(true)),
                22 => Ok(Value::Null),
                _ => Err("unsupported simple/float".into()),
            },
            _ => Err("unsupported integer, bytes or tag placement".into()),
        }
    }
}
pub fn decode_cbor(bytes: &[u8], s: &Value) -> Result<Value, String> {
    if bytes.len() > MAX_BYTES {
        return Err("parcel byte budget".into());
    }
    let mut r = Reader {
        bytes,
        at: 0,
        nodes: 0,
    };
    let out = r.value(s, 0)?;
    if r.at != bytes.len() {
        return Err("trailing CBOR bytes".into());
    }
    Ok(out)
}

pub fn inspect_parcel(bytes: &[u8], media_type: &str) -> Value {
    let parsed = match media_type {
        "application/json" => decode_json(bytes),
        "application/cbor" => decode_cbor(bytes, schema::observation()),
        _ => Err("unsupported media type".into()),
    };
    match parsed {
        Err(e) => {
            json!({"exact_hex":hex(bytes),"media_type":media_type,"disposition":"quarantine","reason":e})
        }
        Ok(v) => {
            let label = match v.get("wire_format_version") {
                None => "historic_unversioned",
                Some(n) if n == 4 => "legacy_v4_unchanged",
                Some(n) if n == 5 => "v5",
                _ => "unsupported_wire_version",
            };
            json!({"exact_hex":hex(bytes),"media_type":media_type,"profile":label,"logical":v})
        }
    }
}
