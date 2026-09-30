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
