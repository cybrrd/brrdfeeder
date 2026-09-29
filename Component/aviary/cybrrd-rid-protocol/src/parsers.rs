//! Higher-level beacon/action-frame entry points.
//!
//! Both routes converge on the consolidated `astm::parse_message_pack`. The
//! prior `stub_telemetry` path that returned bogus zero-filled data for
//! recognized-but-unimplemented OUIs was deliberately removed (Wave 6.0a):
//! emitting "telemetry" with all-zero coordinates pollutes downstream
//! pipelines with phantom drones at (0°, 0°). Returning `None` is
//! substrate-truthful — we only emit when we actually parsed real data.

use crate::astm;
use crate::models::TelemetryData;

// Wi-Fi Alliance OUI for ASTM Open Drone ID (used in beacon-format ODID).
const WFA_ODID_OUI: [u8; 3] = [0x50, 0x6F, 0x9A];
const WFA_ODID_SUBTYPE: u8 = 0x13;

// DJI Legacy OUI (pre-Sept-2023 DJI drones do not faithfully implement
// ASTM F3411-22a — parsing those frames requires reverse-engineered
// per-model handling that is intentionally out of scope for Wave 6.0a).
const DJI_LEGACY_OUI: [u8; 3] = [0x2A, 0x1A, 0x05];

const VENDOR_SPECIFIC_IE: u8 = 0xDD;
const MIN_802_11_MGMT_LEN: usize = 36;
const TRANSMITTER_MAC_OFFSET: usize = 10;

/// Decode a captured 802.11 Beacon / Probe Response.
///
/// Returns `Some(TelemetryData)` when an ASTM Open Drone ID payload is
/// present AND the parser extracted at least a drone position. Returns
/// `None` for unrecognized OUIs (including DJI legacy frames, which are
/// recognized but parsing is deferred).
pub fn decode_beacon(payload: &[u8], rssi: i32) -> Option<TelemetryData> {
    if payload.len() < MIN_802_11_MGMT_LEN {
        return None;
    }

    // Transmitter MAC for the consolidated parser.
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&payload[TRANSMITTER_MAC_OFFSET..TRANSMITTER_MAC_OFFSET + 6]);

    let mut offset = MIN_802_11_MGMT_LEN;
    while offset + 2 <= payload.len() {
        let tag = payload[offset];
        let len = payload[offset + 1] as usize;
        let data_start = offset + 2;
        let data_end = data_start + len;
        if data_end > payload.len() {
            break;
        }

        if tag == VENDOR_SPECIFIC_IE && len >= 3 {
            let ie_data = &payload[data_start..data_end];
            let oui = &ie_data[..3];

            if oui == WFA_ODID_OUI && ie_data.len() > 3 && ie_data[3] == WFA_ODID_SUBTYPE {
                return astm::parse_message_pack(&ie_data[4..], mac, rssi);
            }
            if oui == DJI_LEGACY_OUI {
                // DJI legacy parsing is a substrate-truth gap (DJI's pre-2023
                // frames don't follow the standard); deferred. Drop silently
                // rather than emit bogus data.
                return None;
            }
        }
        offset = data_end;
    }
    None
}

/// Modern ASTM Neighbor Awareness Networking Action frames carry the
/// same ODID Message Pack format. Currently a stub awaiting NAN
/// frame-format substrate-truth in real captures.
pub fn decode_nan_action(_payload: &[u8], _rssi: i32) -> Option<TelemetryData> {
    None
}
