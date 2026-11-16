// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Boundary fault injection against independent fixture expectations. These are
//! development boundaries, not WAL/broker/archive or hardware evidence.
use super::cases::{event, record, Ledger};
use cybrrd_rid_protocol::v5::{self, codec, model, producer::*, schema};
use serde_json::{json, Value};

#[derive(Clone, Debug)]
enum Part {
    Key(String),
    Index(usize),
}
fn paths(v: &Value, p: &mut Vec<Part>, out: &mut Vec<Vec<Part>>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                p.push(Part::Key(k.clone()));
                out.push(p.clone());
                paths(x, p, out);
                p.pop();
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                p.push(Part::Index(i));
                out.push(p.clone());
                paths(x, p, out);
                p.pop();
            }
        }
        _ => {}
    }
}
fn changed(v: &Value) -> Value {
    match v {
        Value::Null => json!(false),
        Value::Bool(b) => json!(!b),
        Value::Number(_) => json!("changed_number"),
        Value::String(s) => json!(format!("{s}!")),
        Value::Array(a) => {
            let mut a = a.clone();
            a.push(Value::Null);
            json!(a)
        }
        Value::Object(m) => {
            let mut m = m.clone();
            m.insert("mutation_probe".into(), json!(true));
            json!(m)
        }
    }
}
fn mutate(v: &mut Value, path: &[Part], delete: bool) {
    let (last, parent) = path.split_last().unwrap();
    let mut cursor = v;
    for part in parent {
        cursor = match part {
            Part::Key(k) => cursor.get_mut(k).unwrap(),
            Part::Index(i) => cursor.get_mut(*i).unwrap(),
        };
    }
    match last {
        Part::Key(k) => {
            let o = cursor.as_object_mut().unwrap();
            if delete {
                o.remove(k);
            } else {
                let x = changed(&o[k]);
                o.insert(k.clone(), x);
            }
        }
        Part::Index(i) => {
            let a = cursor.as_array_mut().unwrap();
            if delete {
                a.remove(*i);
            } else {
                a[*i] = changed(&a[*i]);
            }
        }
    }
}
fn first_bad<'a>(expected: &Value, boundaries: &[(&'a str, Value)]) -> Option<&'a str> {
    boundaries
        .iter()
        .find(|(_, v)| v != expected)
        .map(|(name, _)| *name)
}
fn baseline(expected: &Value, message: bool) -> Vec<(&'static str, Value)> {
    let s = if message {
        schema::message_contract().unwrap()
    } else {
        schema::contract().unwrap()
    };
    let original = codec::canonical_json(expected).unwrap();
    let mut ledger = Ledger::default();
    let e = event(original.clone());
    ledger.append(&e).unwrap();
    let receipt = codec::parse_json(&ledger.events[0].bytes).unwrap();
    let modeled = if message {
        let raw = v5::unhex(expected["raw_hex"].as_str().unwrap()).unwrap();
        let mut decoded =
            v5::decode_message(&raw, expected["index"].as_u64().unwrap() as usize).unwrap();
        // Pure extension fixtures preserve additive members during typed handling.
        if let Some(x) = expected.get("extensions") {
            decoded["extensions"] = x.clone();
        }
        decoded
    } else {
        model::Observation::new(receipt.clone())
            .unwrap()
            .into_record()
    };
    let json_record = codec::parse_json(&codec::canonical_json(&modeled).unwrap()).unwrap();
    let cbor = codec::canonical_cbor(&modeled, s).unwrap();
    let decoded = codec::parse_cbor(&cbor, s).unwrap();
    let mut out = vec![
        ("capture_interface", receipt),
        ("model", modeled),
        ("json_encode", json_record),
        ("cbor_decode", decoded.clone()),
    ];
    if !message {
        out.push((
            "test_ingress",
            record(test_ingress(&mut ledger, event(cbor), Media::CborRevision0).unwrap()),
        ));
    }
    assert_eq!(first_bad(expected, &out), None);
    out
}
#[test]
fn every_field_deletion_and_alteration_at_every_p0_boundary() {
    let messages = super::messages();
    let observations = super::observations();
    let supplemental = super::supplemental();
    let mut counts = [0usize; 5];
    let mut field_count = 0;
    let fixtures = messages["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (&x["expected"], true))
        .chain(
            observations["fixtures"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| (&x["record"], false)),
        )
        .chain(
            supplemental["fixtures"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| (&x["record"], false)),
        );
    for (expected, message) in fixtures {
        let mut expected = expected.clone();
        if expected.get("extensions").is_none() {
            expected["extensions"] = json!({});
        }
        expected["extensions"]["future_member"] = json!({"null":null,"empty":"","zero":0,"false":false,"ordered":["a","a","b"],"hex_looking":"deadbeef"});
        let good = baseline(&expected, message);
        let mut fields = Vec::new();
        paths(&expected, &mut Vec::new(), &mut fields);
        field_count += fields.len();
        for path in fields {
            for delete in [true, false] {
                for boundary in 0..good.len() {
                    let mut bad = good.clone();
                    mutate(&mut bad[boundary].1, &path, delete);
                    assert_ne!(bad[boundary].1, expected, "ineffective mutation {path:?}");
                    // Mutated logical output crosses the real serialization and
                    // parse boundary; invalid canonical values are a rejection.
                    let downstream = if boundary == 3 {
                        codec::canonical_cbor(
                            &bad[boundary].1,
                            if message {
                                schema::message_contract().unwrap()
                            } else {
                                schema::contract().unwrap()
                            },
                        )
                        .and_then(|b| {
                            codec::parse_cbor(
                                &b,
                                if message {
                                    schema::message_contract().unwrap()
                                } else {
                                    schema::contract().unwrap()
                                },
                            )
                        })
                    } else {
                        codec::canonical_json(&bad[boundary].1).and_then(|b| codec::parse_json(&b))
                    };
                    if let Ok(v) = downstream {
                        assert_ne!(v, expected, "mutation normalized away {path:?}");
                    }
                    assert_eq!(first_bad(&expected, &bad), Some(good[boundary].0));
                    counts[boundary] += 1;
                    // Restoration is the original exercised pipeline, not an
                    // assertion inversion or replacement expected value.
                    assert_eq!(first_bad(&expected, &good), None);
                }
            }
        }
    }
    assert!(counts.iter().all(|n| *n > 0));
    println!("P0_FIELD_REGISTRY_NODES={field_count}; P0_MUTATION_FIELD_BOUNDARY_COUNTS={counts:?}; TOTAL={}",counts.iter().sum::<usize>());
}

#[test]
fn explicit_order_counter_and_six_historic_defect_controls() {
    let fixtures = super::observations();
    let expected = fixtures["fixtures"][1]["record"].clone();
    let good = baseline(&expected, false);
    let mut count = 0;
    let defects: [fn(&mut Value); 6] = [
        |v| {
            v["rid_units"][0]["messages"][0]["basic_id"]["id_type"]["raw"] = json!(1);
        },
        |v| {
            v["rid_units"][0]["messages"][2]["location"]["geodetic_altitude_hae"]["unit"] =
                json!("m_MSL");
        },
        |v| {
            v["rid_units"][0]["messages"][4]["auth"]["last_page"]["raw"] = json!(175);
        },
        |v| {
            v["rid_units"][0]["messages"]
                .as_array_mut()
                .unwrap()
                .remove(3);
        },
        |v| {
            v["rid_units"][0]["messages"]
                .as_array_mut()
                .unwrap()
                .remove(1);
        },
        |v| {
            v["rid_units"][0]["messages"][0]["header"]["protocol_version"]["raw"] = json!(0);
        },
    ];
    for defect in defects {
        for boundary in 0..good.len() {
            let mut bad = good.clone();
            defect(&mut bad[boundary].1);
            assert_eq!(first_bad(&expected, &bad), Some(good[boundary].0));
            assert_eq!(first_bad(&expected, &good), None);
            count += 1;
        }
    }
    for boundary in 0..good.len() {
        for mode in 0..3 {
            let mut bad = good.clone();
            let v = &mut bad[boundary].1;
            match mode {
                0 => v["rid_units"][0]["messages"]
                    .as_array_mut()
                    .unwrap()
                    .swap(0, 1),
                1 => v["rid_units"][0]["message_counter"]["raw"] = json!(8),
                _ => v["rid_units"][0]["basic_ids"]
                    .as_array_mut()
                    .unwrap()
                    .reverse(),
            };
            assert_eq!(first_bad(&expected, &bad), Some(good[boundary].0));
            count += 1;
        }
    }
    assert_eq!(count, 45);
    println!("P0_HISTORIC_ORDER_COUNTER_BOUNDARY_CONTROLS={count}/45");
}
