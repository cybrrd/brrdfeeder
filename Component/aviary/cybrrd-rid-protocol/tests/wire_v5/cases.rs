// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_protocol::v5::{self, assembly, codec, model, producer::*, schema, Error};
use serde_json::{json, Map, Value};

#[derive(Default)]
pub struct Ledger {
    pub events: Vec<Event>,
    pub fail: bool,
}
impl RawLedger for Ledger {
    fn append(&mut self, e: &Event) -> v5::Result<Receipt> {
        if self.fail {
            return Err(Error("injected append failure".into()));
        }
        self.events.push(e.clone());
        Ok(Receipt {
            ledger_record: self.events.len().to_string(),
        })
    }
}
pub fn event(bytes: Vec<u8>) -> Event {
    Event {
        identity: json!({"node_id":"test-node","boot_id":"test-boot","capture_sequence":"1"}),
        boundary: "test-service-data".into(),
        metadata: json!({"test":true}),
        bytes,
    }
}
pub fn capture_input(bytes: Vec<u8>, is_pack: bool) -> Capture {
    let length = bytes.len();
    Capture {
        event: event(bytes),
        format: "test-service-data".into(),
        source_kind: "synthetic".into(),
        adapter_version: "test-1".into(),
        completeness: "full_at_adapter_boundary".into(),
        original_length: None,
        unavailable_reason: None,
        transport_kind: "bt4_legacy".into(),
        spans: vec![UnitSpan {
            offset: 0,
            length,
            is_pack,
            counter: None,
        }],
        receiver_cells: Map::new(),
        transport_cells: Map::new(),
        related_captures: vec![],
    }
}
pub fn record(retained: Retained) -> Value {
    match retained.interpretation {
        Interpretation::V5(o) => o.into_record(),
        x => panic!("expected v5: {x:?}"),
    }
}
pub fn pack(messages: &[[u8; 25]]) -> Vec<u8> {
    let mut b = vec![0xf2, 25, messages.len() as u8];
    for m in messages {
        b.extend(m);
    }
    b
}
#[test]
fn changed_content_same_identity_is_appended() {
    let mut ledger = Ledger::default();
    let mut bytes = vec![0; 25];
    bytes[0] = 0x32;
    for n in [0u8, 0, 1] {
        bytes[24] = n;
        let mut c = capture_input(bytes.clone(), false);
        c.event.identity["node_id"] = json!("changed-content-fixture");
        let before = ledger.events.len();
        let result = capture(&mut ledger, c).unwrap();
        assert_eq!(
            ledger.events.len(),
            before + 1,
            "changed-content dedup lost a capture"
        );
        assert_eq!(ledger.events.last().unwrap().bytes, bytes);
        assert_eq!(result.event.bytes, bytes);
    }
    assert_eq!(ledger.events.len(), 3);
}
#[test]
fn ledger_first_no_position_and_no_dedup() {
    let mut ledger = Ledger::default();
    let mut count = 0;
    for kind in [0x22, 0x32, 0x42, 0x52] {
        let mut bytes = vec![0; 25];
        bytes[0] = kind;
        let before = ledger.events.len();
        let r = record(capture(&mut ledger, capture_input(bytes.clone(), false)).unwrap());
        assert_eq!(ledger.events.len(), before + 1);
        assert_eq!(ledger.events.last().unwrap().bytes, bytes);
        assert_eq!(r["rid_units"][0]["messages"].as_array().unwrap().len(), 1);
        assert!(r["rid_units"][0]["messages"][0].get("location").is_none());
        assert!(r["rid_units"][0]["messages"][0].get("basic_id").is_none());
        for media in [Media::Json, Media::CborRevision0] {
            let encoded = match media {
                Media::Json => codec::canonical_json(&r).unwrap(),
                Media::CborRevision0 => {
                    codec::canonical_cbor(&r, schema::contract().unwrap()).unwrap()
                }
            };
            assert_eq!(
                record(test_ingress(&mut ledger, event(encoded), media).unwrap()),
                r
            );
            count += 1;
        }
        // Same observation identity/counter with equal or changed bytes is still
        // appended. Association/conflict policy must never remove source input.
        capture(&mut ledger, capture_input(bytes.clone(), false)).unwrap();
        bytes[24] = 1;
        capture(&mut ledger, capture_input(bytes, false)).unwrap();
        count += 3;
    }
    let n = ledger.events.len();
    ledger.fail = true;
    assert!(capture(&mut ledger, capture_input(vec![0xff; 25], false)).is_err());
    assert!(test_ingress(&mut ledger, event(b"{}".to_vec()), Media::Json).is_err());
    assert_eq!(ledger.events.len(), n);
    assert_eq!(count, 20);
    println!("P0_NO_POSITION_CAPTURE_INGRESS_CASES={count}/20; APPEND_FAILURES=2/2");
}
#[test]
fn all_pack_counts_versions_and_duplicate_ids() {
    let mut count = 0;
    for n in [0u8, 1, 9, 10, 255] {
        let mut raw = vec![0xf2, 25, n];
        for i in 0..n {
            let mut m = [0; 25];
            m[0] = 2;
            m[1] = 0x12;
            m[2] = b'A' + i % 20;
            raw.extend(m);
        }
        let u = v5::decode_unit(&raw, true, 0, 0, 0, Some(255)).unwrap();
        assert_eq!(u["messages"].as_array().unwrap().len(), n as usize);
        assert_eq!(u["basic_ids"].as_array().unwrap().len(), n as usize);
        for (i, m) in u["messages"].as_array().unwrap().iter().enumerate() {
            assert_eq!(m["index"], i);
            assert_eq!(u["basic_ids"][i]["message_index"], i);
        }
        count += 1;
    }
    for version in 0u8..16 {
        for kind in 0u8..16 {
            let mut m = [0; 25];
            m[0] = kind * 16 + version;
            let decoded = v5::decode_message(&m, 0).unwrap();
            model::validate_message(&decoded).unwrap();
            if version != 2 {
                assert_eq!(decoded["parse_status"], "unsupported_version");
            }
            if kind > 5 && version == 2 {
                assert_eq!(decoded["parse_status"], "unsupported_type");
            }
            count += 1;
        }
        let mut m = [0; 25];
        m[0] = 2;
        let mut b = pack(&[m]);
        b[0] = 0xf0 | version;
        let u = v5::decode_unit(&b, true, 0, 0, 0, None).unwrap();
        if version != 2 {
            assert!(u["messages"][0].get("basic_id").is_none());
            assert_eq!(u["messages"][0]["parse_status"], "unsupported_version");
        }
        count += 1;
    }
    for typ in 0u8..16 {
        let mut a = [0; 25];
        a[0] = 2;
        a[1] = typ * 16 + 2;
        a[2] = b'A';
        let mut b = a;
        b[2] = b'B';
        let raw = pack(&[a, a, b]);
        let u = v5::decode_unit(&raw, true, 0, 0, 0, Some(0)).unwrap();
        assert_eq!(u["basic_ids"].as_array().unwrap().len(), 3);
        for i in 0..3 {
            assert_eq!(u["basic_ids"][i]["id_type"]["raw"], typ);
            assert_eq!(u["basic_ids"][i]["message_index"], i);
        }
        assert_ne!(
            u["basic_ids"][0]["uas_id"]["raw"],
            u["basic_ids"][2]["uas_id"]["raw"]
        );
        count += 1;
    }
    assert_eq!(count, 293);
    println!("P0_PACK_VERSION_MULTIPLICITY_CASES={count}/293");
}
#[test]
fn every_truncation_and_malformed_parcel_is_retained() {
    let fixtures = super::messages();
    let mut ledger = Ledger::default();
    let mut count = 0;
    for x in fixtures["vectors"].as_array().unwrap() {
        let raw = v5::unhex(x["raw_message_hex"].as_str().unwrap()).unwrap();
        for n in 0..=raw.len() {
            let bytes = raw[..n].to_vec();
            let retained = capture(&mut ledger, capture_input(bytes.clone(), false)).unwrap();
            assert_eq!(retained.event.bytes, bytes);
            assert_eq!(ledger.events.last().unwrap().bytes, bytes);
            count += 1;
        }
    }
    for raw in [
        vec![],
        vec![0xf2],
        vec![0xf2, 25],
        vec![0xf2, 0, 3, 7, 8],
        vec![0xf2, 24, 1, 0, 2],
        vec![0xf2, 26, 255, 1],
        vec![0xf2, 25, 0, 1, 2, 3],
    ] {
        let retained = capture(&mut ledger, capture_input(raw.clone(), true)).unwrap();
        assert_eq!(retained.event.bytes, raw);
        count += 1;
    }
    let invalid_json: &[&[u8]] = &[
        b"",
        b"{",
        b"{\"a\":1,\"a\":2}",
        b"{\"a\":1.0}",
        b"{\"a\":9007199254740992}",
        b"{\"a\":\"\\ud800\"}",
        b"{\"wire_format_version\":6}",
        b"{\"wire_format_version\":5}",
        b"\xff",
    ];
    for bytes in invalid_json {
        let retained = test_ingress(&mut ledger, event(bytes.to_vec()), Media::Json).unwrap();
        assert_eq!(retained.event.bytes, *bytes);
        assert!(matches!(
            retained.interpretation,
            Interpretation::Unsupported { .. }
        ));
        count += 1;
    }
    let invalid_cbor = [
        vec![0x9f, 0xff],
        vec![0x40],
        vec![0xd9, 0xea, 0x60, 0x40],
        vec![0xc0, 0x60],
        vec![0xa2, 0x61, b'x', 0, 0x61, b'x', 1],
        vec![0xa1, 0x19, 0xff, 0xff, 0],
        vec![0xf9, 0, 0],
    ];
    for bytes in invalid_cbor {
        let retained =
            test_ingress(&mut ledger, event(bytes.clone()), Media::CborRevision0).unwrap();
        assert_eq!(retained.event.bytes, bytes);
        assert!(matches!(
            retained.interpretation,
            Interpretation::Unsupported { .. }
        ));
        count += 1;
    }
    let bytes = vec![0; v5::MAX_PARCEL_BYTES + 1];
    let retained = test_ingress(&mut ledger, event(bytes.clone()), Media::Json).unwrap();
    assert_eq!(retained.event.bytes, bytes);
    assert!(matches!(
        retained.interpretation,
        Interpretation::Unsupported { .. }
    ));
    count += 1;
    assert_eq!(count, 4574);
    println!("P0_RETAINED_MALFORMED_CASES={count}/4574");
}
fn page(n: u8) -> assembly::Page {
    let mut raw = [0u8; 25];
    raw[0] = 0x22;
    raw[1] = 0x10 | n;
    assembly::Page {
        observation_id: json!({"node_id":"test","boot_id":"boot","capture_sequence":"1"}),
        unit_index: 0,
        message_index: n as usize,
        raw,
        source_mac: "02:00:00:00:00:01".into(),
        counter: 255,
        association: None,
        transport_kind: "bt5_secondary".into(),
    }
}
#[test]
fn authentication_order_conflicts_additional_data_and_association() {
    let mut pages: Vec<_> = (0..16).map(page).collect();
    pages[0].raw[2] = 0xaf;
    pages[0].raw[3] = 255;
    let mut stream = vec![0x81; 255];
    stream.push(106);
    stream.extend(vec![0x42; 106]);
    assert_eq!(stream.len(), 362);
    let mut offset = 0;
    for (n, p) in pages.iter_mut().enumerate() {
        let start = if n == 0 { 8 } else { 2 };
        let len = 25 - start;
        p.raw[start..].copy_from_slice(&stream[offset..offset + len]);
        offset += len;
    }
    let original = pages.clone();
    pages.rotate_left(1);
    let a = assembly::reconstruct(&pages).unwrap();
    assert_eq!(a["state"], "complete");
    assert_eq!(a["payload_hex"], "81".repeat(255));
    assert_eq!(a["additional_data_hex"], "42".repeat(106));
    assert_eq!(a["padding_hex"], "");
    assert_eq!(a["verification"], "not_performed");
    let mut count = 1;
    for n in 0..16 {
        let mut missing = original.clone();
        missing.remove(n);
        assert_eq!(
            assembly::reconstruct(&missing).unwrap()["state"],
            "incomplete"
        );
        count += 1;
    }
    let mut duplicate = original.clone();
    duplicate.push(original[4].clone());
    let a = assembly::reconstruct(&duplicate).unwrap();
    assert_eq!(a["page_refs"].as_array().unwrap().len(), 17);
    assert_eq!(a["state"], "complete");
    count += 1;
    duplicate[16].raw[3] ^= 1;
    assert_eq!(
        assembly::reconstruct(&duplicate).unwrap()["state"],
        "conflicting"
    );
    count += 1;
    for change in 0..6 {
        let mut p = original.clone();
        match change {
            0 => p[1].counter = 0,
            1 => p[1].source_mac = "02:00:00:00:00:02".into(),
            2 => p[1].observation_id["boot_id"] = json!("other"),
            3 => p[1].observation_id["capture_sequence"] = json!("257"),
            4 => p[1].raw[0] = 0x21,
            _ => p[1].raw[1] = 0x21,
        }
        assert_eq!(assembly::reconstruct(&p).unwrap()["state"], "ambiguous");
        count += 1;
    }
    for length in [0u8, 17, 18, 255] {
        let mut p = page(0);
        p.raw[3] = length;
        let a = assembly::reconstruct(&[p]).unwrap();
        assert_eq!(
            a["state"],
            if length <= 17 {
                "complete"
            } else {
                "incomplete"
            }
        );
        count += 1;
    }
    for additional in [0u8, 15, 16, 255] {
        let mut p = page(0);
        p.raw[8] = additional;
        let a = assembly::reconstruct(&[p]).unwrap();
        assert_eq!(
            a["state"],
            if additional <= 16 {
                "complete"
            } else {
                "incomplete"
            }
        );
        count += 1;
    }
    for method in 0u8..=255 {
        let mut p = page(0);
        p.raw[1] = 0x50;
        p.raw[3] = 1;
        p.raw[8] = method;
        let a = assembly::reconstruct(&[p]).unwrap();
        assert_eq!(a["verification"], "not_performed");
        let issues = a["issues"].as_array().unwrap();
        assert!(issues.contains(&json!(if method == 0 {
            "specific_method_reserved"
        } else if method >= 225 {
            "specific_method_private_source_conflict"
        } else {
            "specific_method_profile_required"
        })));
        count += 1;
    }
    let mut p = page(0);
    p.raw[1] = 0x30;
    p.transport_kind = "bt4_legacy".into();
    assert!(assembly::reconstruct(&[p]).unwrap()["issues"]
        .as_array()
        .unwrap()
        .contains(&json!("message_set_transport_constraint")));
    count += 1;
    let mut p = page(0);
    p.raw[24] = 1;
    assert!(assembly::reconstruct(&[p]).unwrap()["issues"]
        .as_array()
        .unwrap()
        .contains(&json!("nonzero_padding")));
    count += 1;
    assert_eq!(count, 291);
    println!("P0_AUTH_ASSEMBLY_CASES={count}/291");
}
#[test]
fn ingress_versions_and_codec_domain_controls() {
    let mut ledger = Ledger::default();
    for raw in [
        b"{}".as_slice(),
        b"{\"wire_format_version\":4}",
        b"{\"wire_format_version\":9}",
    ] {
        let r = test_ingress(&mut ledger, event(raw.to_vec()), Media::Json).unwrap();
        assert!(match raw {
            b"{}" => matches!(r.interpretation, Interpretation::HistoricUnversioned(_)),
            b"{\"wire_format_version\":4}" => matches!(r.interpretation, Interpretation::V4(_)),
            _ => matches!(r.interpretation, Interpretation::Unsupported { .. }),
        });
    }
    let v = json!({"x":"deadbeef","unicode":"é𝄞\u{1f}","null":null,"empty":"","list":[],"zero":0,"bool":false,"min":-9007199254740991i64,"max":9007199254740991i64});
    let s = json!({});
    let c = codec::canonical_cbor(&v, &s).unwrap();
    assert_eq!(codec::parse_cbor(&c, &s).unwrap(), v);
    assert_eq!(
        codec::parse_json(&codec::canonical_json(&v).unwrap()).unwrap(),
        v
    );
    assert!(codec::parse_cbor(&[0x18, 0], &s).is_err());
    assert!(codec::parse_cbor(&[0xf8, 20], &s).is_err());
    assert!(codec::parse_cbor(&[0xa1, 0x65, b'i', b'n', b'd', b'e', b'x', 0], &s).is_err());
    println!("P0_INGRESS_EVOLUTION_CASES=3/3; P0_CODEC_DOMAIN_CONTROL_GROUPS=4/4");
}

