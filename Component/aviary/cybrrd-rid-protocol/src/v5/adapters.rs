// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Explicit development adapter profiles. Unsupported formats remain exact
//! ledger events; HCI is never called a full Bluetooth packet capture.
use super::{
    hex,
    producer::{self, Capture, RawLedger, Retained, UnitSpan},
    schema, Error, Result,
};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug)]
pub enum Profile {
    HciSingleReport,
    WifiRadiotapBasic,
}
/// H4 packet indicator (1), HCI event header (2), u8 parameter length (255).
pub const MAX_HCI_CAPTURE_BYTES: usize = 1 + 2 + 255;
/// Local allowance for this basic radiotap profile, including alignment/padding.
/// The supported bits 0..17 need at most 44 bytes; this is not a general
/// radiotap protocol limit.
pub const MAX_RADIOTAP_HEADER_BYTES: usize = 64;
/// Adopted IEEE 802.11 management-frame body ceiling, in octets.
pub const MAX_WIFI_MANAGEMENT_BODY_BYTES: usize = 2304;
/// Basic radiotap allowance + management MAC header + body + FCS.
pub const MAX_WIFI_CAPTURE_BYTES: usize =
    MAX_RADIOTAP_HEADER_BYTES + 24 + MAX_WIFI_MANAGEMENT_BODY_BYTES + 4;

impl Profile {
    pub const fn max_capture_bytes(self) -> usize {
        match self {
            Self::HciSingleReport => MAX_HCI_CAPTURE_BYTES,
            Self::WifiRadiotapBasic => MAX_WIFI_CAPTURE_BYTES,
        }
    }
}

