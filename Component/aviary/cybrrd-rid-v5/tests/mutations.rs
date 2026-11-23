// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_v5::{capture, codec, schema};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf};

fn paths(v: &Value, path: String, leaves: &mut Vec<String>, containers: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            containers.push(path.clone());
            for (k, v) in o {
                paths(
                    v,
                    format!("{path}/{}", k.replace('~', "~0").replace('/', "~1")),
                    leaves,
                    containers,
                );
            }
        }
        Value::Array(a) => {
            containers.push(path.clone());
            for (i, v) in a.iter().enumerate() {
                paths(v, format!("{path}/{i}"), leaves, containers);
            }
        }
        _ => leaves.push(path),
    }
}
fn remove(v: &mut Value, path: &str) {
    if path.is_empty() {
        *v = Value::Null;
        return;
    }
    let (parent, key) = path.rsplit_once('/').unwrap();
    let obj = v.pointer_mut(parent).unwrap();
    if let Some(a) = obj.as_array_mut() {
        a.remove(key.parse().unwrap());
    } else {
        obj.as_object_mut()
            .unwrap()
            .remove(&key.replace("~1", "/").replace("~0", "~"));
    }
}
fn change(v: &mut Value, path: &str) {
    let v = v.pointer_mut(path).unwrap();
    *v = match v {
        Value::Null => json!(0),
        Value::Bool(b) => json!(!*b),
        Value::Number(_) => json!("wrong-type"),
        Value::String(s) => json!(format!("{s}#mutated")),
        Value::Array(_) => json!({}),
        Value::Object(_) => json!([]),
    };
}
fn normalized(path: &str) -> String {
    path.split('/')
        .map(|p| {
            if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) {
                "*"
            } else {
                p
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}
fn sample(s: &Value) -> Value {
    let s = schema::resolved(s);
    if let Some(v) = s.get("const") {
        return v.clone();
    }
    if let Some(a) = s["enum"].as_array() {
        return a[0].clone();
    }
    match s["type"].as_str() {
        Some("object") => {
            let mut v = json!({});
            if let Some(p) = s["properties"].as_object() {
                for (k, s) in p {
                    v[k] = sample(s);
                }
            }
            if v.get("status").is_some() {
                v["status"] = json!("known");
                v["reason"] = Value::Null;
            }
            v
        }
        Some("array") => json!([sample(&s["items"])]),
        Some("integer") => s.get("minimum").cloned().unwrap_or(json!(0)),
        Some("boolean") => json!(false),
        Some("null") => Value::Null,
        Some("string") => {
            if s["x-parquet"] == "BYTE_ARRAY(hex decoded)" {
                json!("00".repeat(s["minLength"].as_u64().unwrap_or(2) as usize / 2))
            } else if s.get("pattern").is_some() {
                json!("0")
            } else {
                json!("synthetic")
            }
        }
        _ => Value::Null,
    }
}
fn matrix(
    label: &str,
    expected: &Value,
    actual: &Value,
    rows: &mut Vec<Value>,
    covered: &mut BTreeSet<String>,
) {
    schema::verify_expected(expected, actual).unwrap();
    let mut leaves = vec![];
    let mut containers = vec![];
    paths(actual, String::new(), &mut leaves, &mut containers);
    for (kind, all) in [("leaf", leaves), ("container", containers)] {
        for path in all {
            covered.insert(normalized(&path));
            for mutation in ["delete", "value"] {
                let mut altered = actual.clone();
                if mutation == "delete" {
                    remove(&mut altered, &path);
                } else {
                    change(&mut altered, &path);
                }
                let error = schema::verify_expected(expected, &altered)
                    .expect_err("mutation escaped exact boundary verifier");
                rows.push(json!({"fixture":label,"path":path,"kind":kind,"mutation":mutation,"boundary":"decode_result_vs_independent_expected","result":"rejected","diagnostic":error}));
            }
        }
    }
}

#[test]
fn all_inventory_homes_and_declared_leaves_have_mutation_receipts() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../contract/golden-observations.json")).unwrap();
    let mut rows = vec![];
    let mut covered = BTreeSet::new();
    let mut fixture_leaves = 0;
    for fixture in fixtures["fixtures"].as_array().unwrap() {
        let expected = &fixture["record"];
        let actual = capture::decode_capture(capture::fixture_input(expected).unwrap()).observation;
        let mut l = vec![];
        paths(&actual, String::new(), &mut l, &mut vec![]);
        fixture_leaves += l.len();
        matrix(
            fixture["name"].as_str().unwrap(),
            expected,
            &actual,
            &mut rows,
            &mut covered,
        );
        // Explicit synthetic receive metadata exercises nullable compound homes
        // that are not exposed by the minimal boundary examples. It is input
        // evidence to the decoder, never invented hardware or signature proof.
        let mut rich = expected.clone();
        rich["issues"] = sample(&schema::observation()["properties"]["issues"]);
        rich["receiver"] = sample(&schema::observation()["properties"]["receiver"]);
        for k in ["raw_frame_bytes", "frame_length"] {
            rich["receiver"][k] = expected["receiver"][k].clone();
        }
        let kind = rich["captures"][0]["transport_kind"]
            .as_str()
            .unwrap()
            .to_owned();
        rich["captures"][0]["transport_fields"][&kind] = sample(
            &schema::observation()["properties"]["captures"]["items"]["properties"]
                ["transport_fields"]["properties"][&kind],
        );
        let actual = capture::decode_capture(capture::fixture_input(&rich).unwrap()).observation;
        matrix(
            &format!("synthetic_metadata_{}", fixture["name"]),
            &rich,
            &actual,
            &mut rows,
            &mut covered,
        );
    }
    assert_eq!(fixture_leaves, 5004);
    let asm: Value = serde_json::from_str(include_str!(
        "../contract/golden-authentication-assembly.json"
    ))
    .unwrap();
    let vectors: Value =
        serde_json::from_str(include_str!("../contract/golden-messages.json")).unwrap();
    let pages = (0..16)
        .map(|i| {
            let raw = vectors["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == format!("auth_page_{i}"))
                .unwrap()["raw_message_hex"]
                .as_str()
                .unwrap();
            cybrrd_rid_v5::auth::PageEvidence {
                observation_id: serde_json::from_value(
                    asm["page_refs"][i]["observation_id"].clone(),
                )
                .unwrap(),
                unit_index: 0,
                message_index: 0,
                bytes: cybrrd_rid_v5::unhex(raw).unwrap(),
                source_mac: Some("020000000001".into()),
                transport: "wifi_beacon".into(),
                counter: Some(7),
                association: Some("synthetic-explicit-generation-and-clock-context".into()),
            }
        })
        .collect();
    let actual =
        cybrrd_rid_v5::auth::assemble(pages, "synthetic-assembly", 1, "design-oracle-0").derived;
    let mut assembly_coverage = BTreeSet::new();
    matrix(
        "derived_assembly",
        &asm,
        &actual,
        &mut rows,
        &mut assembly_coverage,
    );

    // Inventory CSV contains no quoted commas in home columns. Parse just the
    // columns needed for the mapping, preserving the distinct alias homes.
    let mut inventory_rows = vec![];
    for (network, text) in [
        (false, include_str!("../contract/inventory-map.csv")),
        (true, include_str!("../contract/network-inventory-map.csv")),
    ] {
        for line in text.lines().skip(1) {
            let columns: Vec<_> = line.split(',').collect();
            let homes = if network {
                vec![columns[2]]
            } else {
                vec![columns[3], columns[4]]
            };
            for home in homes {
                if home.is_empty() {
                    continue;
                }
                let pointer = format!("/{}", home.replace("[]", "/*").replace('.', "/"));
                assert!(
                    covered.contains(&pointer),
                    "uncovered inventory home {home}"
                );
                let count = rows
                    .iter()
                    .filter(|r| normalized(r["path"].as_str().unwrap()) == pointer)
                    .count();
                assert!(count >= 2);
                inventory_rows.push(json!({"row":columns[0],"home":home,"boundary":"decode","mutations":count,"untested":false}));
            }
        }
    }
    assert_eq!(
        inventory_rows
            .iter()
            .map(|r| r["row"].to_string())
            .collect::<BTreeSet<_>>()
            .len(),
        140
    );
    // Structural alternatives under broadcast-inapplicable network fields have
    // no on-air decoder path in P0. Exercise them through the same typed codec
    // and comparator and label that boundary explicitly in the leaf registry.
    let shape = sample(schema::observation());
    let mut leaves = vec![];
    let mut containers = vec![];
    paths(&shape, String::new(), &mut leaves, &mut containers);
    let mut leaf_registry = vec![];
    for path in leaves {
        let normalized = normalized(&path);
        let boundary = if covered.contains(&normalized) {
            "decode"
        } else {
            "schema_alternative_codec"
        };
        if boundary == "schema_alternative_codec" {
            let wire = codec::canonical_cbor(&shape, schema::observation()).unwrap();
            let actual = codec::decode_cbor(&wire, schema::observation()).unwrap();
            for mutation in ["delete", "value"] {
                let mut changed = actual.clone();
                if mutation == "delete" {
                    remove(&mut changed, &path);
                } else {
                    change(&mut changed, &path);
                }
                assert!(schema::verify_expected(&shape, &changed).is_err());
                rows.push(json!({"fixture":"schema_shape_witness_not_broadcast_conformance","path":path,"kind":"leaf","mutation":mutation,"boundary":boundary,"result":"rejected"}));
            }
        }
        leaf_registry.push(json!({"path":normalized,"boundary":boundary,"untested":false}));
    }
    assert_eq!(leaf_registry.len(), 1032);
    let asm_shape = sample(schema::assembly());
    let mut leaves = vec![];
    paths(&asm_shape, String::new(), &mut leaves, &mut vec![]);
    assert_eq!(leaves.len(), 23);
    for path in leaves {
        let p = normalized(&path);
        assert!(assembly_coverage.contains(&p), "assembly leaf {p}");
        leaf_registry.push(
            json!({"path":format!("assembly{p}"),"boundary":"derived_assembly","untested":false}),
        );
    }
    let report = json!({"inventory_rows":140,"inventory_homes_including_aliases":inventory_rows,"declared_leaf_paths":leaf_registry,"fixture_leaves":fixture_leaves,"mutation_cases":rows.len(),"untested_required_cells":0,"matrix":rows});
    if let Some(dir) = std::env::var_os("V5_RECEIPT_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mutation-matrix.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
    println!("mutation cases: {}; inventory rows: 140; declared leaves: 1055; untested required cells: 0",report["mutation_cases"]);
}

#[test]
fn six_named_historic_regressions_are_rejected() {
    let vectors: Value =
        serde_json::from_str(include_str!("../contract/golden-messages.json")).unwrap();
    let find = |name: &str| {
        vectors["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == name)
            .unwrap()["expected"]
            .clone()
    };
    let mut cases = vec![];
    let expected = find("basic_id_type_2");
    let mut bad = expected.clone();
    bad["basic_id"]["id_type"]["decoded"] = json!(1);
    cases.push(("serial_fallback", expected, bad));
    let expected = find("location_alt_raw_2000");
    let mut bad = expected.clone();
    bad["location"]["geodetic_altitude_hae"]["unit"] = json!("m_MSL");
    cases.push(("hae_msl_confusion", expected, bad));
    let expected = find("auth_page_0");
    let mut bad = expected.clone();
    bad["auth"]["last_page"]["raw"] = json!(175);
    cases.push(("whole_byte_last_page", expected, bad));
    let expected = json!([find("auth_page_1"), find("auth_page_0")]);
    let bad = json!([find("auth_page_0")]);
    cases.push(("continuation_first_loss", expected, bad));
    let expected = json!([find("basic_id_type_1"), find("serial_display_b")]);
    let bad = json!([find("basic_id_type_1")]);
    cases.push(("changed_content_dedup", expected, bad));
    let expected = find("header_version_0");
    let mut bad = expected.clone();
    bad["parse_status"] = json!("complete");
    bad["basic_id"] = find("basic_id_type_0")["basic_id"].clone();
    cases.push(("unchecked_version_nibble", expected, bad));
    let mut receipt = vec![];
    for (name, expected, bad) in cases {
        schema::verify_expected(&expected, &expected).unwrap();
        let failure = schema::verify_expected(&expected, &bad).expect_err(name);
        receipt.push(json!({"regression":name,"expected":expected,"faulty_output":bad,"rejected_at":failure}));
    }
    if let Some(dir) = std::env::var_os("V5_RECEIPT_DIR") {
        std::fs::write(
            PathBuf::from(dir).join("historic-regressions.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
    }
}
