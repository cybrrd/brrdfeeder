// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_v5::{auth, capture, codec, decode_message, decoder, hex, numeric, schema, unhex};
use serde_json::{json, Value};
fn message(t: u8) -> [u8; 25] {
    let mut b = [0; 25];
    b[0] = (t << 4) | 2;
    b
}
fn observations() -> Value {
    serde_json::from_str(include_str!("../contract/golden-observations.json")).unwrap()
}
fn input() -> capture::CaptureRecord {
    capture::fixture_input(&observations()["fixtures"][0]["record"]).unwrap()
}

#[test]
fn message_headers_packs_multiplicity_788() {
    for t in 0..16 {
        for v in 0..16 {
            let mut b = message(t);
            b[0] = (t << 4) | v;
            let m = decode_message(&b, 7);
            assert_eq!(m["index"], 7);
            assert_eq!(m["raw_hex"], hex(&b));
            assert_eq!(m["header"]["protocol_version"]["raw"], v);
            if v != 2 {
                assert_eq!(m["parse_status"], "unsupported_version");
                for k in [
                    "basic_id",
                    "location",
                    "auth",
                    "self_id",
                    "system",
                    "operator_id",
                ] {
                    assert!(m.get(k).is_none());
                }
            } else if t > 5 {
                assert_eq!(m["parse_status"], "unsupported_type");
            }
        }
    }
    for v in 0..16 {
        let pack = [0xf0 | v, 25, 0];
        let u = decoder::decode_unit(&pack, 0, 0, None, true);
        assert_eq!(u["header"]["protocol_version"]["raw"], v);
        assert_eq!(u["raw_hex"], hex(&pack));
    }
    for count in 0..=255_u8 {
        let mut bytes = vec![0xf2, 25, count];
        for _ in 0..count {
            bytes.extend(message(0));
        }
        let u = decoder::decode_unit(&bytes, 0, 0, Some(7), true);
        assert_eq!(u["message_count"]["raw"], count);
        assert_eq!(u["messages"].as_array().unwrap().len(), count as usize);
        assert_eq!(u["basic_ids"].as_array().unwrap().len(), count as usize);
        for (i, m) in u["messages"].as_array().unwrap().iter().enumerate() {
            assert_eq!(m["index"], i);
        }
    }
    for id in 0..16 {
        for ua in 0..16 {
            let mut b = message(0);
            b[1] = (id << 4) | ua;
            b[2] = 65;
            let m = decode_message(&b, 0);
            assert_eq!(m["basic_id"]["id_type"]["raw"], id);
            assert_eq!(m["basic_id"]["ua_type"]["raw"], ua);
            assert!(m.get("hardware_serial").is_none());
        }
    }
    for mode in 0..4 {
        let mut a = message(0);
        a[1] = 0x12;
        a[2] = 65;
        let mut b = a;
        match mode {
            1 => b[2] = 66,
            2 => b[1] = 0x22,
            _ => {}
        }
        let n = if mode == 3 { 9 } else { 2 };
        let mut bytes = vec![0xf2, 25, n];
        for i in 0..n {
            bytes.extend(if i % 2 == 0 { a } else { b });
        }
        let u = decoder::decode_unit(&bytes, 0, 0, None, true);
        let ids = u["basic_ids"].as_array().unwrap();
        assert_eq!(ids.len(), n as usize);
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(id["message_index"], i);
            assert_eq!(
                id["uas_id"]["raw"],
                hex(&if i % 2 == 0 { a } else { b }[2..22])
            );
        }
    }
}

#[test]
fn five_no_position_observations() {
    for typ in [2, 4, 3, 5, 6] {
        let mut r = input();
        r.bytes = message(typ).to_vec();
        r.units = vec![capture::UnitSpan {
            offset: 0,
            length: 25,
            counter_offset: None,
            message_pack: false,
        }];
        r.receiver.clear();
        r.transport_fields.clear();
        let out = capture::decode_capture(r);
        let u = &out.observation["rid_units"][0];
        assert_eq!(u["basic_ids"], json!([]));
        assert!(u["messages"][0].get("location").is_none());
        assert!(!out.observation.is_null());
    }
}