pub fn capture(ledger: &mut impl RawLedger, mut c: Capture, profile: Profile) -> Result<Retained> {
    let name = match profile {
        Profile::HciSingleReport => "hci-single-report-v1",
        Profile::WifiRadiotapBasic => "radiotap-basic-v1",
    };
    c.event.metadata = json!({"source_metadata":c.event.metadata,"input_format_label":c.format,"adapter_profile":name});
    c.format = name.into();
    producer::capture_with(ledger, c, |c| {
        // Retain the event first, then refuse oversize input before any parser
        // or walker copy. Ledger admission quotas remain a separate contract.
        if c.event.bytes.len() > profile.max_capture_bytes() {
            return Err(Error(format!(
                "{name} frame exceeds {}-byte input cap; exact event retained",
                profile.max_capture_bytes()
            )));
        }
        match profile {
            Profile::HciSingleReport => hci(c),
            Profile::WifiRadiotapBasic => wifi(c),
        }
    })
}
fn slice(b: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    b.get(
        start
            ..start
                .checked_add(len)
                .ok_or_else(|| Error("adapter offset overflow".into()))?,
    )
    .ok_or_else(|| Error("truncated adapter buffer".into()))
}
fn evidence(c: &mut Capture, receiver: bool, name: &str, raw: Value, decoded: Value) -> Result<()> {
    let s = schema::contract()?;
    let fields = if receiver {
        &s["properties"]["receiver"]["properties"]
    } else {
        &s["properties"]["captures"]["items"]["properties"]["transport_fields"]["properties"]
            [&c.transport_kind]["properties"]
    };
    let field = fields
        .get(name)
        .ok_or_else(|| Error(format!("unknown adapter field {name}")))?;
    let cell = json!({"raw":raw,"decoded":decoded,"status":"known","unit":field["properties"]["unit"]["const"],"resolution":field["properties"]["resolution"]["const"],"reason":null});
    if receiver {
        if let Some(supplied) = c.receiver_cells.get(name) {
            for member in ["raw", "decoded", "unit", "resolution"] {
                if supplied[member] != cell[member] {
                    return Err(Error(format!(
                        "conflicting adapter metadata {name}.{member}; exact event retained"
                    )));
                }
            }
        } else {
            c.receiver_cells.insert(name.into(), cell);
        }
    } else {
        let occurrences = c
            .transport_cells
            .entry("extensions")
            .or_insert_with(|| json!({"field_occurrences":[]}));
        if let Some(a) = occurrences["field_occurrences"].as_array_mut() {
            a.push(json!({"field":name,"cell":cell}));
        }
        if let Some(previous) = c.transport_cells.get_mut(name) {
            previous["status"] = json!("ambiguous");
            previous["reason"] =
                json!("multiple ordered field occurrences retained in transport extensions");
        } else {
            c.transport_cells.insert(name.into(), cell);
        }
    }
    Ok(())
}
fn same(c: &mut Capture, receiver: bool, name: &str, v: Value) -> Result<()> {
    evidence(c, receiver, name, v.clone(), v)
}
fn transport_bytes(c: &mut Capture, name: &str, b: &[u8]) -> Result<()> {
    same(c, false, name, json!(hex(b)))
}
fn service(
    c: &mut Capture,
    start: usize,
    end: usize,
    is_pack: bool,
    payload_name: &str,
) -> Result<()> {
    let framed = slice(
        &c.event.bytes,
        start,
        end.checked_sub(start)
            .ok_or_else(|| Error("service interval".into()))?,
    )?;
    let Some(&counter) = framed.first() else {
        return Err(Error("service counter truncated".into()));
    };
    let length = framed.len() - 1;
    let payload = json!(hex(&framed[1..]));
    c.spans.push(UnitSpan {
        offset: start + 1,
        length,
        is_pack,
        counter: Some(counter),
    });
    // A single transport field cannot represent multiple unequal instances.
    // Keep all instance values in the ordered unit list, and mark the summary
    // cell ambiguous instead of choosing one. Raw frame retains all instances.
    for (name, v) in [("message_counter", json!(counter)), (payload_name, payload)] {
        same(c, false, name, v)?;
    }
    Ok(())
}
fn hci(c: &mut Capture) -> Result<()> {
    if !c.transport_cells.is_empty() {
        return Err(Error(
            "raw HCI profile received preinterpreted transport cells; input retained".into(),
        ));
    }
    let b = c.event.bytes.clone();
    let prefix = slice(&b, 0, 5)?;
    if prefix[..2] != [4, 0x3e] || usize::from(prefix[2]) + 3 != b.len() {
        return Err(Error("unsupported or malformed HCI event".into()));
    }
    if prefix[4] != 1 {
        return Err(Error(
            "HCI profile requires one report; complete batch retained".into(),
        ));
    }
    let extended = match prefix[3] {
        2 => false,
        13 => true,
        _ => return Err(Error("unsupported HCI subevent".into())),
    };
    let header_len = if extended { 24 } else { 9 };
    let h = slice(&b, 5, header_len)?;
    let legacy = !extended || h[0] & 0x10 != 0;
    let length = usize::from(h[header_len - 1]);
    let start = 5 + header_len;
    let end = start + length;
    slice(&b, start, length)?;
    if b.len() != end + usize::from(!extended) {
        return Err(Error("HCI length mismatch".into()));
    }
    let (address, rssi) = if extended {
        (&h[3..9], h[13])
    } else {
        (&h[2..8], slice(&b, end, 1)?[0])
    };
    c.transport_kind = if legacy {
        "bt4_legacy"
    } else {
        "bt5_secondary"
    }
    .into();
    c.spans.clear();
    c.transport_cells.clear();
    c.completeness = "full_at_adapter_boundary".into();
    c.unavailable_reason =
        Some("HCI report boundary; packet-only PHY fields are not exposed".into());
    let mut logical = address.to_vec();
    logical.reverse();
    evidence(
        c,
        true,
        "source_MAC",
        json!(hex(address)),
        json!(hex(&logical)),
    )?;
    if rssi != 127 {
        same(c, true, "RSSI_dbm", json!((rssi as i8).to_string()))?;
    } else {
        same(c, true, "RSSI_dbm", json!("127"))?;
        if let Some(v) = c.receiver_cells.get_mut("RSSI_dbm") {
            v["status"] = json!("unknown");
            v["reason"] = json!("HCI RSSI unavailable sentinel");
        }
    }
    evidence(
        c,
        false,
        if legacy { "ad_addr" } else { "AdvA" },
        json!(hex(address)),
        json!(hex(&logical)),
    )?;
    same(
        c,
        true,
        "PHY_data_rate",
        json!({"phy":if extended{format!("HCI_primary_{}_secondary_{}",h[9],h[10])}else{"HCI_LE_1M".into()},"bitrate_bps":null,"mcs":null,"coding":null}),
    )?;
    // Full HCI header remains in raw capture; it cannot stand in for a PDU header,
    // access address, CRC, ADI or AuxPtr which HCI did not expose.
    let mut cursor = start;
    while cursor < end {
        let len = usize::from(b[cursor]);
        if len == 0 {
            break;
        }
        let ad_end = cursor + 1 + len;
        if ad_end > end {
            return Err(Error("truncated advertising structure".into()));
        }
        let ad = &b[cursor + 1..ad_end];
        if ad.starts_with(&[0x16, 0xfa, 0xff, 0x0d]) {
            transport_bytes(c, "ad_info", &b[cursor..cursor + 4])?;
            same(c, false, "app_code", json!(0x0d))?;
            service(
                c,
                cursor + 5,
                ad_end,
                !legacy,
                if legacy {
                    "odid_message"
                } else {
                    "odid_message_pack"
                },
            )?;
        }
        cursor = ad_end;
    }
    if extended {
        let flags = u16::from_le_bytes([h[0], h[1]]);
        if flags & 0x60 != 0 {
            return Err(Error(
                "fragmented HCI data retained; no cross-report assembly in this profile".into(),
            ));
        }
    }
    Ok(())
}
fn wifi(c: &mut Capture) -> Result<()> {
    if !c.transport_cells.is_empty() {
        return Err(Error(
            "raw Wi-Fi profile received preinterpreted transport cells; input retained".into(),
        ));
    }
    let b = c.event.bytes.clone();
    let h = slice(&b, 0, 8)?;
    let rtlen = usize::from(u16::from_le_bytes([h[2], h[3]]));
    if h[0] != 0 || rtlen < 8 {
        return Err(Error("unsupported radiotap header".into()));
    }
    if rtlen > MAX_RADIOTAP_HEADER_BYTES {
        return Err(Error(
            "radiotap header allowance exceeded; exact frame retained".into(),
        ));
    }
    slice(&b, 0, rtlen)?;
    let present = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
    if present & !0x0003ffff != 0 {
        return Err(Error(
            "radiotap field outside basic profile; exact frame retained".into(),
        ));
    }
    // Standard field sizes/alignments for bits 0..17. All spans remain in raw.
    let layouts = [
        (8, 8),
        (1, 1),
        (1, 1),
        (2, 4),
        (2, 2),
        (1, 1),
        (1, 1),
        (2, 2),
        (2, 2),
        (2, 2),
        (1, 1),
        (1, 1),
        (1, 1),
        (1, 1),
        (2, 2),
        (2, 2),
        (1, 1),
        (1, 1),
    ];
    let mut cursor = 8;
    let mut flags = None;
    c.spans.clear();
    c.transport_cells.clear();
    for (bit, (align, size)) in layouts.iter().copied().enumerate() {
        if present & (1 << bit) == 0 {
            continue;
        }
        cursor = (cursor + align - 1) & !(align - 1);
        if cursor + size > rtlen {
            return Err(Error("radiotap field exceeds header".into()));
        }
        let f = slice(&b, cursor, size)?;
        cursor += size;
        match bit {
            0 => {
                let n = u64::from_le_bytes([f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]]);
                same(
                    c,
                    true,
                    "radio_timestamp_and_clock_domain",
                    json!({"ticks":n.to_string(),"unit":"microsecond","domain":"radiotap_tsft","wrap_bits":64}),
                )?;
            }
            1 => flags = Some(f[0]),
            2 => same(
                c,
                true,
                "PHY_data_rate",
                json!({"phy":"radiotap_legacy_rate","bitrate_bps":(u64::from(f[0])*500000).to_string(),"mcs":null,"coding":null}),
            )?,
            3 => {
                let frequency = u16::from_le_bytes([f[0], f[1]]);
                same(
                    c,
                    true,
                    "wifi_rx_channel_and_source",
                    json!({"channel":null,"frequency_mhz":frequency.to_string(),"source":"radiotap"}),
                )?;
            }
            5 => same(c, true, "RSSI_dbm", json!((f[0] as i8).to_string()))?,
            6 => same(c, true, "noise_floor", json!((f[0] as i8).to_string()))?,
            11 => same(c, true, "antenna_index", json!(f[0]))?,
            _ => {}
        }
    }
    let fcs = flags.is_some_and(|f| f & 0x10 != 0);
    let end = b
        .len()
        .checked_sub(if fcs { 4 } else { 0 })
        .ok_or_else(|| Error("FCS length".into()))?;
    if end < rtlen {
        return Err(Error("missing MAC frame".into()));
    }
    let frame = slice(&b, rtlen, end - rtlen)?;
    if frame.len() > 24 + MAX_WIFI_MANAGEMENT_BODY_BYTES {
        return Err(Error(
            "management body limit exceeded; exact frame retained".into(),
        ));
    }
    let mh = slice(frame, 0, 24)?;
    same(c, true, "source_MAC", json!(hex(&mh[10..16])))?;
    same(
        c,
        true,
        "frame_length",
        json!({"captured_bytes":b.len(),"original_bytes":c.original_length}),
    )?;
    if let Some(f) = flags {
        same(
            c,
            true,
            "FCS_status",
            json!(if f & 0x40 != 0 {
                "verified_bad"
            } else if fcs {
                "not_checked"
            } else {
                "not_present"
            }),
        )?;
    }
    if mh[1] & 0x44 != 0 || mh[22] & 15 != 0 {
        return Err(Error(
            "protected/fragmented MAC frame retained without guessed decode".into(),
        ));
    }
    match mh[0] {
        0x80 | 0x50 => {
            slice(frame, 0, 36)?;
            c.transport_kind = "wifi_beacon".into();
            let mut cursor = 36;
            while cursor < frame.len() {
                let ie = slice(frame, cursor, 2)?;
                let len = usize::from(ie[1]);
                let body = slice(frame, cursor + 2, len)?;
                if ie[0] == 0xdd && body.starts_with(&[0xfa, 0x0b, 0xbc, 0x0d]) {
                    same(c, false, "element_id", json!(ie[0]))?;
                    same(c, false, "length", json!(ie[1]))?;
                    transport_bytes(c, "oui", &body[..3])?;
                    same(c, false, "vendor_type", json!(body[3]))?;
                    service(
                        c,
                        rtlen + cursor + 6,
                        rtlen + cursor + 2 + len,
                        true,
                        "message_pack",
                    )?;
                } else if ie[0] == 0xdd && body.starts_with(&[0x50, 0x6f, 0x9a, 0x13]) {
                    if !c.spans.is_empty() {
                        return Err(Error("mixed sync and RID beacon layout retained".into()));
                    }
                    c.transport_kind = "wifi_nan_sync".into();
                    same(c, false, "element_id", json!(ie[0]))?;
                    same(c, false, "length", json!(ie[1]))?;
                    transport_bytes(c, "oui", &body[..3])?;
                    same(c, false, "oui_type", json!(body[3]))?;
                    let mut a = 4;
                    while a < body.len() {
                        let head = slice(body, a, 3)?;
                        let n = usize::from(u16::from_le_bytes([head[1], head[2]]));
                        let all = slice(body, a, 3 + n)?;
                        if head[0] == 0 {
                            transport_bytes(c, "master_indication_attr", all)?;
                        } else if head[0] == 1 {
                            transport_bytes(c, "cluster_attr", all)?;
                        }
                        a += 3 + n;
                    }
                }
                cursor += 2 + len;
            }
        }
        0xd0 => {
            if slice(frame, 24, 6)? != [4, 9, 0x50, 0x6f, 0x9a, 0x13] {
                return Err(Error("unsupported action frame retained".into()));
            }
            c.transport_kind = "wifi_nan_sdf".into();
            transport_bytes(c, "oui", &frame[26..29])?;
            same(c, false, "oui_type", json!(frame[29]))?;
            transport_bytes(c, "addr3_bssid", &frame[16..22])?;
            let mut cursor = 30;
            while cursor < frame.len() {
                let head = slice(frame, cursor, 3)?;
                let n = usize::from(u16::from_le_bytes([head[1], head[2]]));
                let body = slice(frame, cursor + 3, n)?;
                if head[0] == 3 && body.starts_with(&[0x88, 0x69, 0x19, 0x9d, 0x92, 0x09]) {
                    let fixed = slice(body, 0, 10)?;
                    if fixed[8] != 0x10 {
                        return Err(Error("unsupported NAN descriptor control retained".into()));
                    }
                    if usize::from(fixed[9]) + 10 != body.len() {
                        return Err(Error("NAN service length mismatch".into()));
                    }
                    transport_bytes(c, "service_descriptor_attr", slice(frame, cursor, 3 + n)?)?;
                    service(
                        c,
                        rtlen + cursor + 13,
                        rtlen + cursor + 3 + n,
                        true,
                        "service_info_message_pack",
                    )?;
                } else if head[0] == 0x0e {
                    transport_bytes(
                        c,
                        "sdea_instance_control_update",
                        slice(frame, cursor, 3 + n)?,
                    )?;
                }
                cursor += 3 + n;
            }
        }
        _ => return Err(Error("unsupported MAC frame type retained".into())),
    }
    Ok(())
}
