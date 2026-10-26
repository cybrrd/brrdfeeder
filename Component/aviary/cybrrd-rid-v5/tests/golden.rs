// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_v5::{decode_message, unhex};
use serde_json::Value;

#[test]
fn all_175_pinned_message_vectors() {
    let corpus: Value =
        serde_json::from_str(include_str!("../contract/golden-messages.json")).unwrap();
    let vectors = corpus["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 175);
    for v in vectors {
        let bytes = unhex(v["raw_message_hex"].as_str().unwrap()).unwrap();
        assert_eq!(decode_message(&bytes, 0), v["expected"], "{}", v["name"]);
    }
}

#[test]
fn all_six_observations_from_exact_capture_spans() {
    let corpus: Value =
        serde_json::from_str(include_str!("../contract/golden-observations.json")).unwrap();
    for fixture in corpus["fixtures"].as_array().unwrap() {
        let expected = &fixture["record"];
        let input = cybrrd_rid_v5::capture::fixture_input(expected).unwrap();
        let exact = input.bytes.clone();
        let output = cybrrd_rid_v5::decode_capture(input);
        assert_eq!(output.raw_capture.bytes, exact);
        assert_eq!(output.observation, *expected, "{}", fixture["name"]);
    }
}

fn pages() -> Vec<cybrrd_rid_v5::auth::PageEvidence> {
    let corpus: Value =
        serde_json::from_str(include_str!("../contract/golden-messages.json")).unwrap();
    (0..16)
        .map(|i| {
            let v = corpus["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == format!("auth_page_{i}"))
                .unwrap();
            cybrrd_rid_v5::auth::PageEvidence {
                observation_id: cybrrd_rid_v5::capture::ObservationId {
                    node_id: "synthetic-node".into(),
                    boot_id: "synthetic-boot".into(),
                    capture_sequence: i.to_string(),
                },
                unit_index: 0,
                message_index: 0,
                bytes: unhex(v["raw_message_hex"].as_str().unwrap()).unwrap(),
                source_mac: Some("020000000001".into()),
                transport: "wifi_beacon".into(),
                counter: Some(7),
                association: Some("synthetic-explicit-generation-and-clock-context".into()),
            }
        })
        .collect()
}
#[test]
fn exact_sixteen_page_assembly() {
    let expected: Value = serde_json::from_str(include_str!(
        "../contract/golden-authentication-assembly.json"
    ))
    .unwrap();
    let got = cybrrd_rid_v5::auth::assemble(pages(), "synthetic-assembly", 1, "design-oracle-0");
    assert_eq!(got.derived, expected);
    assert_eq!(got.pages.len(), 16);
}
#[test]
fn continuation_first_duplicates_conflicts_and_generations() {
    use cybrrd_rid_v5::auth::assemble;
    let p = pages();
    let baseline = assemble(p.clone(), "a", 1, "v5-p0-0").derived;
    let mut reversed = p.clone();
    reversed.reverse();
    let got = assemble(reversed, "a", 2, "v5-p0-0").derived;
    assert_eq!(got["payload_hex"], baseline["payload_hex"]);
    assert_eq!(got["page_refs"][0]["page_number"], 15);
    let mut duplicate = p.clone();
    duplicate.push(p[1].clone());
    let got = assemble(duplicate, "a", 2, "v5-p0-0");
    assert_eq!(got.derived["state"], "complete");
    assert_eq!(got.derived["page_refs"].as_array().unwrap().len(), 17);
    let mut conflict = p.clone();
    let mut changed = p[1].clone();
    changed.bytes[2] ^= 1;
    conflict.push(changed);
    let got = assemble(conflict, "a", 2, "v5-p0-0");
    assert_eq!(got.derived["state"], "conflicting");
    assert!(got.derived["payload_hex"].is_null());
    for change in 0..6 {
        let mut mixed = p.clone();
        match change {
            0 => mixed[1].counter = Some(0),
            1 => mixed[1].source_mac = Some("020000000002".into()),
            2 => mixed[1].observation_id.boot_id = "next-boot".into(),
            3 => mixed[1].association = None,
            4 => mixed[1].transport = "bt4_legacy".into(),
            _ => mixed[1].bytes[1] ^= 0x10,
        }
        assert_eq!(
            assemble(mixed, "a", 2, "v5-p0-0").derived["state"],
            "ambiguous"
        );
    }
    let mut missing = p;
    missing.remove(3);
    assert_eq!(
        assemble(missing, "a", 2, "v5-p0-0").derived["state"],
        "incomplete"
    );
}

#[test]
fn canonical_routes_for_182_design_records() {
    use cybrrd_rid_v5::{codec, schema};
    fn route(v: &Value, s: &Value) {
        let j = codec::canonical_json(v).unwrap();
        assert_eq!(codec::decode_json(&j).unwrap(), *v);
        let b = codec::canonical_cbor(v, s).unwrap();
        let d = codec::decode_cbor(&b, s).unwrap();
        assert_eq!(d, *v);
        assert_eq!(codec::canonical_cbor(&d, s).unwrap(), b);
    }
    let m: Value = serde_json::from_str(include_str!("../contract/golden-messages.json")).unwrap();
    for v in m["vectors"].as_array().unwrap() {
        route(&v["expected"], schema::message());
    }
    let o: Value =
        serde_json::from_str(include_str!("../contract/golden-observations.json")).unwrap();
    for v in o["fixtures"].as_array().unwrap() {
        route(&v["record"], schema::observation());
    }
    let a: Value = serde_json::from_str(include_str!(
        "../contract/golden-authentication-assembly.json"
    ))
    .unwrap();
    route(&a, schema::assembly());
}