#[test]
fn numeric_default_representative_subset_and_all_small_domains() {
    for raw in 0..=255_u16 {
        let signed = raw as u8 as i8;
        assert_eq!(numeric::vertical(signed).scaled, i64::from(signed) * 5);
        for flag in [false, true] {
            let speed = numeric::speed(raw as u8, flag);
            assert_eq!(
                speed.scaled,
                if flag {
                    (i64::from(raw) + 85) * 75
                } else {
                    i64::from(raw) * 25
                }
            );
            assert_eq!(
                numeric::direction(raw as u8, flag).scaled,
                i64::from(raw) + 180 * i64::from(flag)
            );
        }
    }
    for code in 0..16 {
        for (key, max) in [
            ("vertical_accuracy", 6),
            ("horizontal_accuracy", 12),
            ("baro_altitude_accuracy", 6),
            ("speed_accuracy", 4),
        ] {
            let mut b = message(1);
            b[19] = code * 17;
            b[20] = code * 17;
            b[23] = code;
            let m = decode_message(&b, 0);
            assert_eq!(m["location"][key]["raw"], code);
            assert_eq!(
                m["location"][key]["status"],
                if code == 0 {
                    "unknown"
                } else if code > max {
                    "reserved"
                } else {
                    "known"
                }
            );
            assert_eq!(
                m["location"]["timestamp_accuracy"]["decoded"],
                numeric::decimal(i64::from(code), 1)
            );
        }
    }
    // Seeded, deterministic width/endianness subset; full u16 bindings and u32
    // arithmetic run in the dedicated gate, with separately recorded counts.
    let codes = [
        0, 1, 255, 256, 1999, 2000, 2001, 32767, 32768, 35999, 36000, 36001, 65534, 65535,
    ];
    for raw in codes {
        for (typ, fields) in [
            (
                1,
                [
                    ("pressure_altitude", 13),
                    ("geodetic_altitude_hae", 15),
                    ("height", 17),
                ],
            ),
            (
                4,
                [
                    ("area_ceiling_hae", 13),
                    ("area_floor_hae", 15),
                    ("operator_altitude_hae", 18),
                ],
            ),
        ] {
            let mut b = message(typ);
            let body = if typ == 1 { "location" } else { "system" };
            for (name, offset) in fields {
                b[offset..offset + 2].copy_from_slice(&u16::to_le_bytes(raw));
                let m = decode_message(&b, 0);
                let c = &m[body][name];
                assert_eq!(c["raw"], raw);
                assert_eq!(
                    c["decoded"],
                    numeric::decimal(i64::from(raw) * 5 - 10000, 1)
                );
                assert_eq!(c["arithmetic_value"], c["decoded"]);
                if raw == 65535 {
                    assert_eq!(c["status"], "out_of_stated_range");
                    assert_eq!(c["reason"], "arithmetic_exceeds_stated_range");
                }
            }
        }
    }
    for code in [
        0, 1, 255, 256, 65535, 65536, 0x12345678, 0x7fffffff, 0x80000000, 0xfffffffe, 0xffffffff,
    ] {
        for (typ, offset, body) in [(2, 4, "auth"), (4, 20, "system")] {
            let mut b = message(typ);
            b[offset..offset + 4].copy_from_slice(&u32::to_le_bytes(code));
            let m = decode_message(&b, 0);
            assert_eq!(m[body]["timestamp"]["decoded"], code.to_string());
            assert_eq!(m[body]["timestamp"]["status"], "known");
        }
    }
    assert!(numeric::hour_candidates(65535, Some(1000000), 0).is_empty());
    assert!(numeric::hour_candidates(1, None, 0).is_empty());
    assert_eq!(numeric::hour_candidates(1, Some(1000000), 0), vec![972001]);
    assert_eq!(
        numeric::hour_candidates(1, Some(972001), 10),
        vec![936001, 972001]
    );
}

