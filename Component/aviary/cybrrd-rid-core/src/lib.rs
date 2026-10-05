// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! `cybrrd-rid-core` — `no_std`/heapless ASTM F3411-22a Open Drone ID decoder.
//!
//! The shared decode substrate for every cyBRRD Node: the Raspberry-Pi engine
//! (through the `cybrrd-rid-protocol` std wrapper) and the ESP32-S3 Sentinel
//! firmware decode against THIS crate. It owns the byte-level truth of the
//! ASTM Message Pack and nothing else — no serde, no NATS, no std.
//!
//! ## Indestructibility contract
//! This parser runs on raw, untrusted, possibly-malicious over-the-air bytes
//! on a device with no operator present and `panic = "abort"`. It therefore
//! **never panics**:
//!   - every multi-byte read is on a fixed-width `chunks_exact(25)` slice, so
//!     in-sub-message indexing can never go out of bounds;
//!   - string fields are copied into `heapless::String<N>` with an explicit
//!     capacity guard — over-capacity input truncates, it never reallocates
//!     or aborts;
//!   - structural failure yields `Err(ParseError)`, never a crash.
//!
//! ## Message types decoded
//!   Type 0 BASIC_ID    — UAS ID, split by IDType into hardware serial (1)
//!                        vs CAA/FAA registration (2); they never overwrite.
//!   Type 1 LOCATION    — drone lat / lon / geodetic altitude
//!   Type 2 AUTH        — authentication metadata (page-0 fields only)
//!   Type 3 SELF_ID     — operator flight-purpose description text
//!   Type 4 SYSTEM      — operator lat / lon / altitude
//!   Type 5 OPERATOR_ID — operator registration ID

#![cfg_attr(not(test), no_std)]
// #179: runtime code never unwrap()/expect() (compiler-enforced; tests exempt)
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use heapless::String;

#[cfg(test)]
mod differential;

/// ASTM UAS-ID / Operator-ID field width (bytes). Capacity for serial,
/// registration, and operator-id heapless strings.
/// WHAT: max chars in a UAS ID. WHY: ASTM F3411-22a fixes the ID field at
/// 20 bytes. WHEN-to-tune: never, unless the spec revises. DEPENDS-ON:
/// `hardware_serial`/`caa_registration`/`operator_id` capacities below.
pub const ID_CAP: usize = 20;

/// ASTM Self-ID description field width (bytes).
/// WHAT: max chars in a flight-purpose description. WHY: ASTM fixes Self-ID
/// Description at 23 bytes. WHEN-to-tune: never, unless the spec revises.
pub const DESC_CAP: usize = 23;

const SUB_MESSAGE_LEN: usize = 25;
// ASTM altitude unknown is raw 0 (-1000m), not timestamp's 0xFFFF.
// Standards/F3411/corpus/schemas/encodings.yaml: encodings.altitude.
const ALTITUDE_UNKNOWN: u16 = 0;
const ALTITUDE_OFFSET_M: f32 = 1000.0;

/// ASTM Authentication-Message timestamp epoch: 2019-01-01 00:00:00 UTC.
const ASTM_EPOCH_UNIX: u64 = 1_546_300_800;

const MSG_TYPE_BASIC_ID: u8 = 0x0;
const MSG_TYPE_LOCATION: u8 = 0x1;
const MSG_TYPE_AUTH: u8 = 0x2;
const MSG_TYPE_SELF_ID: u8 = 0x3;
const MSG_TYPE_SYSTEM: u8 = 0x4;
const MSG_TYPE_OPERATOR_ID: u8 = 0x5;

/// ASTM BASIC_ID `IDType` (high nibble of sub-message byte 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdType {
    /// 0 — no/unspecified ID type (still carries an ID field in practice).
    None,
    /// 1 — Serial Number (CTA-2063-A manufacturer serial). Consumer DJI.
    SerialNumber,
    /// 2 — CAA-assigned Registration ID (e.g. FAA registration). Part-107.
    CaaRegistration,
    /// 3 — UTM (USS)-assigned UUID.
    Utm,
    /// 4 — Specific session ID.
    SpecificSession,
    /// Any other / reserved value, preserved verbatim.
    Other(u8),
}

