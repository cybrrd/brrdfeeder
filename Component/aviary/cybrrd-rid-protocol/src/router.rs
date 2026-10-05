// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! 802.11 frame walker → consolidated ASTM parser.
//!
//! Single runtime entry point. Identifies whether a captured frame carries
//! an ASTM Open Drone ID vendor IE or NAN Service Discovery Frame; delegates to the
//! canonical strict ASTM parser for field extraction.

use crate::astm;
use crate::models::{RidTransport, TelemetryData};

// Wi-Fi Alliance OUI for Open Drone ID (0xFA, 0x0B, 0xBC).
const FA_OUI: [u8; 3] = [0xFA, 0x0B, 0xBC];
const ODID_OUI_TYPE: u8 = 0x0D;

const FC_BEACON: u8 = 0x80;
const FC_PROBE_RESPONSE: u8 = 0x50;

const MIN_802_11_MGMT_LEN: usize = 36;
const TRANSMITTER_MAC_OFFSET: usize = 10;
const IE_HEADER_LEN: usize = 2;
const VENDOR_SPECIFIC_IE: u8 = 0xDD;
const MIN_VENDOR_IE_LEN: usize = 4; // OUI(3) + OUI-type(1)

/// Process a raw 802.11 management frame (Beacon, Probe Response, or NAN Action)
/// and emit per-frame telemetry data if it carries an ASTM Open Drone ID
/// Message Pack.
///
/// Returns `None` for unrelated or malformed frames. NAN input must exclude any
/// capture-level FCS trailer; the engine removes it using radiotap metadata.
pub fn ingest_frame(dot11_payload: &[u8], rssi_dbm: i32) -> Option<TelemetryData> {
    if dot11_payload.first() == Some(&0xd0) {
        return crate::nan::ingest_sdf(dot11_payload, rssi_dbm);
    }
    if dot11_payload.len() < MIN_802_11_MGMT_LEN {
        return None;
    }

    let frame_control = dot11_payload[0];
    if frame_control != FC_BEACON && frame_control != FC_PROBE_RESPONSE {
        return None;
    }

    // Transmitter MAC: 802.11 Address 2, bytes 10..16.
    let mut mac_address = [0u8; 6];
    mac_address.copy_from_slice(&dot11_payload[TRANSMITTER_MAC_OFFSET..TRANSMITTER_MAC_OFFSET + 6]);

    // Walk Information Elements after the 36-byte management header.
    let mut offset = MIN_802_11_MGMT_LEN;
    while offset + IE_HEADER_LEN <= dot11_payload.len() {
        let ie_id = dot11_payload[offset];
        let ie_len = dot11_payload[offset + 1] as usize;
        let ie_end = offset + IE_HEADER_LEN + ie_len;
        if ie_end > dot11_payload.len() {
            break; // Truncated frame; bail rather than overrun.
        }

        if ie_id == VENDOR_SPECIFIC_IE && ie_len >= MIN_VENDOR_IE_LEN {
            let oui = &dot11_payload[offset + IE_HEADER_LEN..offset + IE_HEADER_LEN + 3];
            let oui_type = dot11_payload[offset + IE_HEADER_LEN + 3];

            if oui == FA_OUI && oui_type == ODID_OUI_TYPE {
                // ASTM Beacon/Probe Response framing is:
                // OUI(3) + OUI-type(1) + message counter(1) + Message Pack.
                // Peel the counter here so the core sees a Pack header at byte
                // zero and never has to guess about transport framing.
                let framed_start = offset + IE_HEADER_LEN + MIN_VENDOR_IE_LEN;
                let framed = &dot11_payload[framed_start..ie_end];
                return astm::parse_wifi_service_info(
                    framed, mac_address, rssi_dbm, RidTransport::WifiBeacon,
                );
            }
        }
        offset = ie_end;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [0x8c, 0x1e, 0xd9, 0x56, 0xe5, 0x81];

    fn standard_pack() -> Vec<u8> {
        let mut pack = vec![0xf2, 25, 2];

        let mut basic_id = [0u8; 25];
        basic_id[0] = 0x02;
        basic_id[1] = 0x12;
        basic_id[2..22].copy_from_slice(b"1581F9DEC261802966XD");
        pack.extend_from_slice(&basic_id);

        let mut location = [0u8; 25];
        location[0] = 0x12;
        location[1] = 0x20;
        location[5..9].copy_from_slice(&407_608_000_i32.to_le_bytes());
        location[9..13].copy_from_slice(&(-953_702_000_i32).to_le_bytes());
        location[15..17].copy_from_slice(&2200_u16.to_le_bytes());
        pack.extend_from_slice(&location);
        pack
    }

    fn management_frame(frame_control: u8, oui: [u8; 3], oui_type: u8, framed: &[u8]) -> Vec<u8> {
        let vendor_len = 3 + 1 + framed.len();
        let mut frame = vec![0u8; MIN_802_11_MGMT_LEN];
        frame[0] = frame_control;
        frame[TRANSMITTER_MAC_OFFSET..TRANSMITTER_MAC_OFFSET + 6].copy_from_slice(&MAC);
        frame.extend_from_slice(&[VENDOR_SPECIFIC_IE, vendor_len as u8]);
        frame.extend_from_slice(&oui);
        frame.push(oui_type);
        frame.extend_from_slice(framed);
        frame
    }

    fn standard_management_frame(frame_control: u8, counter: u8) -> Vec<u8> {
        let mut framed = vec![counter];
        framed.extend_from_slice(&standard_pack());
        management_frame(frame_control, FA_OUI, ODID_OUI_TYPE, &framed)
    }

    fn standard_beacon(counter: u8) -> Vec<u8> {
        standard_management_frame(FC_BEACON, counter)
    }

    #[test]
    fn standard_beacon_accepts_every_message_counter_value() {
        let rejected: Vec<u8> = (0u8..=u8::MAX)
            .filter(|&counter| ingest_frame(&standard_beacon(counter), -52).is_none())
            .collect();
        assert!(
            rejected.is_empty(),
            "standard Beacon counters rejected: {rejected:02x?}"
        );
    }

    #[test]
    fn standard_beacon_sets_transport_and_counter_on_wire() {
        let data = ingest_frame(&standard_beacon(0x7a), -52)
            .expect("standard Beacon must reach its metadata assertions");
        assert_eq!(data.transport, Some(RidTransport::WifiBeacon));
        assert_eq!(data.message_counter, Some(0x7a));

        let wire = serde_json::to_value(data).expect("telemetry serializes");
        assert_eq!(wire["transport"], "wifi_beacon");
        assert_eq!(wire["message_counter"], 0x7a);
    }

    #[test]
    fn standard_probe_response_peels_counter() {
        let data = ingest_frame(&standard_management_frame(FC_PROBE_RESPONSE, 0xf7), -48)
            .expect("standard Probe Response must decode");
        assert_eq!(data.transport, Some(RidTransport::WifiBeacon));
        assert_eq!(data.message_counter, Some(0xf7));
    }

    #[test]
    fn standard_beacon_rejects_malformed_framing() {
        let mut framed = vec![0x33];
        framed.extend_from_slice(&standard_pack());

        assert!(ingest_frame(
            &management_frame(FC_BEACON, [0x00, 0x11, 0x22], ODID_OUI_TYPE, &framed),
            -52,
        )
        .is_none());

        let mut truncated_ie = standard_beacon(0x33);
        truncated_ie.pop();
        assert!(ingest_frame(&truncated_ie, -52).is_none());

        let mut bad_size = framed.clone();
        bad_size[2] = 24;
        assert!(ingest_frame(
            &management_frame(FC_BEACON, FA_OUI, ODID_OUI_TYPE, &bad_size),
            -52,
        )
        .is_none());

        let mut zero_count = framed.clone();
        zero_count[3] = 0;
        assert!(ingest_frame(
            &management_frame(FC_BEACON, FA_OUI, ODID_OUI_TYPE, &zero_count),
            -52,
        )
        .is_none());

        let mut excessive_count = framed.clone();
        excessive_count[3] = 10;
        assert!(ingest_frame(
            &management_frame(FC_BEACON, FA_OUI, ODID_OUI_TYPE, &excessive_count),
            -52,
        )
        .is_none());

        let mut inconsistent_length = framed;
        inconsistent_length.push(0);
        assert!(ingest_frame(
            &management_frame(FC_BEACON, FA_OUI, ODID_OUI_TYPE, &inconsistent_length),
            -52,
        )
        .is_none());
    }

    #[test]
    fn counterless_vendor_payload_is_rejected() {
        let frame = management_frame(FC_BEACON, FA_OUI, ODID_OUI_TYPE, &standard_pack());
        assert!(ingest_frame(&frame, -52).is_none());
    }

    #[test]
    fn standard_beacon_skips_bad_odid_ie() {
        let mut framed = vec![0x21];
        framed.extend(standard_pack());
        let mut bad_header = framed.clone();
        bad_header[1] = 0x12;
        let mut bad_size = framed.clone();
        bad_size[2] = 24;
        let mut bad_count = framed.clone();
        bad_count[3] = 0;
        let mut too_many = framed.clone();
        too_many[3] = 10;
        let mut trailing = framed;
        trailing.push(0);
        let malformed = [
            vec![], vec![0x21], vec![0x21, 0xf2, 25], standard_pack(),
            bad_header, bad_size, bad_count, too_many, trailing,
        ];
        for (case, bad) in malformed.iter().enumerate() {
            let mut frame = management_frame(FC_BEACON, FA_OUI, ODID_OUI_TYPE, bad);
            assert!(ingest_frame(&frame, -52).is_none());
            frame.extend_from_slice(&standard_beacon(0xf7)[MIN_802_11_MGMT_LEN..]);
            let result = ingest_frame(&frame, -52);
            assert!(result.is_some(), "malformed ODID IE case {case} hid a later valid IE");
            let data = result.unwrap();
            assert_eq!(data.message_counter, Some(0xf7));
            assert_eq!(data.transport, Some(RidTransport::WifiBeacon));
            assert_eq!(data.drone_id, "1581F9DEC261802966XD");
        }
    }

    #[test]
    fn standard_beacon_deframer_is_panic_free_for_arbitrary_bytes() {
        let mut state = 0x6d5a_56e9_u32;
        for len in 0..=512 {
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                bytes.push(state as u8);
            }
            let _ = ingest_frame(&bytes, -127);
        }
    }

    #[test]
    fn real_dji_fields_are_unchanged_except_transport_metadata() {
        // Counter + Message Pack captured from a DJI Mini 5 Pro by
        // test-node-2 on 2026-05-07.
        let captured: [u8; 79] = [
            0x01, 0xf2, 0x19, 0x03, 0x02, 0x12, b'1', b'5', b'8', b'1', b'F', b'9', b'D', b'E',
            b'C', b'2', b'6', b'1', b'8', b'0', b'2', b'9', b'6', b'6', b'X', b'D', 0x00, 0x00,
            0x00, 0x12, 0x16, 0xb5, 0x00, 0x00, 0x12, 0x30, 0x5e, 0x18, 0x6a, 0xba, 0xf6, 0xc6,
            0x00, 0x00, 0x46, 0x0a, 0xd0, 0x07, 0x2c, 0x04, 0xa6, 0x48, 0x0a, 0x00, 0x42, 0x01,
            0x26, 0x31, 0x5e, 0x18, 0xdb, 0xb9, 0xf6, 0xc6, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x4b, 0x0a, 0xe3, 0x2c, 0xd2, 0x0d, 0x00,
        ];
        let before = astm::parse_message_pack(&captured, MAC, -52)
            .expect("compatibility baseline must decode");
        let frame = management_frame(FC_BEACON, FA_OUI, ODID_OUI_TYPE, &captured);
        let after = ingest_frame(&frame, -52).expect("strict Wi-Fi path must decode");

        assert_eq!(after.transport, Some(RidTransport::WifiBeacon));
        assert_eq!(after.message_counter, Some(0x01));
        let mut before = serde_json::to_value(before).expect("baseline serializes");
        let mut after = serde_json::to_value(after).expect("strict result serializes");
        before.as_object_mut().unwrap().remove("transport");
        before.as_object_mut().unwrap().remove("message_counter");
        after.as_object_mut().unwrap().remove("transport");
        after.as_object_mut().unwrap().remove("message_counter");
        assert_eq!(after, before, "decoded DJI fields changed");
    }

    #[test]
    fn captured_mavic3t_f0_beacon_decodes_byte_exact() {
        // Full 802.11 frame 16156 from the real Mavic 3T field capture whose
        // SHA-256 is pinned by Component/aviary/validation.pcap.sha256. Its
        // 0xF0 counter is one of the values rejected by the former boundary.
        const FRAME: &[u8] = &[
            0x80, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x60, 0x60, 0x1f, 0x34,
            0x49, 0x5d, 0x60, 0x60, 0x1f, 0x34, 0x49, 0x5d, 0x00, 0x00, 0x90, 0xa3, 0xf7, 0x60,
            0x00, 0x00, 0x00, 0x00, 0xa0, 0x00, 0x20, 0x04, 0x00, 0x18, 0x52, 0x49, 0x44, 0x2d,
            0x31, 0x35, 0x38, 0x31, 0x46, 0x42, 0x45, 0x58, 0x43, 0x32, 0x35, 0x41, 0x4d, 0x30,
            0x30, 0x44, 0x38, 0x38, 0x56, 0x4a, 0xdd, 0x53, 0xfa, 0x0b, 0xbc, 0x0d, 0xf0, 0xf2,
            0x19, 0x03, 0x02, 0x12, 0x31, 0x35, 0x38, 0x31, 0x46, 0x42, 0x45, 0x58, 0x43, 0x32,
            0x35, 0x41, 0x4d, 0x30, 0x30, 0x44, 0x38, 0x38, 0x56, 0x4a, 0x00, 0x00, 0x00, 0x12,
            0x22, 0xb5, 0x00, 0x00, 0xc6, 0x2f, 0x5e, 0x18, 0xd1, 0xb9, 0xf6, 0xc6, 0x00, 0x00,
            0x31, 0x0b, 0xbc, 0x08, 0x3a, 0x02, 0xe4, 0x7f, 0x0a, 0x00, 0x42, 0x01, 0x2b, 0x31,
            0x5e, 0x18, 0xa3, 0xb8, 0xf6, 0xc6, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x55, 0x0a, 0x5a, 0x13, 0xa7, 0x0d, 0x00, 0xc7, 0xff, 0xb9, 0x9f,
        ];

        let data = ingest_frame(FRAME, -60).expect("captured Mavic 3T Beacon must decode");
        assert_eq!(data.mac_address, [0x60, 0x60, 0x1f, 0x34, 0x49, 0x5d]);
        assert_eq!(data.drone_id, "1581FBEXC25AM00D88VJ");
        assert_eq!(data.transport, Some(RidTransport::WifiBeacon));
        assert_eq!(data.message_counter, Some(0xf0));
        assert!(data.pos.is_some());
        assert!(data.operator_pos.is_some());
    }
}