#[test]
fn auth_pages_last_nibbles_and_methods() {
    for typ in 0..16 {
        for page in 0..16 {
            let mut b = message(2);
            b[1] = typ * 16 + page;
            let m = decode_message(&b, 0);
            let a = &m["auth"];
            assert_eq!(a["auth_type"]["raw"], typ);
            assert_eq!(a["data_page_number"]["raw"], page);
            for k in [
                "page0_reserved",
                "last_page",
                "length",
                "timestamp",
                "auth_data_page0",
            ] {
                assert_eq!(a[k]["status"] == "not_applicable", page != 0);
            }
            assert_eq!(
                a["auth_data_pages_1_15"]["status"] == "not_applicable",
                page == 0
            );
        }
    }
    for byte in 0..=255_u8 {
        let mut b = message(2);
        b[2] = byte;
        let m = decode_message(&b, 0);
        assert_eq!(m["auth"]["last_page"]["raw"], byte % 16);
        assert_eq!(m["auth"]["page0_reserved"]["raw"], byte / 16);
    }
    for method in 0..=255 {
        assert_eq!(
            auth::method_status(method),
            if method == 0 {
                "auth_method_reserved"
            } else if method >= 225 {
                "auth_method_private"
            } else {
                "auth_method_registered_profile_required"
            }
        );
    }
}

#[test]
fn all_prefixes_count_size_and_raw_interface() {
    let corpus: Value =
        serde_json::from_str(include_str!("../contract/golden-messages.json")).unwrap();
    let mut cases = 0;
    for v in corpus["vectors"].as_array().unwrap() {
        let b = unhex(v["raw_message_hex"].as_str().unwrap()).unwrap();
        for stop in 0..25 {
            let m = decode_message(&b[..stop], 0);
            assert_eq!(m["raw_hex"], hex(&b[..stop]));
            assert_ne!(m["parse_status"], "complete");
            cases += 1;
        }
    }
    assert_eq!(cases, 4375);
    let mut pack = vec![0xf2, 25, 9];
    for _ in 0..9 {
        pack.extend(message(0));
    }
    for stop in 0..228 {
        let u = decoder::decode_unit(&pack[..stop], 0, 0, None, true);
        assert_eq!(u["raw_hex"], hex(&pack[..stop]));
    }
    for size in 0..=255_u8 {
        for count in 0..=255_u8 {
            let mut pack = vec![0xf2, size, count];
            pack.extend(message(0));
            let u = decoder::decode_unit(&pack, 0, 0, None, true);
            assert_eq!(u["raw_hex"], hex(&pack));
            assert!(u["messages"].as_array().unwrap().len() <= 25);
        }
    }
    let fixtures = observations();
    let mut prefixes = 0;
    for f in fixtures["fixtures"].as_array().unwrap() {
        let base = capture::fixture_input(&f["record"]).unwrap();
        for stop in 0..base.bytes.len() {
            let mut r = base.clone();
            r.bytes.truncate(stop);
            let exact = r.bytes.clone();
            let out = capture::decode_capture(r);
            assert_eq!(out.raw_capture.bytes, exact);
            prefixes += 1;
        }
    }
    assert_eq!(prefixes, 645);
    for variant in 0..7 {
        let mut r = input();
        match variant {
            1 => r.bytes.clear(),
            2 => r.bytes.truncate(8),
            3 => r.bytes[1] = 0,
            4 => r.observation_id.capture_sequence = "9".into(),
            5 => r.bytes[2] ^= 1,
            6 => r.receiver.clear(),
            _ => {}
        }
        let original = serde_json::to_value(&r).unwrap();
        let out = capture::decode_capture(r);
        assert_eq!(serde_json::to_value(out.raw_capture).unwrap(), original);
    }
    let r = input();
    let b = r.bytes.clone();
    let out = capture::decode_with_limits(
        r,
        capture::Limits {
            capture_bytes: 0,
            units: 0,
        },
    );
    assert_eq!(out.raw_capture.bytes, b);
    assert_eq!(out.disposition, "quarantine");
    assert!(out.observation.is_null());
    // Repeated overlapping spans cannot amplify a small capture into an
    // unbounded decoded allocation. The exact capture and span declarations
    // are retained in quarantine, rather than deduplicated or partly dropped.
    let mut r = input();
    r.units.push(r.units[0].clone());
    let exact = serde_json::to_value(&r).unwrap();
    let limit = r.bytes.len();
    let out = capture::decode_with_limits(
        r,
        capture::Limits {
            capture_bytes: limit,
            units: 256,
        },
    );
    assert_eq!(out.disposition, "quarantine");
    assert!(out.observation.is_null());
    assert_eq!(serde_json::to_value(out.raw_capture).unwrap(), exact);
}

