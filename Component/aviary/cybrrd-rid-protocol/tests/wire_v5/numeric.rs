// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_core::v5::{decode_message, Cell, Value as Atom};
use cybrrd_rid_protocol::v5;
use serde_json::{json, Value};
fn field<'a>(m: &'a cybrrd_rid_core::v5::Message<'a>, name: &str) -> Cell<'a> {
    m.fields.iter().find(|f| f.name == name).unwrap().cell
}
fn assert_cell(
    c: Cell<'_>,
    raw: i64,
    decoded: String,
    status: &str,
    unit: &str,
    res: &str,
    reason: Option<&str>,
    arithmetic: bool,
) {
    let v = v5::cell(c);
    let mut expected = json!({"raw":raw,"decoded":decoded,"status":status,"unit":unit,"resolution":res,"reason":reason});
    if arithmetic {
        expected["arithmetic_value"] = expected["decoded"].clone();
    }
    assert_eq!(v, expected);
}
#[test]
fn exhaustive_altitude_fields_through_independent_byte_offsets() {
    // Oracle uses exactly representable binary halves, not production fixed-point
    // arithmetic. All these integers and halves are exact in binary64.
    let fields = [
        ("pressure_altitude", 13, "m_pressure_29.92_inHg"),
        ("geodetic_altitude_hae", 15, "m_WGS84_HAE"),
        ("height", 17, "m_per_height_type"),
        ("area_ceiling_hae", 13, "m_WGS84_HAE"),
        ("area_floor_hae", 15, "m_WGS84_HAE"),
        ("operator_altitude_hae", 18, "m_WGS84_HAE"),
    ];
    let mut count = 0;
    for (selected, (name, offset, unit)) in fields.iter().enumerate() {
        for raw in 0u32..65536 {
            let mut bytes = [0u8; 25];
            bytes[0] = if selected < 3 { 0x12 } else { 0x42 };
            let range = if selected < 3 { 0..3 } else { 3..6 };
            for i in range.clone() {
                let other = 1111u16 * (i as u16 + 1);
                let o = fields[i].1;
                bytes[o..o + 2].copy_from_slice(&other.to_le_bytes());
            }
            bytes[*offset] = (raw % 256) as u8;
            bytes[*offset + 1] = (raw / 256) as u8;
            let m = decode_message(&bytes).unwrap();
            assert_cell(
                field(&m, name),
                i64::from(raw),
                (f64::from(raw) / 2.0 - 1000.0).to_string(),
                if raw == 0 {
                    "unknown"
                } else if raw == 65535 {
                    "out_of_stated_range"
                } else {
                    "known"
                },
                unit,
                "0.5",
                if raw == 65535 {
                    Some("arithmetic_exceeds_stated_range")
                } else {
                    None
                },
                true,
            );
            for i in range {
                if i != selected {
                    assert_eq!(
                        field(&m, fields[i].0).raw,
                        Atom::Integer(1111 * (i as i64 + 1)),
                        "neighboring altitude changed"
                    );
                }
            }
            count += 1;
        }
    }
    assert_eq!(count, 393216);
    println!("P0_ALTITUDE_FIELD_CODES={count}/393216");
}
#[test]
fn exhaustive_motion_time_and_accuracy_domains() {
    let mut counts = [0usize; 5];
    for b in 0u16..256 {
        let signed = if b >= 128 {
            i64::from(b) - 256
        } else {
            i64::from(b)
        };
        let mut bytes = [0u8; 25];
        bytes[0] = 0x12;
        bytes[4] = b as u8;
        let m = decode_message(&bytes).unwrap();
        assert_cell(
            field(&m, "vertical_speed"),
            signed,
            (signed as f64 / 2.0).to_string(),
            match signed {
                126 => "unknown",
                124 => "saturated_lower",
                -124 => "saturated_upper",
                -123..=123 => "known",
                _ => "out_of_range",
            },
            "m/s",
            "0.5",
            None,
            false,
        );
        counts[0] += 1;
        drop(m);
        for flag in [0u8, 1] {
            bytes[1] = flag;
            bytes[3] = b as u8;
            let m = decode_message(&bytes).unwrap();
            let expected = if flag == 0 {
                f64::from(b) / 4.0
            } else {
                f64::from(b) * 0.75 + 63.75
            };
            assert_cell(
                field(&m, "speed"),
                i64::from(b),
                expected.to_string(),
                if flag == 1 && b == 255 {
                    "unknown"
                } else if flag == 1 && b == 254 {
                    "saturated_lower"
                } else {
                    "known"
                },
                "m/s",
                "0.25 or 0.75 by speed_multiplier",
                None,
                false,
            );
            counts[1] += 1;
            drop(m);
            bytes[1] = flag * 2;
            bytes[2] = b as u8;
            let m = decode_message(&bytes).unwrap();
            let expected = i64::from(b) + 180 * i64::from(flag);
            assert_cell(
                field(&m, "track_direction"),
                i64::from(b),
                expected.to_string(),
                if expected == 361 {
                    "unknown"
                } else if expected >= 360 {
                    "out_of_range"
                } else {
                    "known"
                },
                "degree_true",
                "1",
                None,
                false,
            );
            counts[2] += 1;
        }
    }
    for raw in 0u32..65536 {
        let mut b = [0u8; 25];
        b[0] = 0x12;
        b[21] = (raw % 256) as u8;
        b[22] = (raw / 256) as u8;
        let m = decode_message(&b).unwrap();
        let text = if raw % 10 == 0 {
            (raw / 10).to_string()
        } else {
            format!("{}.{}", raw / 10, raw % 10)
        };
        assert_cell(
            field(&m, "timestamp"),
            i64::from(raw),
            text,
            if raw == 65535 {
                "unknown"
            } else if raw > 36000 {
                "out_of_range"
            } else {
                "known"
            },
            "s_since_UTC_hour",
            "0.1",
            None,
            false,
        );
        counts[3] += 1;
    }
    for code in 0u8..16 {
        let mut b = [0u8; 25];
        b[0] = 0x12;
        b[19] = code * 17;
        b[20] = code * 17;
        b[23] = code;
        let m = decode_message(&b).unwrap();
        for (name, max) in [
            ("vertical_accuracy", 6),
            ("horizontal_accuracy", 12),
            ("baro_altitude_accuracy", 6),
            ("speed_accuracy", 4),
        ] {
            let c = field(&m, name);
            assert_eq!(c.raw, Atom::Integer(code.into()));
            assert_eq!(c.decoded, Atom::Integer(code.into()));
            assert_eq!(
                c.status,
                if code == 0 {
                    "unknown"
                } else if code > max {
                    "reserved"
                } else {
                    "known"
                }
            );
            assert_eq!(c.unit, "95_percent_bound_code");
            counts[4] += 1;
        }
        assert_cell(
            field(&m, "timestamp_accuracy"),
            code.into(),
            if code % 10 == 0 {
                (code / 10).to_string()
            } else {
                format!("{}.{}", code / 10, code % 10)
            },
            if code == 0 { "unknown" } else { "known" },
            "s",
            "0.1",
            None,
            false,
        );
        counts[4] += 1;
    }
    assert_eq!(counts, [256, 512, 512, 65536, 80]);
    println!("P0_NUMERIC_DOMAINS={counts:?}");
}
#[test]
fn u32_epoch_boundaries_and_each_byte_position() {
    let mut values = vec![0, 1, u32::MAX, u32::MAX - 1, 2_147_483_647, 2_147_483_648];
    for byte in 0..4 {
        for n in 0u32..256 {
            values.push(n << (byte * 8));
        }
    }
    let mut count = 0;
    for raw in values {
        for (kind, offset, body) in [(0x22, 4, "auth"), (0x42, 20, "system")] {
            let mut b = [0u8; 25];
            b[0] = kind;
            // Independent byte extraction from radix arithmetic.
            for n in 0..4 {
                b[offset + n] = ((u64::from(raw) / 256u64.pow(n as u32)) % 256) as u8;
            }
            let m = v5::decode_message(&b, 0).unwrap();
            let c = &m[body]["timestamp"];
            assert_eq!(c["raw"], raw);
            assert_eq!(c["decoded"], raw.to_string());
            assert_eq!(c["status"], "known");
            assert_eq!(
                cybrrd_rid_core::v5::unix_seconds(raw) as u128,
                u128::from(raw) + 1_546_300_800u128
            );
            count += 1;
        }
    }
    assert_eq!(count, 2060);
    println!("P0_U32_EXECUTABLE_CASES={count}/2060; FULL_DOMAIN_PROOF=external SMT receipt");
}
#[test]
fn all_enum_bits_and_coordinate_edges() {
    let mut count = 0;
    for raw in 0u16..256 {
        for kind in [0x02, 0x12, 0x22, 0x32, 0x42, 0x52] {
            let mut b = [0u8; 25];
            b[0] = kind;
            b[1] = raw as u8;
            let m = v5::decode_message(&b, 0).unwrap();
            model_check(&m);
            count += 1;
            if kind == 0x02 {
                assert_eq!(m["basic_id"]["id_type"]["raw"], raw / 16);
                assert_eq!(m["basic_id"]["ua_type"]["raw"], raw % 16);
            }
            if kind == 0x42 {
                assert_eq!(
                    m["system"]["operator_location_altitude_source_type"]["raw"],
                    raw % 4
                );
                assert_eq!(m["system"]["classification_type"]["raw"], (raw / 4) % 8);
                assert_eq!(m["system"]["flags_reserved"]["raw"], raw / 32);
            }
            if kind == 0x12 {
                assert_eq!(m["location"]["height_type"]["raw"], (raw / 4) % 2);
            }
        }
    }
    for (lat, lon, status_lat, status_lon) in [
        (0i32, 0i32, "unknown", "unknown"),
        (0, 1, "known", "known"),
        (1, 0, "known", "known"),
        (900000000, -1800000000, "known", "known"),
        (900000001, -1800000001, "out_of_range", "out_of_range"),
        (i32::MIN, i32::MAX, "out_of_range", "out_of_range"),
    ] {
        for (kind, lo, la, prefix) in [(0x12, 9, 5, ""), (0x42, 6, 2, "operator_")] {
            let mut b = [0u8; 25];
            b[0] = kind;
            b[la..la + 4].copy_from_slice(&lat.to_le_bytes());
            b[lo..lo + 4].copy_from_slice(&lon.to_le_bytes());
            let m = decode_message(&b).unwrap();
            assert_eq!(field(&m, &format!("{prefix}latitude")).status, status_lat);
            assert_eq!(field(&m, &format!("{prefix}longitude")).status, status_lon);
            count += 1;
        }
    }
    assert_eq!(count, 1548);
    println!("P0_ENUM_COORDINATE_CASES={count}/1548");
}
fn model_check(m: &Value) {
    cybrrd_rid_protocol::v5::model::validate_message(m).unwrap();
}