impl IdType {
    fn from_nibble(n: u8) -> Self {
        match n {
            0 => IdType::None,
            1 => IdType::SerialNumber,
            2 => IdType::CaaRegistration,
            3 => IdType::Utm,
            4 => IdType::SpecificSession,
            other => IdType::Other(other),
        }
    }
}

/// A geographic point (drone or operator). Geodetic; known `alt_m` is metres
/// MSL. None retains a valid 2D fix without inventing an altitude.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
    pub alt_m: Option<f32>,
}

/// Operator's free-text flight-purpose description (ASTM Type 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfId {
    /// 0=Text, 1=Emergency, 2=Extended Status.
    pub desc_type: u8,
    pub description: String<DESC_CAP>,
}

/// Authentication-message page-0 metadata (ASTM Type 2). CE scope: we record
/// that auth WAS broadcast; we do not reassemble multipart auth_data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthMeta {
    pub auth_type: u8,
    pub timestamp_utc: Option<u64>,
    pub total_length: Option<u8>,
    pub last_page_index: Option<u8>,
}

/// The decoded over-the-air content of one ASTM Message Pack.
///
/// `hardware_serial` (IDType 1) and `caa_registration` (IDType 2) are kept
/// SEPARATE so a drone broadcasting both never overwrites one with the other
/// (the Memorial Park forensic gap). First-of-each-type wins.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RidPack {
    pub hardware_serial: Option<String<ID_CAP>>,
    pub caa_registration: Option<String<ID_CAP>>,
    pub drone_pos: Option<GeoPoint>,
    pub operator_pos: Option<GeoPoint>,
    /// Observed LOCATION status nibble, including undeclared/reserved values.
    pub operational_status: Option<u8>,
    /// Decoder inference from the received coordinates, not a status claim.
    pub position_unknown_reason: Option<PositionUnknownReason>,
    pub operator_position_unknown_reason: Option<PositionUnknownReason>,
    pub operator_id: Option<String<ID_CAP>>,
    pub self_id: Option<SelfId>,
    pub auth: Option<AuthMeta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionUnknownReason {
    ZeroPair,
    OutOfRange,
    NoLocation,
}

impl RidPack {
    /// LOCATION (even unknown/implausible) or aircraft identity is an observation.
    pub fn has_observation(&self) -> bool {
        self.operational_status.is_some() || self.hardware_serial.is_some() || self.caa_registration.is_some()
    }
}

/// Why a Message Pack could not be decoded. Always returned instead of a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// Fewer than the 3-byte Pack header.
    TooShort,
    /// High nibble of the header byte is not 0xF — not a Message Pack.
    NotAMessagePack,
    /// MessageSize byte is not the ASTM-mandated 25 (misaligned / non-conformant).
    BadMessageSize,
    /// Message-count byte is outside the ASTM transport limit of 1..=9.
    BadMessageCount,
    /// Payload length does not exactly match the declared message count.
    BadMessagePackLength,
    /// Legacy error name: no LOCATION and no usable aircraft identity.
    /// Unknown position alone is not a rejection in wire v4.
    NoLocation,
}

/// Decode a raw ODID Message Pack payload (the bytes following the `0x0D`
/// ODID OUI-type marker inside a Vendor Specific IE).
///
/// Retains the historical one-byte-prefix and truncated-count tolerance for
/// callers that already validate their transport boundary. New transport
/// deframers should use [`decode_message_pack_strict`] after removing their
/// message counter. Returns `Err` on structural failure or when no LOCATION or
/// aircraft identity is present. Never panics.
pub fn decode_message_pack(payload: &[u8]) -> Result<RidPack, ParseError> {
    // Compatibility behavior: slide one byte when a legacy caller includes
    // transport framing. Transport-specific code must not depend on this.
    let p = if !payload.is_empty() && (payload[0] >> 4) != 0xF {
        &payload[1..]
    } else {
        payload
    };

    if p.len() < 3 {
        return Err(ParseError::TooShort);
    }
    if (p[0] >> 4) != 0xF {
        return Err(ParseError::NotAMessagePack);
    }
    let msg_size = p[1] as usize;
    if msg_size != SUB_MESSAGE_LEN {
        // Locked at 25 by ASTM; a different size means misalignment or a
        // non-conformant broadcaster. Bail rather than mis-slice the Lake.
        return Err(ParseError::BadMessageSize);
    }
    let claimed_count = p[2] as usize;
    let claimed_len = 3 + claimed_count * SUB_MESSAGE_LEN;

    // Trust-but-verify the count: if firmware over-claims, take only what's
    // actually present so chunks_exact() never sees a short tail.
    let pack_data: &[u8] = if p.len() >= claimed_len {
        &p[3..claimed_len]
    } else {
        let available = p.len().saturating_sub(3) / SUB_MESSAGE_LEN;
        &p[3..3 + available * SUB_MESSAGE_LEN]
    };

    let out = decode_submessages(pack_data);
    if !out.has_observation() {
        return Err(ParseError::NoLocation);
    }
    Ok(out)
}