#[test]
fn text_and_binary_141_cases() {
    let patterns: [&[u8]; 7] = [
        b"",
        b"\0TAIL",
        b"AB\0CD",
        b"   ",
        b" A B ",
        b"A\xffB",
        b"SYNTHETIC",
    ];
    for id in 0..16 {
        for pattern in patterns {
            let mut b = message(0);
            b[1] = id * 16;
            b[2..2 + pattern.len()].copy_from_slice(pattern);
            let m = decode_message(&b, 0);
            let c = &m["basic_id"]["uas_id"];
            assert_eq!(c["raw"], hex(&b[2..22]));
            if id == 1 || id == 2 {
                if pattern == b"   " {
                    assert_eq!(c["decoded"], "   ");
                    assert_eq!(c["status"], "unknown");
                }
                if pattern == b" A B " {
                    assert_eq!(c["decoded"], " A B ");
                }
                if pattern == b"A\xffB" {
                    assert!(c["decoded"].is_null());
                    assert_eq!(c["status"], "out_of_range");
                }
            }
        }
    }
    for typ in [3, 5, 0] {
        for pattern in patterns {
            let mut b = message(typ);
            b[2..2 + pattern.len()].copy_from_slice(pattern);
            if typ == 0 {
                let n = 2 + pattern.len();
                assert_ne!(decode_message(&b[..n], 0)["parse_status"], "complete");
            } else {
                let m = decode_message(&b, 0);
                let (body, key, width) = if typ == 3 {
                    ("self_id", "description", 23)
                } else {
                    ("operator_id", "operator_id", 20)
                };
                assert_eq!(m[body][key]["raw"], hex(&b[2..2 + width]));
            }
        }
    }
    for typ in [0, 4] {
        for nonzero in [false, true] {
            let mut b = message(2);
            b[1] = typ * 16;
            b[8] = u8::from(nonzero);
            let m = decode_message(&b, 0);
            assert_eq!(
                m["auth"]["auth_data_page0"]["status"],
                if nonzero {
                    "out_of_range"
                } else if typ == 0 {
                    "unknown"
                } else {
                    "known"
                }
            );
        }
    }
    for case in 0..4 {
        let mut b = message(0);
        b[1] = if case == 3 { 0x40 } else { 0x30 };
        for (i, v) in b[2..22].iter_mut().enumerate() {
            *v = i as u8;
        }
        if case == 1 {
            b[8] = 0;
        }
        if case == 3 {
            b[2] = 1;
        }
        let m = decode_message(&b, 0);
        assert_eq!(m["basic_id"]["uas_id"]["decoded"], hex(&b[2..22]));
    }
}

