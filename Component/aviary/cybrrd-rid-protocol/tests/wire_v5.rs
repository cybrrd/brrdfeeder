// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
#![cfg(feature = "wire-v5")]
use cybrrd_rid_protocol::v5::{self, codec, model, schema};
use serde_json::Value;
#[path = "wire_v5/adapters.rs"]
mod adapters;
#[path = "wire_v5/cases.rs"]
mod cases;
#[path = "wire_v5/controls.rs"]
mod controls;
#[path = "wire_v5/mutations.rs"]
mod mutations;
#[path = "wire_v5/numeric.rs"]
mod numeric;

fn messages() -> Value {
    serde_json::from_str(include_str!("wire_v5/fixtures/golden-messages.json")).unwrap()
}
fn observations() -> Value {
    serde_json::from_str(include_str!("wire_v5/fixtures/golden-observations.json")).unwrap()
}
fn supplemental() -> Value {
    serde_json::from_str(include_str!(
        "wire_v5/fixtures/supplemental-observations.json"
    ))
    .unwrap()
}
#[test]
fn independent_message_goldens() {
    let fixtures = messages();
    let vectors = fixtures["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 175);
    for x in vectors {
        let raw = v5::unhex(x["raw_message_hex"].as_str().unwrap()).unwrap();
        let actual = v5::decode_message(&raw, 0).unwrap();
        assert_eq!(actual, x["expected"], "{}", x["name"]);
        model::validate_message(&actual).unwrap();
        let s = schema::message_contract().unwrap();
        let bytes = codec::canonical_cbor(&actual, s).unwrap();
        assert_eq!(codec::parse_cbor(&bytes, s).unwrap(), actual);
        assert_eq!(
            codec::parse_json(&codec::canonical_json(&actual).unwrap()).unwrap(),
            actual
        );
    }
    println!("P0_SEED_MESSAGES=175/175");
}
#[test]
fn independent_observation_goldens() {
    let fixtures = observations();
    let fixtures = fixtures["fixtures"].as_array().unwrap();
    assert_eq!(fixtures.len(), 6);
    for x in fixtures {
        let r = &x["record"];
        model::validate(r).unwrap_or_else(|e| panic!("{}: {e}", x["name"]));
        let s = schema::contract().unwrap();
        let c = codec::canonical_cbor(r, s).unwrap();
        assert_eq!(codec::parse_cbor(&c, s).unwrap(), *r);
        assert_eq!(
            codec::parse_json(&codec::canonical_json(r).unwrap()).unwrap(),
            *r
        );
    }
    println!("P0_SEED_OBSERVATIONS=6/6");
}

#[test]
fn canonical_json_has_explicit_recursive_key_order() {
    // Deliberately scrambled keys at root, nested object and inside an array;
    // byte equality guards canonical order even with preserve_order enabled.
    let input = br#"{"z":0,"b":{"z":2,"a":1},"a":[{"b":2,"a":1},3,2,1],"empty":{},"null":null,"false":false}"#;
    let value = codec::parse_json(input).unwrap();
    let expected = br#"{"a":[{"a":1,"b":2},3,2,1],"b":{"a":1,"z":2},"empty":{},"false":false,"null":null,"z":0}"#;
    assert_eq!(
        codec::canonical_json(&value).unwrap(),
        expected,
        "canonical JSON byte order"
    );
    let unicode =
        codec::parse_json("{\"é\":{\"z\":0,\"a\":1},\"z\":2,\"a\":3}".as_bytes()).unwrap();
    assert_eq!(
        codec::canonical_json(&unicode).unwrap(),
        "{\"a\":3,\"z\":2,\"é\":{\"a\":1,\"z\":0}}".as_bytes(),
        "canonical JSON Unicode key order"
    );
    println!("P0_CANONICAL_JSON_BYTE_GOLDENS=2/2");
}
