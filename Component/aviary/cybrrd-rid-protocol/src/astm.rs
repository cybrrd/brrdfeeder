// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! ASTM F3411 wire adapter — the std/serde layer over `cybrrd-rid-core`.
//!
//! The byte-level decode now lives in the `no_std` `cybrrd-rid-core` crate
//! (#168 Option B), so the exact same parser runs on the ESP32-S3 Sentinel.
//! This module is the thin std wrapper: it calls the core decoder and maps
//! the heapless [`RidPack`] onto the serde JSON [`TelemetryData`] wire type,
//! adding the receiver-side metadata (transmitter MAC, RSSI) the core, which
//! only sees over-the-air bytes, never has.

use crate::models::{AuthInfo, GeoPoint, ProtocolType, SelfIdInfo, TelemetryData};
use cybrrd_rid_core::{
    decode_message_pack, decode_message_pack_strict, GeoPoint as CoreGeoPoint, RidPack,
};

/// Parse a raw ODID Message Pack payload into the JSON wire [`TelemetryData`].
///
/// Returns `None` when the core decoder rejects the pack (structurally
/// invalid, or neither LOCATION nor usable identity). Wire v4 retains an
/// observed entity even without a position; downstream gate readiness is owed.
pub fn parse_message_pack(
    payload: &[u8],
    mac_address: [u8; 6],
    rssi_dbm: i32,
) -> Option<TelemetryData> {
    let pack: RidPack = decode_message_pack(payload).ok()?;
    telemetry_from_pack(pack, mac_address, rssi_dbm)
}

/// Parse a Message Pack whose transport framing has already been removed.
///
/// Unlike [`parse_message_pack`], this requires the Pack header at byte zero,
/// count 1..=9, and an exact declared-length match.
pub fn parse_message_pack_strict(
    payload: &[u8],
    mac_address: [u8; 6],
    rssi_dbm: i32,
) -> Option<TelemetryData> {
    let pack: RidPack = decode_message_pack_strict(payload).ok()?;
    telemetry_from_pack(pack, mac_address, rssi_dbm)
}

pub(crate) fn telemetry_from_pack(
    pack: RidPack,
    mac_address: [u8; 6],
    rssi_dbm: i32,
) -> Option<TelemetryData> {
    if !pack.has_observation() { return None; }

    let hardware_serial = pack.hardware_serial.as_ref().map(|s| s.as_str().to_owned());
    let caa_registration = pack.caa_registration.as_ref().map(|s| s.as_str().to_owned());

    // Back-compat primary id: hardware serial, else CAA registration, else UNKNOWN.
    let drone_id = hardware_serial
        .clone()
        .or_else(|| caa_registration.clone())
        .unwrap_or_else(|| "UNKNOWN".to_owned());

    Some(TelemetryData {
        transport: None,
        message_counter: None,
        protocol: ProtocolType::AstmF3411_22a,
        mac_address,
        drone_id,
        hardware_serial,
        caa_registration,
        operational_status: pack.operational_status,
        position_unknown_reason: pack.position_unknown_reason.map(Into::into).or_else(|| {
            if pack.operational_status.is_none() && pack.drone_pos.is_none() {
                Some(crate::models::PositionUnknownReason::NoLocation)
            } else { None }
        }),
        operator_position_unknown_reason: pack.operator_position_unknown_reason.map(Into::into),
        pos: pack.drone_pos.map(conv_geo),
        operator_pos: pack.operator_pos.map(conv_geo),
        operator_id: pack.operator_id.as_ref().map(|s| s.as_str().to_owned()),
        self_id: pack.self_id.as_ref().map(|s| SelfIdInfo {
            desc_type: s.desc_type,
            description: s.description.as_str().to_owned(),
        }),
        auth: pack.auth.map(|a| AuthInfo {
            auth_type: a.auth_type,
            timestamp_utc: a.timestamp_utc,
            total_length: a.total_length,
            last_page_index: a.last_page_index,
        }),
        signal_rssi_dbm: rssi_dbm,
    })
}