#[test]
fn all_262_na_cells_and_1048_mutants() {
    fn cells(v: &Value, path: String, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                if v["status"] == "not_applicable" {
                    out.push(path.clone());
                }
                for (k, v) in o {
                    cells(v, format!("{path}/{k}"), out);
                }
            }
            Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    cells(v, format!("{path}/{i}"), out);
                }
            }
            _ => {}
        }
    }
    let fixtures = observations();
    let mut count = 0;
    for f in fixtures["fixtures"].as_array().unwrap() {
        let expected = &f["record"];
        let decoded =
            capture::decode_capture(capture::fixture_input(expected).unwrap()).observation;
        let mut paths = vec![];
        cells(&decoded, String::new(), &mut paths);
        for path in paths {
            let c = decoded.pointer(&path).unwrap();
            assert!(c["raw"].is_null() && c["decoded"].is_null());
            assert!(c["reason"].as_str().is_some_and(|s| !s.is_empty()));
            for variant in 0..4 {
                let mut changed = decoded.clone();
                if variant == 0 {
                    let (parent, key) = path.rsplit_once('/').unwrap();
                    changed
                        .pointer_mut(parent)
                        .unwrap()
                        .as_object_mut()
                        .unwrap()
                        .remove(key);
                    assert!(schema::verify_expected(expected, &changed).is_err());
                    continue;
                }
                let c = changed.pointer_mut(&path).unwrap();
                match variant {
                    0 => {
                        *c = Value::Null;
                    }
                    1 => c["raw"] = json!(0),
                    2 => c["decoded"] = json!(0),
                    _ => c["reason"] = Value::Null,
                };
                assert!(schema::verify_expected(expected, &changed).is_err());
            }
            count += 1;
        }
    }
    assert_eq!(count, 262);
}

#[test]
fn codec_domain_extensions_and_evolution() {
    for v in [
        Value::Null,
        json!(false),
        json!(0),
        json!(""),
        json!([]),
        json!({}),
        json!({"é":1,"z":2}),
        json!("\u{0001}\u{0008}\t\n\u{000c}\r"),
        json!("31767.5"),
        json!("18446744073709551615"),
        json!({"raw":"00"}),
        json!({"future":"00"}),
    ] {
        let j = codec::canonical_json(&v).unwrap();
        assert_eq!(codec::decode_json(&j).unwrap(), v);
        let b = codec::canonical_cbor(&v, &Value::Null).unwrap();
        assert_eq!(codec::decode_cbor(&b, &Value::Null).unwrap(), v);
    }
    for version in [None, Some(4), Some(5), Some(99)] {
        let mut v =
            json!({"future":{"hex-looking":"00","enum":999,"array":[null,false,0,"",[],{}]}});
        if let Some(n) = version {
            v["wire_format_version"] = json!(n);
        }
        let exact = serde_json::to_vec_pretty(&v).unwrap();
        let out = codec::inspect_parcel(&exact, "application/json");
        assert_eq!(out["exact_hex"], hex(&exact));
        assert_eq!(out["logical"], v);
    }
}

#[test]
fn codec_negative_controls_retain_exact_parcels() {
    let bad_json: Vec<Vec<u8>> = vec![
        br#"{"a":1,"a":2}"#.to_vec(),
        vec![0xff],
        br#""\ud800""#.to_vec(),
        b"1.0".to_vec(),
        b"1e0".to_vec(),
        b"9007199254740992".to_vec(),
        b"NaN".to_vec(),
        b"Infinity".to_vec(),
        vec![b'['; 65],
    ];
    for b in bad_json {
        assert!(codec::decode_json(&b).is_err());
        let p = codec::inspect_parcel(&b, "application/json");
        assert_eq!(p["exact_hex"], hex(&b));
        assert_eq!(p["disposition"], "quarantine");
    }
    for h in [
        "a201000100",
        "a2617800617801",
        "a16372617700",
        "a119ffff00",
        "d9ea614100",
        "d9ea604100",
        "4100",
        "bf00ff",
        "9fff",
        "1817",
        "a202000100",
        "7a00010000",
        "9bffffffffffffffff",
        "0000",
        "1a000000",
    ] {
        let b = unhex(h).unwrap();
        assert!(
            codec::decode_cbor(&b, schema::observation()).is_err(),
            "{h}"
        );
        let p = codec::inspect_parcel(&b, "application/cbor");
        assert_eq!(p["exact_hex"], h);
        assert_eq!(p["disposition"], "quarantine");
    }
}
