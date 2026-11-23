// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_v5::{decode_message, hex, numeric, unhex};
use serde_json::json;
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "dedicated release gate; default suite uses deterministic field-binding subset"]
fn all_numeric_domains_except_u32_460112() {
    let began = Instant::now();
    let mut count = 0_u64;
    for code in 0..256 {
        let raw = code as u8 as i8;
        let n = numeric::vertical(raw);
        assert_eq!(n.scaled, i64::from(raw) * 5);
        assert_eq!(
            n.status,
            if raw == 126 {
                "unknown"
            } else if raw == 124 {
                "saturated_lower"
            } else if raw == -124 {
                "saturated_upper"
            } else if (-123..=123).contains(&raw) {
                "known"
            } else {
                "out_of_range"
            }
        );
        count += 1;
    }
    for branch in [false, true] {
        for code in 0..256 {
            let raw = code as u8;
            let n = numeric::speed(raw, branch);
            let hundredths = if branch { 75 * (code + 85) } else { 25 * code };
            assert_eq!(n.scaled, hundredths);
            assert_eq!(
                n.status,
                if hundredths == 25500 {
                    "unknown"
                } else if hundredths == 25425 {
                    "saturated_lower"
                } else {
                    "known"
                }
            );
            count += 1;
            let n = numeric::direction(raw, branch);
            let degrees = code + 180 * i64::from(branch);
            assert_eq!(n.scaled, degrees);
            assert_eq!(
                n.status,
                if degrees == 361 {
                    "unknown"
                } else if degrees < 360 {
                    "known"
                } else {
                    "out_of_range"
                }
            );
            count += 1;
        }
    }
    for raw in 0..=u16::MAX {
        for (typ, body, fields) in [
            (
                1,
                "location",
                [
                    ("pressure_altitude", 13),
                    ("geodetic_altitude_hae", 15),
                    ("height", 17),
                ],
            ),
            (
                4,
                "system",
                [
                    ("area_ceiling_hae", 13),
                    ("area_floor_hae", 15),
                    ("operator_altitude_hae", 18),
                ],
            ),
        ] {
            let mut b = [0_u8; 25];
            b[0] = (typ << 4) | 2;
            for (_, offset) in fields {
                b[offset..offset + 2].copy_from_slice(&raw.to_le_bytes());
            }
            let m = decode_message(&b, 0);
            let half_units = i64::from(raw) - 2000;
            let expected = if half_units % 2 == 0 {
                (half_units / 2).to_string()
            } else {
                format!(
                    "{}{}.5",
                    if half_units < 0 { "-" } else { "" },
                    half_units.abs() / 2
                )
            };
            for (field, _) in fields {
                let c = &m[body][field];
                assert_eq!(c["raw"], raw);
                assert_eq!(c["decoded"], expected);
                assert_eq!(c["arithmetic_value"], expected);
                assert_eq!(
                    c["status"],
                    if raw == 0 {
                        "unknown"
                    } else if raw == 65535 {
                        "out_of_stated_range"
                    } else {
                        "known"
                    }
                );
                if raw == 65535 {
                    assert_eq!(c["reason"], "arithmetic_exceeds_stated_range");
                }
                count += 1;
            }
        }
        let n = numeric::hour_timestamp(raw);
        assert_eq!(n.scaled, i64::from(raw));
        assert_eq!(n.places, 1);
        assert_eq!(
            n.status,
            if raw == 65535 {
                "unknown"
            } else if raw > 36000 {
                "out_of_range"
            } else {
                "known"
            }
        );
        count += 1;
    }
    for code in 0..16 {
        for max in [6, 12, 6, 4] {
            assert_eq!(
                numeric::accuracy_status(code, max),
                if code == 0 {
                    "unknown"
                } else if code > max {
                    "reserved"
                } else {
                    "known"
                }
            );
            count += 1;
        }
        let n = numeric::timestamp_accuracy(code);
        assert_eq!(n.scaled, i64::from(code));
        assert_eq!(n.places, 1);
        assert_eq!(n.status, if code == 0 { "unknown" } else { "known" });
        count += 1;
    }
    assert_eq!(count, 460112);
    println!(
        "{}",
        json!({"gate":"numeric_non_u32","cases":count,"elapsed_ms":began.elapsed().as_millis(),"status":"passed"})
    );
}

#[test]
#[ignore = "dedicated release gate: all 24-bit reserved values"]
fn all_reserved_patterns_33554986() {
    let start = Instant::now();
    let mut count = 0_u64;
    for typ in [0_u8, 5] {
        for value in 0_u32..1 << 24 {
            let bytes = black_box(value.to_le_bytes());
            let slice = &bytes[..3];
            // These are the exact unmodified byte spans fed to the common
            // reserved-cell representation. Boundary binding is checked below.
            let actual = hex(slice);
            let expected = format!(
                "{:02x}{:02x}{:02x}",
                value & 255,
                (value >> 8) & 255,
                (value >> 16) & 255
            );
            assert_eq!(actual, expected);
            assert_eq!(unhex(&actual).unwrap(), slice);
            count += 1;
            if value < 256 || value % 65537 == 0 || value == 0xffffff {
                let mut m = [0_u8; 25];
                m[0] = typ * 16 + 2;
                m[22..25].copy_from_slice(slice);
                let out = decode_message(&m, 0);
                let b = if typ == 0 { "basic_id" } else { "operator_id" };
                assert_eq!(out[b]["reserved"]["raw"], actual);
                assert_eq!(out[b]["reserved"]["decoded"], actual);
                assert_eq!(out[b]["reserved"]["status"], "reserved");
            }
        }
    }
    for (typ, body, field, offset, shift, max) in [
        (1, "location", "reserved", 24, 0, 256),
        (4, "system", "reserved", 24, 0, 256),
        (1, "location", "status_reserved", 1, 3, 2),
        (1, "location", "timestamp_reserved", 23, 4, 16),
        (2, "auth", "page0_reserved", 2, 4, 16),
        (4, "system", "flags_reserved", 1, 5, 8),
    ] {
        for value in 0..max {
            let mut b = [0_u8; 25];
            b[0] = typ * 16 + 2;
            b[offset] = (value as u8) << shift;
            let out = decode_message(&b, 0);
            let expected = if field == "reserved" {
                json!(format!("{value:02x}"))
            } else {
                json!(value)
            };
            assert_eq!(out[body][field]["raw"], expected);
            assert_eq!(out[body][field]["decoded"], expected);
            count += 1;
        }
    }
    assert_eq!(count, 33554986);
    println!(
        "{}",
        json!({"gate":"reserved_patterns","cases":count,"elapsed_ms":start.elapsed().as_millis(),"status":"passed"})
    );
}