/// Decode an exactly framed ASTM Message Pack.
///
/// The caller must remove any transport-level message counter first. This
/// entry point accepts only a Pack header at byte zero, MessageSize 25, a
/// count in 1..=9, and an exact count/length match. It performs no byte slide
/// and no truncated-pack salvage.
pub fn decode_message_pack_strict(payload: &[u8]) -> Result<RidPack, ParseError> {
    if payload.len() < 3 {
        return Err(ParseError::TooShort);
    }
    if (payload[0] >> 4) != 0xF {
        return Err(ParseError::NotAMessagePack);
    }
    if payload[1] as usize != SUB_MESSAGE_LEN {
        return Err(ParseError::BadMessageSize);
    }
    let count = payload[2] as usize;
    if !(1..=9).contains(&count) {
        return Err(ParseError::BadMessageCount);
    }
    let required_len = 3 + count * SUB_MESSAGE_LEN;
    if payload.len() != required_len {
        return Err(ParseError::BadMessagePackLength);
    }

    let out = decode_submessages(&payload[3..]);
    if !out.has_observation() {
        return Err(ParseError::NoLocation);
    }
    Ok(out)
}

/// Decode exactly one BT4 ODID message through the same sub-message decoder.
/// Unlike the map-oriented pack entry point, a Basic ID alone is meaningful
/// here; transport assembly must wait for a Location before emitting telemetry.
pub fn decode_single_message(payload: &[u8]) -> Result<RidPack, ParseError> {
    if payload.len() != SUB_MESSAGE_LEN {
        return Err(ParseError::BadMessageSize);
    }
    Ok(decode_submessages(payload))
}

