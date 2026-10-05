// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_protocol::{models::RidTransport, router::ingest_frame};

const SOURCE: [u8; 6] = [0x02, 1, 2, 3, 4, 5];
const CLUSTER: [u8; 6] = [0x50, 0x6f, 0x9a, 1, 0, 0xff];
const SERVICE: [u8; 6] = [0x88, 0x69, 0x19, 0x9d, 0x92, 0x09];

fn pack(count: u8) -> Vec<u8> {
    let mut bytes = vec![0xf2, 25, count];
    let mut basic = [0; 25];
    basic[0] = 2;
    basic[1] = 0x12;
    basic[2..13].copy_from_slice(b"NAN-TEST-01");
    bytes.extend_from_slice(&basic);
    for _ in 1..count {
        // Valid LOCATION messages also exercise identity-independent count limits.
        let mut location = [0; 25];
        location[0] = 0x12;
        location[5..9].copy_from_slice(&407_608_000_i32.to_le_bytes());
        location[9..13].copy_from_slice(&(-953_702_000_i32).to_le_bytes());
        bytes.extend_from_slice(&location);
    }
    bytes
}

fn attr(id: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![id];
    out.extend_from_slice(&(body.len() as u16).to_le_bytes());
    out.extend_from_slice(body);
    out
}

fn descriptor(counter: u8, pack: &[u8]) -> Vec<u8> {
    let mut body = SERVICE.to_vec();
    body.extend_from_slice(&[1, 0, 0x10, (1 + pack.len()) as u8, counter]);
    body.extend_from_slice(pack);
    attr(3, &body)
}

fn action(attributes: &[u8]) -> Vec<u8> {
    let mut frame = vec![0; 24];
    frame[0] = 0xd0;
    frame[4..10].fill(0xff);
    frame[10..16].copy_from_slice(&SOURCE);
    frame[16..22].copy_from_slice(&CLUSTER);
    frame.extend_from_slice(&[4, 9, 0x50, 0x6f, 0x9a, 0x13]);
    frame.extend_from_slice(attributes);
    frame
}

fn sdf(counter: u8, count: u8) -> Vec<u8> {
    action(&descriptor(counter, &pack(count)))
}

#[test]
fn nan_sdf_sets_wire_metadata() {
    let result = ingest_frame(&sdf(0xf7, 2), -49);
    assert!(
        result.is_some(),
        "valid ODID NAN SDF was dropped at admission"
    );
    let data = result.unwrap();
    assert_eq!(data.transport, Some(RidTransport::WifiNan));
    assert_eq!(data.message_counter, Some(0xf7));
    assert_eq!(data.mac_address, SOURCE);
    assert_eq!(data.drone_id, "NAN-TEST-01");
    let wire = serde_json::to_value(data).unwrap();
    assert_eq!(wire["transport"], "wifi_nan");
    assert_eq!(wire["message_counter"], 0xf7);
    assert_eq!(wire["mac_address"], "02:01:02:03:04:05");
    assert_eq!(wire["wifi_bssid"], "50:6f:9a:01:00:ff");
}

#[test]
fn nan_all_counters_and_pack_counts() {
    let mut rejected = Vec::new();
    for count in 1..=9 {
        for counter in 0..=255 {
            match ingest_frame(&sdf(counter, count), -49) {
                Some(data)
                    if data.transport == Some(RidTransport::WifiNan)
                        && data.message_counter == Some(counter) => {}
                _ => rejected.push((count, counter)),
            }
        }
    }
    assert!(
        rejected.is_empty(),
        "NAN (count, counter) failures: {rejected:?}"
    );
}

#[test]
fn nan_skips_unknown_attributes() {
    let mut attributes = attr(0xee, &[1, 2, 3]);
    let mut unrelated = descriptor(1, &pack(1));
    unrelated[3] ^= 1;
    attributes.extend(unrelated);
    attributes.extend(descriptor(0x24, &pack(2)));
    attributes.extend(attr(0x0e, &[1, 0, 2, 0x24]));
    assert_eq!(
        ingest_frame(&action(&attributes), -49)
            .unwrap()
            .message_counter,
        Some(0x24)
    );
}

#[test]
fn nan_rejects_wrong_service() {
    let mut frame = sdf(0x24, 2);
    for offset in 33..39 {
        frame[offset] ^= 1;
        assert!(
            ingest_frame(&frame, -49).is_none(),
            "service-ID byte {offset} ignored"
        );
        frame[offset] ^= 1;
    }
}

#[test]
fn nan_rejects_other_actions() {
    let good = sdf(0x24, 2);
    for offset in [0, 24, 25, 26, 27, 28, 29, 41] {
        let mut bad = good.clone();
        bad[offset] ^= 1;
        assert!(
            ingest_frame(&bad, -49).is_none(),
            "admission byte {offset} ignored"
        );
    }
    // Protected frames and fragments cannot be interpreted as a complete SDF.
    for flags in [0x40, 0x04] {
        let mut bad = good.clone();
        bad[1] = flags;
        assert!(ingest_frame(&bad, -49).is_none());
    }
    let mut fragment = good;
    fragment[22] = 1;
    assert!(ingest_frame(&fragment, -49).is_none());
}