#[test]
fn asymmetric_nibbles_and_every_message_bit_are_exposed() {
    let mut nibbles = 0;
    for raw in 0u16..256 {
        let mut b = [0u8; 25];
        b[0] = 0x12;
        b[1] = raw as u8;
        b[19] = raw as u8;
        b[20] = raw as u8;
        b[23] = raw as u8;
        let m = v5::decode_message(&b, 0).unwrap();
        for (name, want) in [
            ("operational_status", raw / 16),
            ("status_reserved", (raw / 8) % 2),
            ("height_type", (raw / 4) % 2),
            ("ew_direction_segment", (raw / 2) % 2),
            ("speed_multiplier", raw % 2),
            ("vertical_accuracy", raw / 16),
            ("horizontal_accuracy", raw % 16),
            ("baro_altitude_accuracy", raw / 16),
            ("speed_accuracy", raw % 16),
            ("timestamp_accuracy", raw % 16),
            ("timestamp_reserved", raw / 16),
        ] {
            assert_eq!(m["location"][name]["raw"], want);
            nibbles += 1;
        }
        b[0] = 0x42;
        b[17] = raw as u8;
        let m = v5::decode_message(&b, 0).unwrap();
        assert_eq!(m["system"]["category"]["raw"], raw / 16);
        assert_eq!(m["system"]["class"]["raw"], raw % 16);
        nibbles += 2;
        b[0] = 0x22;
        b[1] = 0x10;
        b[2] = raw as u8;
        let m = v5::decode_message(&b, 0).unwrap();
        assert_eq!(m["auth"]["page0_reserved"]["raw"], raw / 16);
        assert_eq!(m["auth"]["last_page"]["raw"], raw % 16);
        nibbles += 2;
    }
    fn raws(v: &Value, out: &mut Vec<Value>) {
        match v {
            Value::Object(m) => {
                if let Some(r) = m.get("raw") {
                    out.push(r.clone());
                }
                for v in m.values() {
                    raws(v, out);
                }
            }
            Value::Array(a) => {
                for v in a {
                    raws(v, out);
                }
            }
            _ => {}
        }
    }
    let mut bits = 0;
    for (kind, page) in [(0, 0), (1, 0), (2, 0), (2, 1), (3, 0), (4, 0), (5, 0)] {
        let mut b = [0u8; 25];
        b[0] = kind * 16 + 2;
        b[1] = page;
        let good = v5::decode_message(&b, 0).unwrap();
        let mut original = Vec::new();
        raws(&good, &mut original);
        for bit in 0..200 {
            let mut altered = b;
            altered[bit / 8] ^= 1 << (bit % 8);
            let decoded = v5::decode_message(&altered, 0).unwrap();
            let mut actual = Vec::new();
            raws(&decoded, &mut actual);
            assert_ne!(
                actual, original,
                "unexposed message bit {kind}/{page}/{bit}"
            );
            bits += 1;
        }
    }
    assert_eq!(nibbles, 3840);
    assert_eq!(bits, 1400);
    println!("P0_ASYMMETRIC_NIBBLE_CHECKS={nibbles}/3840; NAMED_RAW_BIT_SENSITIVITY={bits}/1400");
}