#[test]
fn text_binary_and_auth_applicability_domains() {
    let mut texts = 0;
    let mut ids = 0;
    let mut auth = 0;
    for (header, kind, body, name, length) in [
        (0x02, 0x12, "basic_id", "uas_id", 20),
        (0x02, 0x22, "basic_id", "uas_id", 20),
        (0x32, 0, "self_id", "description", 23),
        (0x52, 0, "operator_id", "operator_id", 20),
    ] {
        for pos in [0, 9, length - 1] {
            for byte in 0u16..256 {
                let mut b = [0u8; 25];
                b[0] = header;
                b[1] = kind;
                b[2..2 + length].fill(b'A');
                b[2 + pos] = byte as u8;
                let m = v5::decode_message(&b, 0).unwrap();
                let cell = &m[body][name];
                assert_eq!(cell["raw"], v5::hex(&b[2..2 + length]));
                if byte == 0 && pos < length - 1 {
                    assert_eq!(cell["status"], "out_of_range");
                    assert_eq!(cell["reason"], "nonzero_after_nul");
                    assert_eq!(cell["decoded"], "A".repeat(pos));
                } else if byte != 0 && !(32..=126).contains(&byte) {
                    assert!(cell["decoded"].is_null());
                    assert_eq!(cell["reason"], "invalid_ascii_text");
                } else {
                    assert_eq!(cell["status"], "known");
                }
                texts += 1;
            }
        }
    }
    for kind in 0u8..16 {
        let mut b = [0u8; 25];
        b[0] = 2;
        b[1] = kind * 16;
        let m = v5::decode_message(&b, 0).unwrap();
        let cell = &m["basic_id"]["uas_id"];
        assert_eq!(cell["status"], "unknown");
        assert_eq!(cell["raw"], "00".repeat(20));
        assert_eq!(
            cell["decoded"],
            if kind == 1 || kind == 2 {
                "".into()
            } else {
                "00".repeat(20)
            }
        );
        ids += 1;
        b[3] = 0xff;
        let m = v5::decode_message(&b, 0).unwrap();
        let cell = &m["basic_id"]["uas_id"];
        assert_eq!(cell["raw"], v5::hex(&b[2..22]));
        if kind == 3 {
            assert_eq!(cell["status"], "known");
            assert_eq!(cell["decoded"], cell["raw"]);
        } else if kind == 4 {
            assert_eq!(cell["status"], "out_of_range");
            assert_eq!(cell["reason"], "session_method_reserved");
        } else if kind > 4 {
            assert_eq!(cell["status"], "reserved");
        }
        assert_eq!(m["basic_id"]["id_type"]["raw"], kind);
        ids += 1;
    }
    for method in 0u8..=255 {
        let mut b = [0u8; 25];
        b[0] = 2;
        b[1] = 0x42;
        b[2] = method;
        b[3] = 1;
        let m = v5::decode_message(&b, 0).unwrap();
        assert_eq!(
            m["basic_id"]["uas_id"]["status"],
            if method == 0 {
                "out_of_range"
            } else if method >= 225 {
                "private_use"
            } else {
                "known"
            }
        );
        ids += 1;
    }
    for kind in 0u8..16 {
        for page in 0u8..16 {
            for nonzero in [false, true] {
                let mut b = [0u8; 25];
                b[0] = 0x22;
                b[1] = kind * 16 + page;
                let start = if page == 0 { 8 } else { 2 };
                if nonzero {
                    b[start] = 1;
                }
                let m = v5::decode_message(&b, 0).unwrap();
                let a = &m["auth"];
                let name = if page == 0 {
                    "auth_data_page0"
                } else {
                    "auth_data_pages_1_15"
                };
                assert_eq!(a[name]["raw"], v5::hex(&b[start..]));
                assert_eq!(a[name]["decoded"], a[name]["raw"]);
                assert_eq!(
                    a[name]["status"],
                    if nonzero && (kind == 0 || kind == 4) {
                        "out_of_range"
                    } else if kind == 0 {
                        "unknown"
                    } else {
                        "known"
                    }
                );
                let na: Vec<&str> = if page == 0 {
                    vec!["auth_data_pages_1_15"]
                } else {
                    vec![
                        "page0_reserved",
                        "last_page",
                        "length",
                        "timestamp",
                        "auth_data_page0",
                    ]
                };
                for name in na {
                    assert_eq!(a[name]["status"], "not_applicable");
                    assert!(a[name]["raw"].is_null());
                    assert!(a[name]["decoded"].is_null());
                    assert_eq!(a[name]["reason"], "opposite_authentication_page_shape");
                }
                auth += 1;
            }
        }
    }
    assert_eq!(texts, 3072);
    assert_eq!(ids, 288);
    assert_eq!(auth, 512);
    println!("P0_TEXT_BYTE_CASES={texts}/3072; ID_BINARY_METHOD_CASES={ids}/288; AUTH_TYPE_PAGE_DATA_CASES={auth}/512");
}

