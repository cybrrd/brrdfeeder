// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_v5::{decode_message, decoder::message_issues, hex};
use serde_json::json;
fn msg(t: u8) -> [u8; 25] {
    let mut b = [0; 25];
    b[0] = t * 16 + 2;
    b
}
#[test]
#[ignore = "full area-count domain is a dedicated release gate"]
fn semantics_66904_plus_reserved_gate() {
    let mut count = 0;
    let coords = [
        (0, 0),
        (0, 1),
        (1, 0),
        (1, 1),
        (-1, -1),
        (900000000, 1),
        (-900000000, 1),
        (900000001, 1),
        (-900000001, 1),
        (i32::MIN, 1),
        (i32::MAX, 1),
        (1, 1800000000),
        (1, -1800000000),
        (1, 1800000001),
        (1, -1800000001),
        (1, i32::MIN),
        (1, i32::MAX),
        (-1, 1),
    ];
    for (typ, body, latkey, lonkey, lat_at, lon_at) in [
        (1, "location", "latitude", "longitude", 5, 9),
        (4, "system", "operator_latitude", "operator_longitude", 2, 6),
    ] {
        for (lat, lon) in coords {
            let mut b = msg(typ);
            b[lat_at..lat_at + 4].copy_from_slice(&lat.to_le_bytes());
            b[lon_at..lon_at + 4].copy_from_slice(&lon.to_le_bytes());
            let out = decode_message(&b, 0);
            for (key, raw, limit) in [(latkey, lat, 900000000_i64), (lonkey, lon, 1800000000)] {
                assert_eq!(out[body][key]["raw"], raw);
                assert_eq!(
                    out[body][key]["status"],
                    if lat == 0 && lon == 0 {
                        "unknown"
                    } else if i64::from(raw).abs() > limit {
                        "out_of_range"
                    } else {
                        "known"
                    }
                );
            }
            count += 1;
        }
    }
    for source in 0..4 {
        for classif in 0..8 {
            let mut b = msg(4);
            b[1] = classif * 4 + source;
            let out = decode_message(&b, 0);
            assert_eq!(
                out["system"]["operator_location_altitude_source_type"]["raw"],
                source
            );
            assert_eq!(out["system"]["classification_type"]["raw"], classif);
            count += 1;
        }
    }
    for classif in [0, 1] {
        for category in 0..16 {
            for class in 0..16 {
                let mut b = msg(4);
                b[1] = classif * 4;
                b[17] = category * 16 + class;
                let out = decode_message(&b, 0);
                assert_eq!(out["system"]["category"]["raw"], category);
                assert_eq!(out["system"]["class"]["raw"], class);
                let issues = message_issues(&out, "");
                assert_eq!(
                    issues
                        .iter()
                        .any(|v| v["code"] == "classification_inconsistent"),
                    classif != 1 && (category != 0 || class != 0)
                );
                count += 1;
            }
        }
    }
    for raw in 0..=65535_u16 {
        let mut b = msg(4);
        b[10..12].copy_from_slice(&raw.to_le_bytes());
        let out = decode_message(&b, 0);
        assert_eq!(out["system"]["area_count"]["raw"], raw);
        assert_eq!(out["system"]["area_count"]["decoded"], raw);
        assert_eq!(
            out["system"]["area_count"]["status"],
            if raw > 65000 { "out_of_range" } else { "known" }
        );
        count += 1;
    }
    for raw in 0..=255_u8 {
        let mut b = msg(4);
        b[12] = raw;
        let out = decode_message(&b, 0);
        assert_eq!(
            out["system"]["area_radius"]["decoded"],
            (u16::from(raw) * 10).to_string()
        );
        assert_eq!(
            out["system"]["area_radius"]["status"],
            if raw > 250 { "out_of_range" } else { "known" }
        );
        count += 1;
    }
    for (typ, body, key, assigned_max) in [
        (3, "self_id", "description_type", 2),
        (5, "operator_id", "operator_id_type", 0),
    ] {
        for raw in 0..=255_u8 {
            let mut b = msg(typ);
            b[1] = raw;
            let out = decode_message(&b, 0);
            assert_eq!(out[body][key]["raw"], raw);
            assert_eq!(
                out[body][key]["status"],
                if raw <= assigned_max {
                    "known"
                } else if raw <= 200 {
                    "reserved"
                } else {
                    "private_use"
                }
            );
            count += 1;
        }
    }
    for (typ, id, body, key) in [
        (0, 1, "basic_id", "uas_id"),
        (0, 2, "basic_id", "uas_id"),
        (3, 0, "self_id", "description"),
        (5, 0, "operator_id", "operator_id"),
    ] {
        for pattern in [b"A\xffB".as_slice(), b"\0TAIL", b"AB\0CD", b"   ", b" A B "] {
            let mut b = msg(typ);
            b[1] = id * 16;
            b[2..2 + pattern.len()].copy_from_slice(pattern);
            let out = decode_message(&b, 0);
            let width = if typ == 3 { 23 } else { 20 };
            assert_eq!(out[body][key]["raw"], hex(&b[2..2 + width]));
            assert_eq!(
                out[body][key]["status"],
                if pattern == b"   " {
                    "unknown"
                } else if pattern == b" A B " {
                    "known"
                } else {
                    "out_of_range"
                }
            );
            count += 1;
        }
    }
    assert_eq!(count, 66904);
    println!(
        "{}",
        json!({"gate":"other_semantics","cases":count,"status":"passed"})
    );
}