fn conv_geo(g: CoreGeoPoint) -> GeoPoint {
    GeoPoint {
        lat: g.lat,
        lon: g.lon,
        alt_m: g.alt_m,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUB: usize = 25;

    /// 3-byte ASTM Pack header: 0xF1 (PV=1, MT=F), 0x19 (size=25), count.
    fn build_pack_header(count: u8) -> Vec<u8> {
        vec![0xF1, SUB as u8, count]
    }

    fn put_location(buf: &mut [u8], off: usize, lat_e7: i32, lon_e7: i32, alt_raw: u16) {
        buf[off] = 0x10;
        buf[off + 5..off + 9].copy_from_slice(&lat_e7.to_le_bytes());
        buf[off + 9..off + 13].copy_from_slice(&lon_e7.to_le_bytes());
        buf[off + 15..off + 17].copy_from_slice(&alt_raw.to_le_bytes());
    }

    fn synthesize_pack(serial: &[u8], lat_e7: i32, lon_e7: i32, alt_raw: u16) -> Vec<u8> {
        let mut buf = build_pack_header(2);
        buf.resize(3 + 2 * SUB, 0);
        // BASIC_ID (IDType nibble 0 — untyped, falls back to hardware_serial)
        buf[3] = 0x00;
        let n = serial.len().min(20);
        buf[5..5 + n].copy_from_slice(&serial[..n]);
        put_location(&mut buf, 3 + SUB, lat_e7, lon_e7, alt_raw);
        buf
    }

    #[test]
    fn parses_basic_id_and_location_to_wire() {
        let buf = synthesize_pack(b"CYBRRD-VALID-RUST-01", 407_608_000, -953_702_000, 2200);
        let mac = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        let r = parse_message_pack(&buf, mac, -30).expect("should parse");
        assert_eq!(r.drone_id, "CYBRRD-VALID-RUST-01");
        assert_eq!(r.hardware_serial.as_deref(), Some("CYBRRD-VALID-RUST-01"));
        assert_eq!(r.caa_registration, None);
        assert_eq!(r.mac_address, mac);
        assert!((r.pos.as_ref().unwrap().lat - 40.7608).abs() < 1e-6);
        assert!((r.pos.as_ref().unwrap().lon - (-95.3702)).abs() < 1e-6);
        assert_eq!(r.signal_rssi_dbm, -30);
    }

    #[test]
    fn dual_basic_id_splits_serial_and_registration_on_wire() {
        // BASIC_ID(serial, type1) + BASIC_ID(registration, type2) + LOCATION
        let mut buf = build_pack_header(3);
        buf.resize(3 + 3 * SUB, 0);
        buf[3] = 0x00;
        buf[4] = 1 << 4; // IDType 1 = Serial
        buf[5..5 + 20].copy_from_slice(b"1581FBEXC25AM00D88VJ");
        let b2 = 3 + SUB;
        buf[b2] = 0x00;
        buf[b2 + 1] = 2 << 4; // IDType 2 = CAA Registration
        buf[b2 + 2..b2 + 2 + 10].copy_from_slice(b"FA3GPQK7L9");
        put_location(&mut buf, 3 + 2 * SUB, 407_608_000, -953_702_000, 2200);

        let r = parse_message_pack(&buf, [0; 6], -40).expect("should parse");
        assert_eq!(r.hardware_serial.as_deref(), Some("1581FBEXC25AM00D88VJ"));
        assert_eq!(r.caa_registration.as_deref(), Some("FA3GPQK7L9"));
        // drone_id back-compat resolves to the serial.
        assert_eq!(r.drone_id, "1581FBEXC25AM00D88VJ");
    }

    #[test]
    fn registration_only_resolves_drone_id_to_registration() {
        let mut buf = build_pack_header(2);
        buf.resize(3 + 2 * SUB, 0);
        buf[3] = 0x00;
        buf[4] = 2 << 4; // IDType 2 = CAA Registration only
        buf[5..5 + 10].copy_from_slice(b"FA-REG-001");
        put_location(&mut buf, 3 + SUB, 407_608_000, -953_702_000, 2200);
        let r = parse_message_pack(&buf, [0; 6], -40).unwrap();
        assert_eq!(r.hardware_serial, None);
        assert_eq!(r.caa_registration.as_deref(), Some("FA-REG-001"));
        assert_eq!(r.drone_id, "FA-REG-001");
    }

    #[test]
    fn identity_only_pack_has_unknown_position() {
        let mut buf = build_pack_header(1);
        buf.resize(3 + SUB, 0);
        buf[3] = 0x00;
        buf[5..25].copy_from_slice(b"CYBRRD-NO-LOCATION-A");
        let data = parse_message_pack(&buf, [0; 6], -50).unwrap();
        assert!(data.pos.is_none());
        assert_eq!(data.drone_id, "CYBRRD-NO-LOCATION-A");
    }

    #[test]
    fn rejects_non_pack_header() {
        let mut buf = vec![0u8; 30];
        buf[0] = 0x10;
        assert!(parse_message_pack(&buf, [1, 2, 3, 4, 5, 6], -50).is_none());
    }

    #[test]
    fn parses_operator_position_from_system_message() {
        let serial = b"CYBRRD-WITH-OPERATOR";
        let mut buf = build_pack_header(3);
        buf.resize(3 + 3 * SUB, 0);
        buf[3] = 0x00;
        buf[5..5 + serial.len()].copy_from_slice(serial);
        put_location(&mut buf, 3 + SUB, 407_608_000, -953_702_000, 2200);
        let sysm = 3 + 2 * SUB;
        buf[sysm] = 0x40;
        buf[sysm + 2..sysm + 6].copy_from_slice(&407_600_000_i32.to_le_bytes());
        buf[sysm + 6..sysm + 10].copy_from_slice(&(-953_700_000_i32).to_le_bytes());
        buf[sysm + 18..sysm + 20].copy_from_slice(&2050_u16.to_le_bytes());
        let r = parse_message_pack(&buf, [0; 6], -40).expect("should parse");
        let op = r.operator_pos.expect("operator pos must be set");
        assert!((op.lat - 40.7600).abs() < 1e-6);
        assert!((op.lon - (-95.3700)).abs() < 1e-6);
        assert!((op.alt_m.unwrap() - 25.0).abs() < 1e-3);
    }

    #[test]
    fn parses_operator_id_and_self_id() {
        let serial = b"CYBRRD-WITH-OP-ID";
        let mut buf = build_pack_header(4);
        buf.resize(3 + 4 * SUB, 0);
        buf[3] = 0x00;
        buf[5..5 + serial.len()].copy_from_slice(serial);
        put_location(&mut buf, 3 + SUB, 407_608_000, -953_702_000, 2200);
        let op = 3 + 2 * SUB;
        buf[op] = 0x50;
        buf[op + 2..op + 2 + 16].copy_from_slice(b"FAA-OP-USA-12345");
        let sid = 3 + 3 * SUB;
        buf[sid] = 0x30;
        buf[sid + 2..sid + 2 + 21].copy_from_slice(b"Mapping survey flight");
        let r = parse_message_pack(&buf, [0; 6], -40).expect("should parse");
        assert_eq!(r.operator_id.as_deref(), Some("FAA-OP-USA-12345"));
        let s = r.self_id.expect("self_id must be set");
        assert_eq!(s.desc_type, 0);
        assert_eq!(s.description, "Mapping survey flight");
    }

    #[test]
    fn parses_auth_page_zero_metadata() {
        let serial = b"CYBRRD-WITH-AUTH";
        let mut buf = build_pack_header(3);
        buf.resize(3 + 3 * SUB, 0);
        buf[3] = 0x00;
        buf[5..5 + serial.len()].copy_from_slice(serial);
        put_location(&mut buf, 3 + SUB, 407_608_000, -953_702_000, 2200);
        let au = 3 + 2 * SUB;
        buf[au] = 0x20;
        buf[au + 1] = 0x20; // auth_type 2, page 0
        buf[au + 2] = 0x01;
        buf[au + 3] = 27;
        buf[au + 4..au + 8].copy_from_slice(&31_536_000_u32.to_le_bytes());
        let r = parse_message_pack(&buf, [0; 6], -40).expect("should parse");
        let a = r.auth.expect("auth must be set");
        assert_eq!(a.auth_type, 2);
        assert_eq!(a.last_page_index, Some(1));
        assert_eq!(a.total_length, Some(27));
        assert_eq!(a.timestamp_utc, Some(1_577_836_800));
    }

    #[test]
    fn protocol_type_serializes_to_canonical_wire_name() {
        let json = serde_json::to_string(&ProtocolType::AstmF3411_22a).unwrap();
        assert_eq!(json, "\"ASTM_F3411_22a\"");
    }

    /// Verbatim counter + Message Pack from a real DJI Mini 5 Pro broadcast
    /// (test-node-2, 2026-05-07). Byte 0 is the Wi-Fi message counter. This
    /// compatibility entry point preserves the pre-router decoded fields.
    #[test]
    fn parses_real_dji_mini5pro_to_wire() {
        let payload: [u8; 79] = [
            0x01, 0xf2, 0x19, 0x03, 0x02, 0x12, b'1', b'5', b'8', b'1', b'F', b'9', b'D', b'E',
            b'C', b'2', b'6', b'1', b'8', b'0', b'2', b'9', b'6', b'6', b'X', b'D', 0x00, 0x00,
            0x00, 0x12, 0x16, 0xb5, 0x00, 0x00, 0x12, 0x30, 0x5e, 0x18, 0x6a, 0xba, 0xf6, 0xc6,
            0x00, 0x00, 0x46, 0x0a, 0xd0, 0x07, 0x2c, 0x04, 0xa6, 0x48, 0x0a, 0x00, 0x42, 0x01,
            0x26, 0x31, 0x5e, 0x18, 0xdb, 0xb9, 0xf6, 0xc6, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x4b, 0x0a, 0xe3, 0x2c, 0xd2, 0x0d, 0x00,
        ];
        let mac = [0x8c, 0x1e, 0xd9, 0x56, 0xe5, 0x81];
        let r = parse_message_pack(&payload, mac, -52).expect("real DJI frame must parse");
        assert_eq!(r.drone_id, "1581F9DEC261802966XD");
        assert_eq!(r.hardware_serial.as_deref(), Some("1581F9DEC261802966XD"));
        assert_eq!(r.signal_rssi_dbm, -52);
        assert!(r.pos.as_ref().unwrap().lat != 0.0 || r.pos.as_ref().unwrap().lon != 0.0);
    }
}
