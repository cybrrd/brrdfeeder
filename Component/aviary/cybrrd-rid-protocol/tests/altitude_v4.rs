// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_protocol::astm::parse_message_pack;
use cybrrd_rid_protocol::models::{NormalizedTelemetry, Node, NodeLocation, WIRE_FORMAT_VERSION};
use serde_json::{json, Value};

// Synthetic LOCATION + SYSTEM pack. Absolute raw codes, not the decoder's
// constants: this test must detect sentinel transposition independently.
fn frame(raw: u16) -> Value {
    let mut bytes = [0u8; 53];
    bytes[..3].copy_from_slice(&[0xF1, 25, 2]);
    bytes[3] = 0x10;
    // All four not-yet-published altitude fields are present on the input.
    bytes[16..18].copy_from_slice(&2200_u16.to_le_bytes()); // pressure
    bytes[20..22].copy_from_slice(&2200_u16.to_le_bytes()); // height
    bytes[8..12].copy_from_slice(&10_000_000_i32.to_le_bytes());
    bytes[12..16].copy_from_slice(&20_000_000_i32.to_le_bytes());
    bytes[18..20].copy_from_slice(&raw.to_le_bytes());
    bytes[28] = 0x40;
    bytes[41..43].copy_from_slice(&2200_u16.to_le_bytes()); // ceiling
    bytes[43..45].copy_from_slice(&2200_u16.to_le_bytes()); // floor
    bytes[30..34].copy_from_slice(&10_000_000_i32.to_le_bytes());
    bytes[34..38].copy_from_slice(&20_000_000_i32.to_le_bytes());
    bytes[46..48].copy_from_slice(&raw.to_le_bytes());
    let data = parse_message_pack(&bytes, [1, 2, 3, 4, 5, 6], -40)
        .expect("valid 2D drone fix must be published");
    serde_json::to_value(NormalizedTelemetry {
        wire_format_version: Some(WIRE_FORMAT_VERSION),
        node: Node {
            id: "d27-test".into(), version: "d27".into(),
            location: NodeLocation { lat: 1.0, lon: 2.0, alt_m: 10.0, position_source: None },
        },
        timestamp_utc: 1_789_819_200_000, data,
    }).unwrap()
}

#[test]
fn altitude_wire_unknown_omits_member_without_losing_2d_fix() {
    let value = frame(0);
    for name in ["pressure_altitude", "height", "area_ceiling", "area_floor"] {
        assert!(!value.to_string().contains(name), "deferred field leaked: {name}");
    }
    for point in [&value["data"]["pos"], &value["data"]["operator_pos"]] {
        assert_eq!(point, &json!({"lat": 1.0, "lon": 2.0}));
        assert!(!point.as_object().unwrap().contains_key("alt_m"));
    }
    let decoded: NormalizedTelemetry = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    println!("D27_FRAME={value}");
}

#[test]
fn altitude_wire_maximum_is_numeric() {
    let value = frame(0xFFFF);
    assert_eq!(value["data"]["pos"]["alt_m"], json!(31767.5));
    assert_eq!(value["data"]["operator_pos"]["alt_m"], json!(31767.5));
    println!("D27_FRAME={value}");
}

#[test]
fn altitude_wire_known_zero_is_numeric() {
    let value = frame(2000);
    assert_eq!(value["data"]["pos"]["alt_m"], json!(0.0));
    assert_eq!(value["data"]["operator_pos"]["alt_m"], json!(0.0));
    println!("D27_FRAME={value}");
}

#[test]
fn altitude_wire_version_is_four() { assert_eq!(WIRE_FORMAT_VERSION, 4); }

#[test]
fn d30_version_provenance_is_optional_not_inferred_from_node_build() {
    let mut value = frame(0);
    assert_eq!(value["wire_format_version"], 4);
    assert_eq!(value["node"]["version"], "d27");
    value.as_object_mut().unwrap().remove("wire_format_version");
    let unversioned: NormalizedTelemetry = serde_json::from_value(value.clone()).unwrap();
    assert!(unversioned.wire_format_version.is_none());
    assert_eq!(serde_json::to_value(unversioned).unwrap(), value);
    // A received explicit version is evidence, not automatically overwritten.
    value["wire_format_version"] = json!(3);
    let legacy: NormalizedTelemetry = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(legacy.wire_format_version, Some(3));
    assert_eq!(serde_json::to_value(legacy).unwrap(), value);
}
