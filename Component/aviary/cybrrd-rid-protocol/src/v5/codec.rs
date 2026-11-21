// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Canonical JSON and revision-0 CBOR. Unsupported parcels remain the caller's
//! ledger evidence; this module never produces a partial successful decode.
use super::{hex, schema, unhex, Error, Result, MAX_DEPTH, MAX_PARCEL_BYTES};
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    ser::{SerializeMap, SerializeSeq},
    Deserialize, Deserializer, Serialize,
};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, fmt, sync::OnceLock};
const SAFE: i64 = 9_007_199_254_740_991;
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("canonical logical value")
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_bool<E: de::Error>(self, x: bool) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Bool(x)))
            }
            fn visit_i64<E: de::Error>(self, x: i64) -> std::result::Result<Unique, E> {
                if !(-SAFE..=SAFE).contains(&x) {
                    return Err(E::custom("unsafe integer"));
                }
                Ok(Unique(x.into()))
            }
            fn visit_u64<E: de::Error>(self, x: u64) -> std::result::Result<Unique, E> {
                if x > SAFE as u64 {
                    return Err(E::custom("unsafe integer"));
                }
                Ok(Unique(x.into()))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Unique, E> {
                Err(E::custom("floating point outside canonical domain"))
            }
            fn visit_str<E: de::Error>(self, x: &str) -> std::result::Result<Unique, E> {
                Ok(Unique(x.into()))
            }
            fn visit_string<E: de::Error>(self, x: String) -> std::result::Result<Unique, E> {
                Ok(Unique(x.into()))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut out = Vec::new();
                while let Some(Unique(x)) = a.next_element()? {
                    out.push(x);
                }
                Ok(Unique(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut out = Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if out.contains_key(&k) {
                        return Err(de::Error::custom("duplicate object key"));
                    }
                    let Unique(v) = a.next_value()?;
                    out.insert(k, v);
                }
                Ok(Unique(Value::Object(out)))
            }
        }
        d.deserialize_any(V)
    }
}
pub fn parse_json(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_PARCEL_BYTES {
        return Err(Error("parcel size limit".into()));
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let Unique(v) = Unique::deserialize(&mut d).map_err(|e| Error(e.to_string()))?;
    d.end().map_err(|e| Error(e.to_string()))?;
    domain(&v, 0)?;
    Ok(v)
}
pub fn domain(v: &Value, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error("logical depth limit".into()));
    }
    match v {
        Value::Number(n) => {
            if !n.as_i64().is_some_and(|i| (-SAFE..=SAFE).contains(&i)) {
                return Err(Error("noncanonical numeric domain".into()));
            }
        }
        Value::Array(a) => {
            for x in a {
                domain(x, depth + 1)?;
            }
        }
        Value::Object(m) => {
            for x in m.values() {
                domain(x, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
// Explicit ordering is independent of serde_json's map backend/features.
struct SortedJson<'a>(&'a Value);
impl Serialize for SortedJson<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(m) => {
                let mut entries: Vec<_> = m.iter().collect();
                entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
                let mut out = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    out.serialize_entry(key, &SortedJson(value))?;
                }
                out.end()
            }
            Value::Array(a) => {
                let mut out = serializer.serialize_seq(Some(a.len()))?;
                for value in a {
                    out.serialize_element(&SortedJson(value))?;
                }
                out.end()
            }
            value => value.serialize(serializer),
        }
    }
}