#[test]
fn nan_rejects_sync_beacon() {
    let mut frame = vec![0; 36];
    frame[0] = 0x80;
    frame[10..16].copy_from_slice(&SOURCE);
    // A NAN OUI is not the Beacon ODID OUI, even if payload looks like RID.
    let mut body = vec![0x50, 0x6f, 0x9a, 0x13, 7];
    body.extend(pack(2));
    frame.extend_from_slice(&[0xdd, body.len() as u8]);
    frame.extend(body);
    assert!(ingest_frame(&frame, -49).is_none());
    frame[38..42].copy_from_slice(&[0x2a, 0x1a, 5, 0x13]);
    assert!(
        ingest_frame(&frame, -49).is_none(),
        "unverified legacy OUI accepted"
    );
}

#[test]
fn nan_rejects_bad_lengths() {
    let good = sdf(0x24, 2);
    for (offset, value) in [(31, 0), (31, 255), (32, 255), (42, 0), (42, 255)] {
        let mut bad = good.clone();
        bad[offset] = value;
        assert!(
            ingest_frame(&bad, -49).is_none(),
            "bad length at {offset} accepted"
        );
    }
    for trailer in [&[0xee][..], &[0xee, 2][..], &[0xee, 2, 0, 7][..]] {
        let mut bad = good.clone();
        bad.extend_from_slice(trailer);
        assert!(
            ingest_frame(&bad, -49).is_none(),
            "truncated trailing attribute accepted"
        );
    }
    let mut overlong = pack(9);
    overlong.extend([0; 27]); // 256-byte service info; cannot fit its one-byte length.
    assert!(ingest_frame(&action(&descriptor(1, &overlong)), -49).is_none());
}

#[test]
fn nan_rejects_bad_pack() {
    for (offset, value) in [(0, 0x12), (1, 24), (2, 0), (2, 10)] {
        let mut bad = pack(2);
        bad[offset] = value;
        assert!(ingest_frame(&action(&descriptor(1, &bad)), -49).is_none());
    }
    let mut bad = pack(2);
    bad.push(0);
    assert!(ingest_frame(&action(&descriptor(1, &bad)), -49).is_none());
}

#[test]
fn nan_truncation_and_byte_mutations_do_not_panic() {
    let frame = sdf(0xff, 9);
    for len in 0..frame.len() {
        assert!(
            ingest_frame(&frame[..len], -127).is_none(),
            "prefix {len} accepted"
        );
    }
    for offset in 0..frame.len() {
        for value in 0..=255 {
            let mut bytes = frame.clone();
            bytes[offset] = value;
            let _ = ingest_frame(&bytes, -127);
        }
    }
}

#[test]
fn nan_independent_c_oracle_fields_match() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/nan-oracle.json")).unwrap();
    assert_eq!(
        fixture["upstream_commit"],
        "6484f26545d4f012682524e2d843fab0fbdc0b34"
    );
    let unhex = |value: &serde_json::Value| -> Vec<u8> {
        value
            .as_str()
            .unwrap()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    };
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 36);
    for case in cases {
        let frame = unhex(&case["frame_hex"]);
        let data = ingest_frame(&frame, -49).expect("C-generated NAN frame must decode");
        let wire = serde_json::to_value(data).unwrap();
        assert_eq!(wire["message_counter"], case["counter"]);
        assert_eq!(wire["transport"], "wifi_nan");
        assert_eq!(wire["mac_address"], "02:01:02:03:04:05");
        assert_eq!(wire["wifi_bssid"], "50:6f:9a:01:00:ff");
        for (field, expected) in case["decoded"].as_object().unwrap() {
            assert_eq!(
                &wire[field], expected,
                "C/Rust field {field}, count {}",
                case["count"]
            );
        }
        assert_eq!(wire["drone_id"], case["decoded"]["hardware_serial"]);
    }
    assert!(ingest_frame(&unhex(&fixture["sync_frame_hex"]), -49).is_none());
}

#[test]
fn nan_negative_controls_have_valid_pairs() {
    // These assertions keep rejection tests honest: repairing the changed byte
    // must recover real telemetry rather than just another rejected input.
    let good = sdf(0x24, 2);
    assert!(ingest_frame(&good, -49).is_some());
    for offset in [24, 25, 26, 27, 28, 29, 33, 34, 35, 36, 37, 38, 41, 42] {
        let mut changed = good.clone();
        changed[offset] ^= 1;
        assert!(ingest_frame(&changed, -49).is_none());
        changed[offset] ^= 1;
        assert!(ingest_frame(&changed, -49).is_some());
    }
    for offset in [31, 32, 44, 45, 46] {
        let mut changed = good.clone();
        changed[offset] = 0;
        if good[offset] == 0 {
            changed[offset] = 255;
        }
        assert!(ingest_frame(&changed, -49).is_none());
        changed[offset] = good[offset];
        assert!(ingest_frame(&changed, -49).is_some());
    }
    let mut duplicate = descriptor(1, &pack(1));
    duplicate.extend(descriptor(2, &pack(1)));
    assert!(ingest_frame(&action(&duplicate), -49).is_none());
}
