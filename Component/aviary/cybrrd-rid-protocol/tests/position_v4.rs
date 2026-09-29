// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_protocol::astm::parse_message_pack;
use serde_json::{json, Value};

fn pack(lat: i32, lon: i32, status: u8, operator: (i32, i32)) -> Vec<u8> {
    let mut bytes = vec![0u8; 78];
    bytes[..3].copy_from_slice(&[0xF2, 25, 3]);
    bytes[4] = 0x10;
    bytes[5..9].copy_from_slice(b"D27E");
    bytes[28] = 0x12;
    bytes[29] = status << 4 | 0x07; // unrelated low flags must not enter status
    bytes[33..37].copy_from_slice(&lat.to_le_bytes());
    bytes[37..41].copy_from_slice(&lon.to_le_bytes());
    bytes[43..45].copy_from_slice(&2201_u16.to_le_bytes());
    bytes[53] = 0x42;
    bytes[55..59].copy_from_slice(&operator.0.to_le_bytes());
    bytes[59..63].copy_from_slice(&operator.1.to_le_bytes());
    bytes[71..73].copy_from_slice(&2201_u16.to_le_bytes());
    bytes
}

fn wire(bytes: &[u8]) -> Value {
    let data = parse_message_pack(bytes, [1,2,3,4,5,6], -40)
        .expect("present entity must survive an unknown position");
    let value = serde_json::to_value(data).unwrap();
    let full = json!({"node":{"id":"d27e","version":"test","location":{"lat":1,"lon":2,"alt_m":10}},
        "timestamp_utc":1789819200000_u64,"data":value});
    println!("D27_FRAME={full}");
    value
}

#[test]
fn zero_pair_is_absent_independent_of_status_claim() {
    for status in 0..=15 {
        let value = wire(&pack(0, 0, status, (10_000_000, 20_000_000)));
        assert_eq!(value["drone_id"], "D27E");
        assert!(value.get("pos").is_none(), "status {status}: {value}");
        assert_eq!(value["position_unknown_reason"], "zero_pair");
        assert_eq!(value["operational_status"], status);
        assert_eq!(value["operator_pos"]["lat"], 1.0);
    }
}

#[test]
fn observed_status_preserves_reserved_and_nonconforming_values() {
    for status in 0..=15 {
        let value = wire(&pack(10_000_000, 20_000_000, status, (0, 0)));
        assert_eq!(value["operational_status"], status);
        assert!(value.get("position_unknown_reason").is_none());
    }
}

#[test]
fn operator_zero_pair_is_absent_not_null_or_null_island() {
    let value = wire(&pack(10_000_000, 20_000_000, 2, (0, 0)));
    assert!(value.get("operator_pos").is_none());
    assert_eq!(value["operator_position_unknown_reason"], "zero_pair");
    assert_eq!(value["pos"]["alt_m"], 100.5);
}

#[test]
fn missing_location_and_status_do_not_suppress_identity() {
    let bytes = pack(0, 0, 0, (0, 0));
    let mut identity = bytes[..28].to_vec();
    identity[2] = 1;
    let value = wire(&identity);
    assert_eq!(value["drone_id"], "D27E");
    assert!(value.get("pos").is_none());
    assert!(value.get("operational_status").is_none());
    assert_eq!(value["position_unknown_reason"], "no_location");
}

#[test]
fn implausible_coordinates_remain_absent_and_distinguishable() {
    let value = wire(&pack(900_000_001, 20_000_000, 2, (10_000_000, -1_800_000_001)));
    assert!(value.get("pos").is_none());
    assert!(value.get("operator_pos").is_none());
    assert_eq!(value["position_unknown_reason"], "out_of_range");
    assert_eq!(value["operator_position_unknown_reason"], "out_of_range");
}

#[test]
fn single_zero_and_wire_precision_are_preserved() {
    for (lat, lon) in [(0, 1), (1, 0), (-1, 1), (407_608_001, -953_702_001)] {
        let value = wire(&pack(lat, lon, 0, (lat, lon)));
        let expected = json!({"lat":lat as f64 / 1e7, "lon":lon as f64 / 1e7, "alt_m":100.5});
        assert_eq!(value["pos"], expected);
        assert_eq!(value["operator_pos"], expected);
    }
}
