// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use super::cases::{capture_input, pack, record, Ledger};
use cybrrd_rid_protocol::v5::{
    self,
    adapters::{self, Profile},
    codec,
    producer::*,
    schema,
};
use serde_json::Value;

pub fn message(kind: u8) -> [u8; 25] {
    let mut m = [0; 25];
    m[0] = kind * 16 + 2;
    if kind == 0 {
        m[1] = 0x12;
        m[2..8].copy_from_slice(b"TEST01");
    }
    m
}
pub fn hci(extended: bool, counter: u8) -> Vec<u8> {
    let payload = if extended {
        pack(&[message(0), message(2)])
    } else {
        message(0).to_vec()
    };
    let mut ad = vec![(payload.len() + 5) as u8, 0x16, 0xfa, 0xff, 0x0d, counter];
    ad.extend(payload);
    let mut header = vec![0; if extended { 24 } else { 9 }];
    if extended {
        header[3..9].copy_from_slice(&[1, 0, 0, 0, 0, 2]);
        header[9] = 1;
        header[10] = 3;
        header[11] = 9;
        header[13] = 212;
        header[23] = ad.len() as u8;
    } else {
        header[0] = 3;
        header[2..8].copy_from_slice(&[1, 0, 0, 0, 0, 2]);
        header[8] = ad.len() as u8;
    }
    let mut bytes = vec![
        4,
        0x3e,
        (2 + header.len() + ad.len() + usize::from(!extended)) as u8,
        if extended { 13 } else { 2 },
        1,
    ];
    bytes.extend(header);
    bytes.extend(ad);
    if !extended {
        bytes.push(214);
    }
    bytes
}
fn attribute(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut a = vec![kind];
    a.extend((body.len() as u16).to_le_bytes());
    a.extend(body);
    a
}
fn descriptor(counter: u8) -> Vec<u8> {
    let p = pack(&[message(2)]);
    let mut body = vec![
        0x88,
        0x69,
        0x19,
        0x9d,
        0x92,
        0x09,
        1,
        0,
        0x10,
        (p.len() + 1) as u8,
        counter,
    ];
    body.extend(p);
    attribute(3, &body)
}
pub fn wifi(kind: &str, duplicates: bool) -> Vec<u8> {
    let mut frame = vec![0; if kind == "wifi_nan_sdf" { 24 } else { 36 }];
    frame[0] = if kind == "wifi_nan_sdf" { 0xd0 } else { 0x80 };
    frame[4..10].fill(255);
    frame[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
    match kind {
        "wifi_beacon" => {
            let mut body = vec![0xfa, 0x0b, 0xbc, 0x0d, 7];
            body.extend(pack(&[message(3)]));
            frame.extend([0xdd, body.len() as u8]);
            frame.extend(body);
        }
        "wifi_nan_sdf" => {
            frame.extend([4, 9, 0x50, 0x6f, 0x9a, 0x13]);
            frame.extend(descriptor(7));
            if duplicates {
                frame.extend(descriptor(8));
            }
            frame.extend(attribute(0x0e, &[1, 0, 2, 0x24]));
        }
        "wifi_nan_sync" => {
            let mut body = vec![0x50, 0x6f, 0x9a, 0x13];
            body.extend(attribute(0, &[1, 2]));
            body.extend(attribute(1, &[0; 13]));
            frame.extend([0xdd, body.len() as u8]);
            frame.extend(body);
        }
        _ => panic!("unknown test kind"),
    }
    // Independent basic radiotap layout: flags/rate/channel/signal/noise/antenna.
    let mut bytes = vec![
        0, 0, 17, 0, 0x6e, 0x08, 0, 0, 0x50, 12, 0x6c, 0x09, 0, 0, 214, 166, 1,
    ];
    bytes.extend(frame);
    bytes.extend([0xde, 0xad, 0xbe, 0xef]);
    bytes
}
pub fn all_fixtures() -> Vec<(Vec<u8>, Profile, &'static str)> {
    vec![
        (hci(false, 7), Profile::HciSingleReport, "bt4_legacy"),
        (hci(true, 255), Profile::HciSingleReport, "bt5_secondary"),
        (
            wifi("wifi_beacon", false),
            Profile::WifiRadiotapBasic,
            "wifi_beacon",
        ),
        (
            wifi("wifi_nan_sdf", false),
            Profile::WifiRadiotapBasic,
            "wifi_nan_sdf",
        ),
        (
            wifi("wifi_nan_sync", false),
            Profile::WifiRadiotapBasic,
            "wifi_nan_sync",
        ),
    ]
}
fn output(bytes: Vec<u8>, profile: Profile, ledger: &mut Ledger) -> Value {
    let mut c = capture_input(bytes, false);
    c.event.boundary = match profile {
        Profile::HciSingleReport => "hci-single-report-v1",
        Profile::WifiRadiotapBasic => "radiotap-basic-v1",
    }
    .into();
    c.spans.clear();
    record(adapters::capture(ledger, c, profile).unwrap())
}
#[test]
fn independent_adapter_layouts_preserve_frames_and_exposure() {
    let mut ledger = Ledger::default();
    let mut count = 0;
    for (bytes, profile, kind) in all_fixtures() {
        let r = output(bytes.clone(), profile, &mut ledger);
        assert_eq!(ledger.events.last().unwrap().bytes, bytes);
        assert_eq!(r["captures"][0]["raw_hex"], v5::hex(&bytes));
        assert_eq!(r["receiver"]["raw_frame_bytes"]["raw"], v5::hex(&bytes));
        assert_eq!(r["captures"][0]["transport_kind"], kind);
        assert_eq!(r["receiver"]["source_MAC"]["decoded"], "020000000001");
        let groups = r["captures"][0]["transport_fields"].as_object().unwrap();
        assert_eq!(groups.len(), 6);
        let mut homes = 0;
        for (name, fields) in groups {
            for (key, cell) in fields.as_object().unwrap() {
                if key == "extensions" {
                    continue;
                }
                homes += 1;
                if name != kind {
                    assert_eq!(cell["status"], "not_applicable");
                    assert!(cell["raw"].is_null());
                }
            }
        }
        assert_eq!(homes, 44);
        assert_eq!(r["receiver"].as_object().unwrap().len(), 35);
        assert_eq!(r["network_information"].as_object().unwrap().len(), 4);
        for unit in r["rid_units"].as_array().unwrap() {
            let a = unit["capture_offset"].as_u64().unwrap() as usize;
            let raw = v5::unhex(unit["raw_hex"].as_str().unwrap()).unwrap();
            assert_eq!(&bytes[a..a + raw.len()], raw);
            for m in unit["messages"].as_array().unwrap() {
                assert_eq!(m["header"]["protocol_version"]["raw"], 2);
            }
        }
        if kind.starts_with("bt") {
            assert_eq!(
                r["captures"][0]["transport_fields"][kind]["access_address"]["status"],
                "not_exposed"
            );
            assert_eq!(
                r["captures"][0]["transport_fields"][kind]["crc"]["status"],
                "not_exposed"
            );
        } else {
            assert_eq!(r["receiver"]["FCS_status"]["raw"], "verified_bad");
            assert_eq!(r["receiver"]["RSSI_dbm"]["raw"], "-42");
            assert_eq!(r["receiver"]["noise_floor"]["raw"], "-90");
            assert_eq!(
                r["receiver"]["PHY_data_rate"]["decoded"]["bitrate_bps"],
                "6000000"
            );
            assert_eq!(
                r["receiver"]["wifi_rx_channel_and_source"]["raw"]["frequency_mhz"],
                "2412"
            );
        }
        if kind == "wifi_nan_sync" {
            assert!(r["rid_units"].as_array().unwrap().is_empty());
        }
        let cbor = codec::canonical_cbor(&r, schema::contract().unwrap()).unwrap();
        assert_eq!(
            codec::parse_cbor(&cbor, schema::contract().unwrap()).unwrap(),
            r
        );
        assert_eq!(
            record(
                test_ingress(&mut ledger, super::cases::event(cbor), Media::CborRevision0).unwrap()
            ),
            r
        );
        count += 1;
    }
    let r = output(
        wifi("wifi_nan_sdf", true),
        Profile::WifiRadiotapBasic,
        &mut ledger,
    );
    assert_eq!(r["rid_units"].as_array().unwrap().len(), 2);
    assert_eq!(r["rid_units"][0]["message_counter"]["raw"], 7);
    assert_eq!(r["rid_units"][1]["message_counter"]["raw"], 8);
    assert_eq!(
        r["captures"][0]["transport_fields"]["wifi_nan_sdf"]["message_counter"]["status"],
        "ambiguous"
    );
    count += 1;
    assert_eq!(count, 6);
    println!("P0_ADAPTER_BOUNDARY_FIXTURES={count}/6; TRANSPORT_HOMES=44/44; RECEIVER_HOMES=35/35; NETWORK_HOMES=4/4");
}
#[test]
fn every_adapter_truncation_and_byte_flip_retains_input() {
    let mut count = 0;
    let mut ledger = Ledger::default();
    for (bytes, profile, _) in all_fixtures() {
        for n in 0..bytes.len() {
            let input = bytes[..n].to_vec();
            let r = adapters::capture(&mut ledger, capture_input(input.clone(), false), profile)
                .unwrap();
            assert_eq!(r.event.bytes, input);
            assert_eq!(ledger.events.last().unwrap().bytes, input);
            count += 1;
        }
        for n in 0..bytes.len() {
            let mut input = bytes.clone();
            input[n] ^= 0xff;
            let r = adapters::capture(&mut ledger, capture_input(input.clone(), false), profile)
                .unwrap();
            assert_eq!(r.event.bytes, input);
            assert_eq!(ledger.events.last().unwrap().bytes, input);
            count += 1;
        }
    }
    assert!(count > 0);
    println!("P0_ADAPTER_TRUNCATION_FLIP_CASES={count}/{count}");
    let mut input = capture_input(hci(false, 7), false);
    input.event.bytes[4] = 2;
    assert!(matches!(
        adapters::capture(&mut ledger, input, Profile::HciSingleReport)
            .unwrap()
            .interpretation,
        Interpretation::Unsupported { .. }
    ));
    ledger.fail = true;
    assert!(adapters::capture(
        &mut ledger,
        capture_input(vec![], false),
        Profile::WifiRadiotapBasic
    )
    .is_err());
}

#[test]
fn both_profiles_refuse_preinterpreted_transport_cells() {
    let mut count = 0;
    for (bytes, profile, kind) in [
        (hci(false, 7), Profile::HciSingleReport, "bt4_legacy"),
        (
            wifi("wifi_beacon", false),
            Profile::WifiRadiotapBasic,
            "wifi_beacon",
        ),
    ] {
        let mut ledger = Ledger::default();
        // A valid, consistent cell from the same frame must still be refused:
        // the raw adapter owns interpretation, not its caller.
        let good = output(bytes.clone(), profile, &mut ledger);
        let cell = good["captures"][0]["transport_fields"][kind]["message_counter"].clone();
        let mut input = capture_input(bytes.clone(), false);
        input.spans.clear();
        input
            .transport_cells
            .insert("message_counter".into(), cell.clone());
        let retained = adapters::capture(&mut ledger, input, profile).unwrap();
        let Interpretation::Unsupported { reason } = &retained.interpretation else {
            panic!("{profile:?} admitted preinterpreted transport cells");
        };
        assert!(
            reason.contains("preinterpreted transport cells"),
            "{profile:?}: {reason}"
        );
        assert_eq!(retained.event.bytes, bytes);
        assert_eq!(ledger.events.len(), 2);
        assert_eq!(ledger.events[1], retained.event);
        assert_eq!(
            retained.event.metadata["transport_cells"]["message_counter"],
            cell
        );
        count += 1;
    }
    assert_eq!(count, 2);
    println!("P0_PREINTERPRETED_PROFILE_REFUSALS=2/2; VALID_PROFILE_CONTROLS=2/2");
}

fn wifi_at_cap(n: usize) -> Vec<u8> {
    assert!((2395..=2397).contains(&n));
    // 64 radiotap + 24 MAC + (12 beacon fixed + 327 minimal RID IEs
    // + one non-RID padding IE) + 4 FCS. Only cap+1 is ever over-cap.
    let mut b = vec![0; 64];
    b[..8].copy_from_slice(&[0, 0, 64, 0, 2, 0, 0, 0]);
    b[8] = 0x10;
    let mut frame = vec![0; 36];
    frame[0] = 0x80;
    for counter in 0..327 {
        frame.extend([0xdd, 5, 0xfa, 0x0b, 0xbc, 0x0d, (counter & 255) as u8]);
    }
    let padding = n - (64 + frame.len() + 4);
    frame.extend([0, (padding - 2) as u8]);
    frame.resize(frame.len() + padding - 2, 0);
    b.extend(frame);
    b.extend([0; 4]);
    assert_eq!(b.len(), n);
    b
}
fn check_profile_cap(profile: Profile, cap: usize, inputs: [Vec<u8>; 3]) {
    assert_eq!(profile.max_capture_bytes(), cap);
    for (index, bytes) in inputs.into_iter().enumerate() {
        let n = cap - 1 + index;
        assert_eq!(bytes.len(), n);
        let mut input = capture_input(bytes.clone(), false);
        input.spans.clear();
        let mut ledger = Ledger::default();
        let retained = adapters::capture(&mut ledger, input, profile).unwrap();
        assert_eq!(ledger.events.len(), 1);
        assert_eq!(ledger.events[0], retained.event);
        assert_eq!(retained.event.bytes, bytes);
        if n > cap {
            let Interpretation::Unsupported { reason } = &retained.interpretation else {
                panic!("{profile:?} admitted cap+1 input");
            };
            assert!(
                reason.contains(&format!("exceeds {cap}-byte input cap")),
                "{profile:?} cap reason: {reason}"
            );
            // Unsupported has no Observation and therefore no produced RID units.
        } else {
            let Interpretation::V5(observation) = &retained.interpretation else {
                panic!(
                    "{profile:?} refused in-cap input: {:?}",
                    retained.interpretation
                );
            };
            assert_eq!(
                observation.record()["captures"][0]["raw_hex"],
                v5::hex(&bytes)
            );
        }
        println!(
            "P0_RADIO_BOUNDARY profile={profile:?} bytes={n} retained=1 interpreted={}",
            n <= cap
        );
    }
}
#[test]
fn hci_structural_cap_boundaries() {
    assert_eq!(adapters::MAX_HCI_CAPTURE_BYTES, 258);
    let inputs = [257, 258, 259].map(|n| {
        let mut bytes = hci(false, 7);
        bytes.pop();
        bytes.resize(n - 1, 0);
        bytes.push(214);
        bytes[2] = (n - 3) as u8;
        bytes[13] = (n - 15) as u8;
        bytes
    });
    check_profile_cap(Profile::HciSingleReport, 258, inputs);
    println!("P0_HCI_CAP_BOUNDARIES=3/3");
}
#[test]
fn wifi_structural_cap_boundaries() {
    assert_eq!(adapters::MAX_WIFI_CAPTURE_BYTES, 2396);
    check_profile_cap(
        Profile::WifiRadiotapBasic,
        2396,
        [2395, 2396, 2397].map(wifi_at_cap),
    );
    println!("P0_WIFI_CAP_BOUNDARIES=3/3");
}
#[test]
fn wifi_header_and_body_allowances_are_separate() {
    let mut header = wifi_at_cap(2395);
    header.insert(64, 0);
    header[2] = 65;
    let mut body = wifi_at_cap(2396);
    body.drain(17..64);
    body[2] = 17;
    let end = body.len() - 4;
    body[end - 2] = 2;
    body.insert(end, 0);
    for (bytes, expected) in [
        (header, "radiotap header allowance"),
        (body, "management body limit"),
    ] {
        assert!(bytes.len() <= 2396);
        let mut ledger = Ledger::default();
        let mut input = capture_input(bytes.clone(), false);
        input.spans.clear();
        let retained = adapters::capture(&mut ledger, input, Profile::WifiRadiotapBasic).unwrap();
        let Interpretation::Unsupported { reason } = retained.interpretation else {
            panic!("admitted excess {expected}");
        };
        assert!(reason.contains(expected), "{expected}: {reason}");
        assert_eq!(retained.event.bytes, bytes);
        assert_eq!(ledger.events[0].bytes, bytes);
    }
    println!("P0_WIFI_COMPONENT_LIMITS=2/2");
}