#[test]
fn every_seed_observation_capture_truncation_is_retained() {
    let fixtures = super::observations();
    let mut ledger = Ledger::default();
    let mut count = 0;
    for fixture in fixtures["fixtures"].as_array().unwrap() {
        let source = &fixture["record"];
        let bytes = v5::unhex(source["captures"][0]["raw_hex"].as_str().unwrap()).unwrap();
        for n in 0..=bytes.len() {
            let truncated = bytes[..n].to_vec();
            let mut c = capture_input(truncated.clone(), false);
            c.transport_kind = source["captures"][0]["transport_kind"]
                .as_str()
                .unwrap()
                .into();
            c.spans.clear();
            if n > 0 && !source["rid_units"].as_array().unwrap().is_empty() {
                c.spans.push(UnitSpan {
                    offset: 1,
                    length: n - 1,
                    is_pack: source["rid_units"][0]["kind"] == "message_pack",
                    counter: Some(bytes[0]),
                });
            }
            let retained = capture(&mut ledger, c).unwrap();
            assert_eq!(retained.event.bytes, truncated);
            assert_eq!(ledger.events.last().unwrap().bytes, truncated);
            count += 1;
        }
    }
    assert!(count > 0);
    println!("P0_SEED_CAPTURE_TRUNCATIONS={count}/{count}");
}

