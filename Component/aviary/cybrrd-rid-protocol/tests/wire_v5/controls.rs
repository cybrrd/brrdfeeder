// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_protocol::v5::{codec, model, schema};
use serde_json::Value;
use std::collections::BTreeSet;

fn apply(v: &mut Value, c: &Value) {
    let path = c["path"].as_str().unwrap();
    let op = c["op"].as_str().unwrap();
    if op == "replace" {
        *v.pointer_mut(path).unwrap() = c["value"].clone();
        return;
    }
    if op == "duplicate_first" {
        let a = v.pointer_mut(path).unwrap().as_array_mut().unwrap();
        a.push(a[0].clone());
        return;
    }
    assert_eq!(op, "delete");
    let (parent, key) = path.rsplit_once('/').unwrap();
    let target = v.pointer_mut(parent).unwrap();
    if let Some(o) = target.as_object_mut() {
        assert!(o.remove(key).is_some());
    } else {
        target
            .as_array_mut()
            .unwrap()
            .remove(key.parse::<usize>().unwrap());
    }
}
#[test]
fn every_supplied_negative_has_executed_control_or_named_phase_deferral() {
    let original: Value =
        serde_json::from_str(include_str!("fixtures/semantic-negative-cases.json")).unwrap();
    let plan: Value =
        serde_json::from_str(include_str!("fixtures/negative-control-plan.json")).unwrap();
    let original = original["cases"].as_array().unwrap();
    let plan = plan.as_array().unwrap();
    assert_eq!(original.len(), 195);
    assert_eq!(plan.len(), 195);
    let expected: BTreeSet<_> = original
        .iter()
        .map(|x| x["case"].as_str().unwrap())
        .collect();
    let actual: BTreeSet<_> = plan.iter().map(|x| x["case"].as_str().unwrap()).collect();
    assert_eq!(actual, expected);
    let messages = super::messages();
    let observations = super::observations();
    let mut counts = [0usize; 3];
    for c in plan {
        match c["phase"].as_str().unwrap() {
            "P1" => {
                assert!(!c["reason"].as_str().unwrap().is_empty());
                counts[1] += 1;
                continue;
            }
            "P2" => {
                assert!(!c["reason"].as_str().unwrap().is_empty());
                counts[2] += 1;
                continue;
            }
            "P0" => {}
            x => panic!("unclassified {x}"),
        }
        let message = c["kind"] == "message";
        let pool = if message {
            messages["vectors"].as_array().unwrap()
        } else {
            observations["fixtures"].as_array().unwrap()
        };
        let seed = pool.iter().find(|x| x["name"] == c["seed"]).unwrap();
        let good = &seed[if message { "expected" } else { "record" }];
        let validate = |v: &Value| {
            if message {
                model::validate_message(v)
            } else {
                model::validate(v)
            }
        };
        validate(good).unwrap();
        let mut bad = good.clone();
        apply(&mut bad, c);
        assert_ne!(&bad, good);
        assert!(validate(&bad).is_err(), "negative escaped: {}", c["case"]);
        validate(good).unwrap();
        counts[0] += 1;
    }
    assert_eq!(counts, [101, 1, 93]);
    println!("P0_SEMANTIC_NEGATIVES=101/101; P1_DEFERRED=1; P2_DEFERRED=93; CLASSIFIED=195/195");
}
fn resolve<'a>(roots: Vec<&'a Value>, path: &str) -> Vec<&'a Value> {
    let mut values = roots;
    for segment in path.split('.') {
        if let Some(name) = segment.strip_suffix("[]") {
            values = values
                .into_iter()
                .filter_map(|v| v.get(name).and_then(Value::as_array))
                .flat_map(|a| a.iter())
                .collect();
        } else {
            values = values.into_iter().filter_map(|v| v.get(segment)).collect();
        }
    }
    values
}
#[test]
fn all_inventory_homes_and_optional_model_cells_are_present() {
    let homes: Value = serde_json::from_str(include_str!("fixtures/inventory-homes.json")).unwrap();
    let homes = homes.as_array().unwrap();
    assert_eq!(homes.len(), 140);
    let fixtures = super::observations();
    let records: Vec<_> = fixtures["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| &x["record"])
        .collect();
    let mut aliases = 0;
    for home in homes {
        let path = home["path"].as_str().unwrap();
        assert!(
            !resolve(records.clone(), path).is_empty(),
            "missing home {path}"
        );
        let alias = home["also_at"].as_str().unwrap();
        if !alias.is_empty() {
            assert!(
                !resolve(records.clone(), alias).is_empty(),
                "missing alias {alias}"
            );
            aliases += 1;
        }
    }
    let supplemental = super::supplemental();
    let mut typed_cells = 0;
    for fixture in supplemental["fixtures"].as_array().unwrap() {
        let r = &fixture["record"];
        model::validate(r).unwrap();
        let c = codec::canonical_cbor(r, schema::contract().unwrap()).unwrap();
        assert_eq!(
            codec::parse_cbor(&c, schema::contract().unwrap()).unwrap(),
            *r
        );
        for cell in r["receiver"].as_object().unwrap().values() {
            if cell["raw"].is_object() || cell["raw"].is_array() {
                typed_cells += 1;
            }
        }
    }
    assert!(typed_cells > 0);
    println!("P0_INVENTORY_HOMES=140/140; ALIAS_HOMES={aliases}/{aliases}; OPTIONAL_STRUCTURED_RECEIVER_CELLS={typed_cells}");
}