pub fn canonical_json(v: &Value) -> Result<Vec<u8>> {
    domain(v, 0)?;
    let bytes = serde_json::to_vec(&SortedJson(v)).map_err(|e| Error(e.to_string()))?;
    if bytes.len() > MAX_PARCEL_BYTES {
        return Err(Error("parcel size limit".into()));
    }
    Ok(bytes)
}
struct Dictionary {
    forward: BTreeMap<String, u64>,
    reverse: BTreeMap<u64, String>,
}
fn dictionary() -> Result<&'static Dictionary> {
    static D: OnceLock<std::result::Result<Dictionary, String>> = OnceLock::new();
    match D.get_or_init(|| {
        let v: Value = serde_json::from_str(include_str!("contract/cbor-keys.json"))
            .map_err(|e| e.to_string())?;
        let keys = v
            .get("keys")
            .and_then(Value::as_object)
            .ok_or("dictionary shape")?;
        let mut forward = BTreeMap::new();
        let mut reverse = BTreeMap::new();
        for (k, v) in keys {
            let n = v.as_u64().ok_or("dictionary code")?;
            if n == 0 || reverse.insert(n, k.clone()).is_some() {
                return Err("dictionary collision".into());
            }
            forward.insert(k.clone(), n);
        }
        Ok(Dictionary { forward, reverse })
    }) {
        Ok(d) => Ok(d),
        Err(e) => Err(Error(e.clone())),
    }
}
fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let prefix = major << 5;
    if n < 24 {
        out.push(prefix | n as u8);
    } else if n <= 255 {
        out.extend([prefix | 24, n as u8]);
    } else if n <= 65535 {
        out.push(prefix | 25);
        out.extend((n as u16).to_be_bytes());
    } else if n <= u64::from(u32::MAX) {
        out.push(prefix | 26);
        out.extend((n as u32).to_be_bytes());
    } else {
        out.push(prefix | 27);
        out.extend(n.to_be_bytes());
    }
}
fn text(out: &mut Vec<u8>, s: &str) {
    head(out, 3, s.len() as u64);
    out.extend(s.as_bytes());
}
pub fn canonical_cbor(v: &Value, s: &Value) -> Result<Vec<u8>> {
    domain(v, 0)?;
    let mut out = Vec::new();
    encode(v, Some(s), &mut out, dictionary()?)?;
    if out.len() > MAX_PARCEL_BYTES {
        return Err(Error("parcel size limit".into()));
    }
    Ok(out)
}
fn encode(v: &Value, s: Option<&Value>, out: &mut Vec<u8>, d: &Dictionary) -> Result<()> {
    match v {
        Value::Null => out.push(0xf6),
        Value::Bool(b) => out.push(if *b { 0xf5 } else { 0xf4 }),
        Value::Number(n) => {
            let i = n.as_i64().ok_or_else(|| Error("integer required".into()))?;
            if i >= 0 {
                head(out, 0, i as u64);
            } else {
                head(out, 1, (-1 - i) as u64);
            }
        }
        Value::String(t) => {
            if s.is_some_and(schema::is_hex_schema) {
                let bytes = unhex(t)?;
                head(out, 6, 60000);
                head(out, 2, bytes.len() as u64);
                out.extend(bytes);
            } else {
                text(out, t);
            }
        }
        Value::Array(a) => {
            head(out, 4, a.len() as u64);
            for x in a {
                encode(x, s.and_then(schema::item), out, d)?;
            }
        }
        Value::Object(m) => {
            let mut entries = Vec::with_capacity(m.len());
            for (k, v) in m {
                let mut key = Vec::new();
                if let Some(n) = d.forward.get(k) {
                    head(&mut key, 0, *n);
                } else {
                    text(&mut key, k);
                }
                entries.push((key, k, v));
            }
            entries.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then(a.0.cmp(&b.0)));
            head(out, 5, entries.len() as u64);
            for (key, k, v) in entries {
                out.extend(key);
                encode(v, s.and_then(|s| schema::child(s, k)), out, d)?;
            }
        }
    }
    Ok(())
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    d: &'static Dictionary,
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .offset
            .checked_add(n)
            .ok_or_else(|| Error("length overflow".into()))?;
        let b = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| Error("truncated CBOR".into()))?;
        self.offset = end;
        Ok(b)
    }
    fn header(&mut self) -> Result<(u8, u64)> {
        let b = self.take(1)?[0];
        let ai = b & 31;
        let n = match ai {
            0..=23 => u64::from(ai),
            24 => u64::from(self.take(1)?[0]),
            25 => {
                let b = self.take(2)?;
                u64::from(u16::from_be_bytes([b[0], b[1]]))
            }
            26 => {
                let b = self.take(4)?;
                u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            }
            27 => {
                let b = self.take(8)?;
                u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            }
            _ => return Err(Error("indefinite/reserved CBOR".into())),
        };
        Ok((b >> 5, n))
    }
    fn len(&self, n: u64) -> Result<usize> {
        let n = usize::try_from(n).map_err(|_| Error("length overflow".into()))?;
        if n > self.bytes.len() - self.offset {
            return Err(Error("length exceeds remaining parcel".into()));
        }
        Ok(n)
    }
    fn string(&mut self, n: u64) -> Result<String> {
        let n = self.len(n)?;
        std::str::from_utf8(self.take(n)?)
            .map(str::to_owned)
            .map_err(|e| Error(e.to_string()))
    }
    fn value(&mut self, s: Option<&Value>, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(Error("CBOR depth limit".into()));
        }
        let (major, n) = self.header()?;
        match major {
            0 => {
                if n > SAFE as u64 {
                    return Err(Error("unsafe integer".into()));
                }
                Ok(n.into())
            }
            1 => {
                if n >= SAFE as u64 {
                    return Err(Error("unsafe integer".into()));
                }
                Ok((-1 - (n as i64)).into())
            }
            3 => {
                if s.is_some_and(schema::is_hex_schema) {
                    return Err(Error("byte path requires tag".into()));
                }
                Ok(self.string(n)?.into())
            }
            4 => {
                let count = self.len(n)?;
                let mut a = Vec::new();
                for _ in 0..count {
                    a.push(self.value(s.and_then(schema::item), depth + 1)?);
                }
                Ok(Value::Array(a))
            }
            5 => {
                let count = self.len(n)?;
                let mut m = Map::new();
                for _ in 0..count {
                    let (t, k) = self.header()?;
                    let name = match t {
                        0 => self
                            .d
                            .reverse
                            .get(&k)
                            .cloned()
                            .ok_or_else(|| Error("unknown dictionary code".into()))?,
                        3 => {
                            let key = self.string(k)?;
                            if self.d.forward.contains_key(&key) {
                                return Err(Error("known dictionary text alias".into()));
                            }
                            key
                        }
                        _ => return Err(Error("map key type".into())),
                    };
                    if m.contains_key(&name) {
                        return Err(Error("duplicate map key".into()));
                    }
                    let v = self.value(s.and_then(|s| schema::child(s, &name)), depth + 1)?;
                    m.insert(name, v);
                }
                Ok(Value::Object(m))
            }
            6 => {
                if n != 60000 || !s.is_some_and(schema::is_hex_schema) {
                    return Err(Error("unknown or misplaced tag".into()));
                }
                let (t, n) = self.header()?;
                if t != 2 {
                    return Err(Error("tag requires bytes".into()));
                }
                let n = self.len(n)?;
                Ok(hex(self.take(n)?).into())
            }
            7 => match n {
                20 => Ok(false.into()),
                21 => Ok(true.into()),
                22 => Ok(Value::Null),
                _ => Err(Error("unsupported simple/float".into())),
            },
            _ => Err(Error("untagged bytes or unsupported CBOR".into())),
        }
    }
}
pub fn parse_cbor(bytes: &[u8], s: &Value) -> Result<Value> {
    if bytes.len() > MAX_PARCEL_BYTES {
        return Err(Error("parcel size limit".into()));
    }
    let mut r = Reader {
        bytes,
        offset: 0,
        d: dictionary()?,
    };
    let v = r.value(Some(s), 0)?;
    if r.offset != bytes.len() {
        return Err(Error("trailing CBOR".into()));
    }
    if canonical_cbor(&v, s)? != bytes {
        return Err(Error("noncanonical CBOR".into()));
    }
    Ok(v)
}