fn decode_submessages(pack_data: &[u8]) -> RidPack {
    let mut out = RidPack::default();

    for chunk in pack_data.as_chunks::<SUB_MESSAGE_LEN>().0 {
        // chunk is exactly 25 bytes — every chunk[i] for i < 25 is safe.
        match (chunk[0] & 0xF0) >> 4 {
            MSG_TYPE_BASIC_ID => {
                let id_type = IdType::from_nibble((chunk[1] & 0xF0) >> 4);
                if let Some(id) = bounded_id(&chunk[2..22]) {
                    match id_type {
                        IdType::SerialNumber => {
                            if out.hardware_serial.is_none() {
                                out.hardware_serial = Some(id);
                            }
                        }
                        IdType::CaaRegistration => {
                            if out.caa_registration.is_none() {
                                out.caa_registration = Some(id);
                            }
                        }
                        // IDType 0 (None) / UTM / session / reserved: many
                        // consumer drones don't set a meaningful IDType. Treat
                        // an unclassified-but-present ID as the hardware serial
                        // only if we have no typed ID yet — back-compatible
                        // with the prior single-field behaviour.
                        _ => {
                            if out.hardware_serial.is_none() && out.caa_registration.is_none() {
                                out.hardware_serial = Some(id);
                            }
                        }
                    }
                }
            }
            MSG_TYPE_LOCATION => {
                out.operational_status = Some(chunk[1] >> 4);
                out.drone_pos = decode_location(chunk);
                let (lat, lon) = location_coordinates(chunk);
                out.position_unknown_reason = position_unknown_reason(lat, lon);
            }
            MSG_TYPE_SYSTEM => {
                // OperatorLatitude/Longitude i32 LE @1e-7 at bytes 2..10,
                // OperatorAltitudeGeo u16 LE (+1000m, 0.5m res) at bytes 18..20.
                let lat = i32::from_le_bytes([chunk[2], chunk[3], chunk[4], chunk[5]]);
                let lon = i32::from_le_bytes([chunk[6], chunk[7], chunk[8], chunk[9]]);
                // Decode all three altitude-encoded SYSTEM fields through
                // the shared primitive. Area bounds are not on the envelope yet.
                let [_area_ceiling, _area_floor, operator_altitude] = system_altitudes(chunk);
                out.operator_pos = validated_point_with_altitude(lat, lon, operator_altitude);
                out.operator_position_unknown_reason = position_unknown_reason(lat, lon);
            }
            MSG_TYPE_OPERATOR_ID => {
                // byte 1 = OperatorIdType; bytes 2..22 = OperatorId (20B).
                if out.operator_id.is_none() {
                    out.operator_id = bounded_id(&chunk[2..22]);
                }
            }
            MSG_TYPE_SELF_ID => {
                // byte 1 = DescType; bytes 2..25 = Description (23B).
                if out.self_id.is_none() {
                    if let Some(description) = bounded_desc(&chunk[2..25]) {
                        out.self_id = Some(SelfId {
                            desc_type: chunk[1],
                            description,
                        });
                    }
                }
            }
            MSG_TYPE_AUTH if out.auth.is_none() => {
                // byte 1 = AuthType (high nibble) | DataPage (low nibble).
                // First seen wins — don't let a continuation page clobber page 0.
                let auth_type = (chunk[1] & 0xF0) >> 4;
                let data_page = chunk[1] & 0x0F;
                if data_page == 0 {
                    let last_page_index = chunk[2];
                    let total_length = chunk[3];
                    let astm_ts =
                        u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                    out.auth = Some(AuthMeta {
                        auth_type,
                        timestamp_utc: Some(ASTM_EPOCH_UNIX + astm_ts as u64),
                        total_length: Some(total_length),
                        last_page_index: Some(last_page_index),
                    });
                } else {
                    out.auth = Some(AuthMeta {
                        auth_type,
                        timestamp_utc: None,
                        total_length: None,
                        last_page_index: None,
                    });
                }
            }
            _ => {} // Types 6/7 reserved; ignored.
        }
    }

    if out.operational_status.is_none() && out.has_observation() {
        out.position_unknown_reason = Some(PositionUnknownReason::NoLocation);
    }
    out
}

/// Copy a null-padded UTF-8 ID field into a capacity-bounded string.
/// Never panics: stops at the first NUL, validates UTF-8, trims, and pushes
/// with an explicit capacity guard (truncates rather than reallocating).
fn bounded_id(bytes: &[u8]) -> Option<String<ID_CAP>> {
    bounded::<ID_CAP>(bytes)
}

fn bounded_desc(bytes: &[u8]) -> Option<String<DESC_CAP>> {
    bounded::<DESC_CAP>(bytes)
}