#[test]
fn supplied_authentication_assembly_payload_golden() {
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/golden-authentication-assembly.json")).unwrap();
    let vectors = super::messages();
    let mut pages = Vec::new();
    for n in 0u8..16 {
        let name = format!("auth_page_{n}");
        let vector = vectors["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == name)
            .unwrap();
        let mut p = page(n);
        p.raw = v5::unhex(vector["raw_message_hex"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        p.counter = 7;
        p.message_index = 0;
        p.observation_id["capture_sequence"] = json!(n.to_string());
        p.association = Some("synthetic independent grouping fixture".into());
        pages.push(p);
    }
    // This synthetic recipe uses all remaining capacity for additional data:
    // the first byte after the 255-byte payload is page 11, data offset 8.
    pages[11].raw[10] = 106;
    let actual = assembly::reconstruct(&pages).unwrap();
    for field in [
        "state",
        "counter_raw",
        "auth_type_raw",
        "last_page_raw",
        "payload_length_raw",
        "timestamp_raw",
        "payload_hex",
        "additional_length_raw",
        "additional_data_hex",
        "padding_hex",
    ] {
        assert_eq!(actual[field], expected[field], "assembly field {field}");
    }
    assert_eq!(actual["page_refs"].as_array().unwrap().len(), 16);
    assert_eq!(actual["verification"], "not_performed");
    println!("P0_SUPPLIED_AUTH_ASSEMBLY_PAYLOAD_GOLDEN=1/1");
}

#[test]
fn contradictory_metadata_is_retained_without_repair() {
    let mut ledger = Ledger::default();
    let bytes = vec![0x32; 25];
    let good = record(capture(&mut ledger, capture_input(bytes.clone(), false)).unwrap());
    for name in ["raw_frame_bytes", "frame_length", "node_id", "transport"] {
        let mut c = capture_input(bytes.clone(), false);
        let mut supplied = good["receiver"][name].clone();
        supplied["raw"] = json!("contradictory input");
        c.receiver_cells.insert(name.into(), supplied.clone());
        let retained = capture(&mut ledger, c).unwrap();
        assert!(matches!(
            retained.interpretation,
            Interpretation::Unsupported { .. }
        ));
        assert_eq!(retained.event.metadata["receiver_cells"][name], supplied);
        assert_eq!(ledger.events.last().unwrap(), &retained.event);
    }
    println!("P0_CONTRADICTORY_METADATA_RETENTION=4/4");
}
