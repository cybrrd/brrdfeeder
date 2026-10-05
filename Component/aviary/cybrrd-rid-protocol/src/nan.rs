// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Bounded ODID NAN SDF admission (BWF0010/BWF0030/BWF0040/BWF0060/BWF0100).
//! Sync beacons carry cluster metadata, never Remote ID observations.

use crate::{
    astm,
    models::{RidTransport, TelemetryData},
};

const SDF_HEADER: [u8; 6] = [0x04, 0x09, 0x50, 0x6f, 0x9a, 0x13];
const ODID_SERVICE_ID: [u8; 6] = [0x88, 0x69, 0x19, 0x9d, 0x92, 0x09];

pub(crate) fn ingest_sdf(frame: &[u8], rssi_dbm: i32) -> Option<TelemetryData> {
    // Only complete, unprotected management Action frames with the base header.
    // Retry/power-management/more-data flags do not change the payload layout.
    if frame.first() != Some(&0xd0)
        || frame.get(1)? & 0xc7 != 0
        || frame.get(22)? & 0x0f != 0
        || frame.get(24..30)? != SDF_HEADER
    {
        return None;
    }
    let mut attrs = frame.get(30..)?;
    let mut selected = None;
    while !attrs.is_empty() {
        let header = attrs.get(..3)?;
        let len = u16::from_le_bytes([header[1], header[2]]) as usize;
        let body = attrs.get(3..3 + len)?;
        attrs = attrs.get(3 + len..)?;
        if header[0] != 0x03 {
            continue;
        }
        if body.get(..6)? != ODID_SERVICE_ID {
            continue;
        }
        // Standard ODID publish with service info present. Other control layouts
        // (optional filters, subscribe/follow-up, reserved bits) are not ODID SDFs.
        if *body.get(8)? != 0x10 {
            return None;
        }
        let info_len = *body.get(9)? as usize;
        if body.len() != 10 + info_len || selected.is_some() {
            return None;
        }
        selected = Some(body.get(10..)?);
    }
    // Walk the entire attribute sequence before decoding, so malformed trailers
    // cannot be hidden behind a valid descriptor. Duplicate ODID descriptors are
    // ambiguous for this one-observation API and fail closed.
    let source: [u8; 6] = frame.get(10..16)?.try_into().ok()?;
    let mut data =
        astm::parse_wifi_service_info(selected?, source, rssi_dbm, RidTransport::WifiNan)?;
    let bssid = frame.get(16..22)?;
    data.wifi_bssid = Some(format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bssid[0], bssid[1], bssid[2], bssid[3], bssid[4], bssid[5],
    ));
    Some(data)
}