fn bounded<const N: usize>(bytes: &[u8]) -> Option<String<N>> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    // Cap the slice at N bytes up front so a malformed over-length field can
    // never push past capacity (belt-and-suspenders with the push guard).
    let capped = &bytes[..end.min(N)];
    let s = core::str::from_utf8(capped).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out: String<N> = String::new();
    for ch in trimmed.chars() {
        // push() returns Err when full — truncate gracefully, never panic.
        if out.push(ch).is_err() {
            break;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// ASTM altitude encoding, shared by all six LOCATION/SYSTEM altitude fields.
/// Raw zero is unknown; 0xFFFF is the valid upper endpoint (31767.5m).
pub fn decode_altitude(raw: u16) -> Option<f32> {
    (raw != ALTITUDE_UNKNOWN).then(|| raw as f32 * 0.5 - ALTITUDE_OFFSET_M)
}

fn altitude_at(chunk: &[u8], offset: usize) -> Option<f32> {
    decode_altitude(u16::from_le_bytes([chunk[offset], chunk[offset + 1]]))
}

fn location_altitudes(chunk: &[u8]) -> [Option<f32>; 3] {
    // Table 6: pressure_altitude, geodetic_altitude, height.
    [13, 15, 17].map(|offset| altitude_at(chunk, offset))
}

fn system_altitudes(chunk: &[u8]) -> [Option<f32>; 3] {
    // Table 11: area_ceiling, area_floor, operator_altitude (pilot).
    [13, 15, 18].map(|offset| altitude_at(chunk, offset))
}

fn location_coordinates(chunk: &[u8]) -> (i32, i32) {
    (i32::from_le_bytes([chunk[5], chunk[6], chunk[7], chunk[8]]),
     i32::from_le_bytes([chunk[9], chunk[10], chunk[11], chunk[12]]))
}

fn decode_location(chunk: &[u8]) -> Option<GeoPoint> {
    // Latitude i32 LE @1e-7 at bytes 5..9, Longitude at 9..13,
    // Geodetic Altitude u16 LE (+1000m, 0.5m res) at bytes 15..17.
    let (lat_raw, lon_raw) = location_coordinates(chunk);
    // Preserve the decode path for the two fields not yet on the envelope.
    let [_pressure_altitude, geodetic_altitude, _height] = location_altitudes(chunk);
    validated_point_with_altitude(lat_raw, lon_raw, geodetic_altitude)
}

// Compatibility adapter for D18's frozen loop reference (which intentionally
// shares the live scalar/coordinate helpers). Its reference body is unchanged.
#[cfg(test)]
fn validated_point(lat_raw: i32, lon_raw: i32, alt_raw: u16) -> Option<GeoPoint> {
    validated_point_with_altitude(lat_raw, lon_raw, decode_altitude(alt_raw))
}

fn validated_point_with_altitude(lat_raw: i32, lon_raw: i32, alt_m: Option<f32>) -> Option<GeoPoint> {
    if position_unknown_reason(lat_raw, lon_raw).is_some() {
        return None;
    }
    let lat = lat_raw as f64 / 10_000_000.0;
    let lon = lon_raw as f64 / 10_000_000.0;
    // Preserve valid lat/lon rather than inventing an altitude: wire v4
    // omits alt_m for the raw-0 unknown code. Every nonzero code, including
    // 0xFFFF (31767.5m), is numeric. Never replace missing position with 0,0.
    Some(GeoPoint { lat, lon, alt_m })
}

fn position_unknown_reason(lat_raw: i32, lon_raw: i32) -> Option<PositionUnknownReason> {
    if lat_raw == 0 && lon_raw == 0 {
        Some(PositionUnknownReason::ZeroPair)
    } else if !(-900_000_000..=900_000_000).contains(&lat_raw)
        || !(-1_800_000_000..=1_800_000_000).contains(&lon_raw) {
        Some(PositionUnknownReason::OutOfRange)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_pack_header(count: u8) -> heapless::Vec<u8, 256> {
        let mut v = heapless::Vec::new();
        v.extend_from_slice(&[0xF1, SUB_MESSAGE_LEN as u8, count]).unwrap();
        v
    }

    /// Put a BASIC_ID sub-message at `buf[off..]` with the given IDType nibble.
    fn put_basic_id(buf: &mut [u8], off: usize, id_type_nibble: u8, id: &[u8]) {
        buf[off] = 0x00; // MessageType 0
        buf[off + 1] = id_type_nibble << 4;
        let n = id.len().min(20);
        buf[off + 2..off + 2 + n].copy_from_slice(&id[..n]);
    }

    fn put_location(buf: &mut [u8], off: usize, lat_e7: i32, lon_e7: i32, alt_raw: u16) {
        buf[off] = 0x10; // MessageType 1
        buf[off + 5..off + 9].copy_from_slice(&lat_e7.to_le_bytes());
        buf[off + 9..off + 13].copy_from_slice(&lon_e7.to_le_bytes());
        buf[off + 15..off + 17].copy_from_slice(&alt_raw.to_le_bytes());
    }

    #[test]
    fn dual_basic_id_serial_and_registration_do_not_overwrite() {
        // 3 subs: BASIC_ID(serial, type 1) + BASIC_ID(registration, type 2) + LOCATION
        let mut buf = [0u8; 3 + 3 * SUB_MESSAGE_LEN];
        buf[..3].copy_from_slice(&[0xF1, 25, 3]);
        put_basic_id(&mut buf, 3, 1, b"1581FBEXC25AM00D88VJ"); // serial
        put_basic_id(&mut buf, 3 + SUB_MESSAGE_LEN, 2, b"FA3GPQK7L9"); // CAA reg
        put_location(&mut buf, 3 + 2 * SUB_MESSAGE_LEN, 407_608_000, -953_702_000, 2200);

        let pack = decode_message_pack(&buf).expect("should decode");
        assert_eq!(pack.hardware_serial.as_deref(), Some("1581FBEXC25AM00D88VJ"));
        assert_eq!(pack.caa_registration.as_deref(), Some("FA3GPQK7L9"));
    }

    #[test]
    fn untyped_basic_id_falls_back_to_serial() {
        // IDType nibble 0 (None) — consumer-drone common case.
        let mut buf = [0u8; 3 + 2 * SUB_MESSAGE_LEN];
        buf[..3].copy_from_slice(&[0xF1, 25, 2]);
        put_basic_id(&mut buf, 3, 0, b"UNTYPED-SERIAL-01");
        put_location(&mut buf, 3 + SUB_MESSAGE_LEN, 407_608_000, -953_702_000, 2200);
        let pack = decode_message_pack(&buf).unwrap();
        assert_eq!(pack.hardware_serial.as_deref(), Some("UNTYPED-SERIAL-01"));
        assert!(pack.caa_registration.is_none());
    }

    #[test]
    fn over_capacity_id_truncates_never_panics() {
        // A BASIC_ID whose 20 ID bytes are entirely non-null 'A' — full field,
        // no NUL terminator. Must truncate to ID_CAP, never panic/realloc.
        let mut buf = [0u8; 3 + 2 * SUB_MESSAGE_LEN];
        buf[..3].copy_from_slice(&[0xF1, 25, 2]);
        buf[3] = 0x00;
        buf[3 + 1] = 1 << 4; // serial
        for i in 0..20 {
            buf[3 + 2 + i] = b'A';
        }
        put_location(&mut buf, 3 + SUB_MESSAGE_LEN, 1, 1, 2200);
        let pack = decode_message_pack(&buf).unwrap();
        let s = pack.hardware_serial.expect("serial present");
        assert_eq!(s.len(), ID_CAP);
        assert!(s.chars().all(|c| c == 'A'));
    }

    #[test]
    fn no_location_identity_is_retained_without_position() {
        let mut buf = [0u8; 3 + SUB_MESSAGE_LEN];
        buf[..3].copy_from_slice(&[0xF1, 25, 1]);
        put_basic_id(&mut buf, 3, 1, b"SERIAL-ONLY");
        let pack = decode_message_pack(&buf).unwrap();
        assert!(pack.drone_pos.is_none());
        assert_eq!(pack.position_unknown_reason, Some(PositionUnknownReason::NoLocation));
        assert_eq!(pack.hardware_serial.as_deref(), Some("SERIAL-ONLY"));
    }

    #[test]
    fn structural_rejections_return_err() {
        // Empty → too short.
        assert_eq!(decode_message_pack(&[]), Err(ParseError::TooShort));
        // A lone non-0xF byte is treated as legacy transport framing and slid
        // off, leaving a 2-byte remainder → TooShort.
        assert_eq!(decode_message_pack(&[0x10, 25, 1]), Err(ParseError::TooShort));
        // Prefix slid off, but the real header byte is still not a Pack (0xF).
        assert_eq!(
            decode_message_pack(&[0x01, 0x10, 25, 1]),
            Err(ParseError::NotAMessagePack)
        );
        // Valid Pack header nibble but MessageSize != 25 → misaligned.
        assert_eq!(
            decode_message_pack(&[0xF0, 18, 1, 0xFF]),
            Err(ParseError::BadMessageSize)
        );
    }

    #[test]
    fn strict_pack_requires_exact_standard_framing() {
        let mut valid = [0u8; 3 + SUB_MESSAGE_LEN];
        valid[..3].copy_from_slice(&[0xF1, 25, 1]);
        put_basic_id(&mut valid, 3, 1, b"STRICT-SERIAL");
        assert_eq!(
            decode_message_pack_strict(&valid)
                .unwrap()
                .hardware_serial
                .as_deref(),
            Some("STRICT-SERIAL")
        );

        let mut prefixed = [0u8; 4 + SUB_MESSAGE_LEN];
        prefixed[1..].copy_from_slice(&valid);
        assert_eq!(
            decode_message_pack_strict(&prefixed),
            Err(ParseError::NotAMessagePack)
        );

        let mut bad_count = valid;
        bad_count[2] = 0;
        assert_eq!(
            decode_message_pack_strict(&bad_count),
            Err(ParseError::BadMessageCount)
        );
        bad_count[2] = 10;
        assert_eq!(
            decode_message_pack_strict(&bad_count),
            Err(ParseError::BadMessageCount)
        );

        assert_eq!(
            decode_message_pack_strict(&valid[..valid.len() - 1]),
            Err(ParseError::BadMessagePackLength)
        );
        let mut trailing = [0u8; 4 + SUB_MESSAGE_LEN];
        trailing[..valid.len()].copy_from_slice(&valid);
        assert_eq!(
            decode_message_pack_strict(&trailing),
            Err(ParseError::BadMessagePackLength)
        );
    }

    #[test]
    fn over_claimed_count_does_not_panic() {
        // Header claims 5 subs; only 1 (LOCATION) provided. Must not panic.
        let mut buf = [0u8; 3 + SUB_MESSAGE_LEN];
        buf[..3].copy_from_slice(&[0xF1, 25, 5]);
        put_location(&mut buf, 3, 407_608_000, -953_702_000, 2200);
        let pack = decode_message_pack(&buf).expect("partial pack still decodes");
        assert!((pack.drone_pos.unwrap().lat - 40.7608).abs() < 1e-6);
    }

    /// Verbatim counter + Message Pack from a real DJI Mini 5 Pro broadcast,
    /// captured by test-node-2 2026-05-07. Byte 0 is the standard Wi-Fi
    /// message counter; the historical compatibility decoder removes it.
    /// IDType nibble of the BASIC_ID byte 1 (0x12) is 0x1 → SerialNumber.
    #[test]
    fn real_dji_mini5pro_decodes_serial_and_position() {
        let payload: [u8; 79] = [
            0x01, 0xf2, 0x19, 0x03,
            // BASIC_ID (byte1 = 0x12 -> IDType 1 Serial)
            0x02, 0x12, b'1', b'5', b'8', b'1', b'F', b'9', b'D', b'E', b'C', b'2',
            b'6', b'1', b'8', b'0', b'2', b'9', b'6', b'6', b'X', b'D', 0x00, 0x00, 0x00,
            // LOCATION
            0x12, 0x16, 0xb5, 0x00, 0x00, 0x12, 0x30, 0x5e, 0x18, 0x6a, 0xba, 0xf6,
            0xc6, 0x00, 0x00, 0x46, 0x0a, 0xd0, 0x07, 0x2c, 0x04, 0xa6, 0x48, 0x0a, 0x00,
            // SYSTEM
            0x42, 0x01, 0x26, 0x31, 0x5e, 0x18, 0xdb, 0xb9, 0xf6, 0xc6, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4b, 0x0a, 0xe3, 0x2c, 0xd2, 0x0d, 0x00,
        ];
        let pack = decode_message_pack(&payload).expect("real DJI frame must decode");
        assert_eq!(pack.hardware_serial.as_deref(), Some("1581F9DEC261802966XD"));
        assert!(pack.caa_registration.is_none());
        let pos = pack.drone_pos.expect("location present");
        assert!(pos.lat != 0.0 || pos.lon != 0.0);
        assert!(pack.operator_pos.is_some());
    }

    #[test]
    fn altitude_coordinate_ranges_remain_rejected() {
        let mut buf = [0u8; 3 + SUB_MESSAGE_LEN];
        buf[..3].copy_from_slice(&[0xF1, 25, 1]);
        for (lat,lon) in [(900_000_001,0),(-900_000_001,0),(0,1_800_000_001),(0,-1_800_000_001)] {
            put_location(&mut buf,3,lat,lon,2200);
            let decoded = decode_message_pack(&buf).unwrap();
            assert!(decoded.drone_pos.is_none());
            assert_eq!(decoded.position_unknown_reason, Some(PositionUnknownReason::OutOfRange));
        }
    }

    // D27 preregistered in PR #38 before execution. Absolute corpus-derived
    // expectations: the D18 frozen loop shares validated_point and cannot
    // independently catch a transposed altitude sentinel.
    fn assert_altitude(message_type: u8, raw: u16, expected: Option<f32>) {
        let mut message = [0u8; SUB_MESSAGE_LEN];
        if message_type == MSG_TYPE_LOCATION {
            put_location(&mut message, 0, 10_000_000, 20_000_000, raw);
        } else {
            message[0] = MSG_TYPE_SYSTEM << 4;
            message[2..6].copy_from_slice(&10_000_000_i32.to_le_bytes());
            message[6..10].copy_from_slice(&20_000_000_i32.to_le_bytes());
            message[18..20].copy_from_slice(&raw.to_le_bytes());
        }
        let pack = decode_single_message(&message).expect("valid submessage");
        let point = if message_type == MSG_TYPE_LOCATION { pack.drone_pos } else { pack.operator_pos }
            .expect("valid lat/lon must survive any altitude code");
        assert_eq!((point.lat, point.lon), (1.0, 2.0));
        let altitude: Option<f32> = point.alt_m;
        assert_eq!(altitude, expected, "raw altitude {raw:#06x}, type {message_type}");

        if message_type == MSG_TYPE_LOCATION {
            let mut bytes = [0u8; 3 + SUB_MESSAGE_LEN];
            bytes[..3].copy_from_slice(&[0xF1, 25, 1]);
            bytes[3..].copy_from_slice(&message);
            assert_eq!(decode_message_pack(&bytes).unwrap().drone_pos, Some(point));
        }
    }

    #[test]
    fn altitude_unknown_location_retains_2d_fix() { assert_altitude(MSG_TYPE_LOCATION, 0, None); }

    #[test]
    fn altitude_unknown_system_retains_2d_fix() { assert_altitude(MSG_TYPE_SYSTEM, 0, None); }

    #[test]
    fn altitude_maximum_location_is_valid() { assert_altitude(MSG_TYPE_LOCATION, 0xFFFF, Some(31767.5)); }

    #[test]
    fn altitude_maximum_system_is_valid() { assert_altitude(MSG_TYPE_SYSTEM, 0xFFFF, Some(31767.5)); }

    #[test]
    fn altitude_known_zero_and_ordinary_values() {
        for kind in [MSG_TYPE_LOCATION, MSG_TYPE_SYSTEM] {
            for (raw, expected) in [(1999, -0.5), (2000, 0.0), (2200, 100.0)] {
                assert_altitude(kind, raw, Some(expected));
            }
        }
    }

    #[test]
    fn altitude_all_six_field_offsets_share_optional_encoding() {
        for (offsets, decode) in [
            ([13, 15, 17], location_altitudes as fn(&[u8]) -> [Option<f32>; 3]),
            ([13, 15, 18], system_altitudes as fn(&[u8]) -> [Option<f32>; 3]),
        ] {
            for field in 0..3 {
                for (raw, expected) in [(0_u16, None), (1, Some(-999.5)),
                    (1999, Some(-0.5)), (2000, Some(0.0)), (2200, Some(100.0)),
                    (65535, Some(31767.5))] {
                    let mut message = [0u8; 25];
                    for offset in offsets {
                        message[offset..offset + 2].copy_from_slice(&2200_u16.to_le_bytes());
                    }
                    let offset = offsets[field];
                    message[offset..offset + 2].copy_from_slice(&raw.to_le_bytes());
                    let mut want = [Some(100.0); 3];
                    want[field] = expected;
                    assert_eq!(decode(&message), want, "offset {offset}, raw {raw}");
                }
            }
        }
    }
}
