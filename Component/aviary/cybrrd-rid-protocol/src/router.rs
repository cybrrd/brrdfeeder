// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! 802.11 frame walker → consolidated ASTM parser.
//!
//! Single runtime entry point. Identifies whether a captured frame carries
//! an ASTM Open Drone ID Vendor Specific IE; if so, delegates to the
//! canonical `astm::parse_message_pack` for field extraction.

use crate::astm;
use crate::models::TelemetryData;

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

/// Process a raw 802.11 management frame payload (Beacon or Probe Response)
/// and emit per-frame telemetry data if it carries an ASTM Open Drone ID
/// Message Pack.
///
/// Returns `None` for any frame that isn't a viable beacon/probe-response,
/// doesn't carry the Wi-Fi Alliance ODID OUI, or has a malformed Message Pack.
pub fn ingest_frame(dot11_payload: &[u8], rssi_dbm: i32) -> Option<TelemetryData> {
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
                // Skip OUI(3) + OUI-type(1) = 4 bytes; rest is the Message Pack.
                let pack_start = offset + IE_HEADER_LEN + MIN_VENDOR_IE_LEN;
                let pack = &dot11_payload[pack_start..ie_end];
                return astm::parse_message_pack(pack, mac_address, rssi_dbm);
            }
        }
        offset = ie_end;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RidTransport;

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

    fn standard_beacon(counter: u8) -> Vec<u8> {
        let pack = standard_pack();
        let vendor_len = 3 + 1 + 1 + pack.len();
        let mut frame = vec![0u8; MIN_802_11_MGMT_LEN];
        frame[0] = FC_BEACON;
        frame[TRANSMITTER_MAC_OFFSET..TRANSMITTER_MAC_OFFSET + 6].copy_from_slice(&MAC);
        frame.extend_from_slice(&[VENDOR_SPECIFIC_IE, vendor_len as u8]);
        frame.extend_from_slice(&FA_OUI);
        frame.push(ODID_OUI_TYPE);
        frame.push(counter);
        frame.extend_from_slice(&pack);
        frame
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
}
