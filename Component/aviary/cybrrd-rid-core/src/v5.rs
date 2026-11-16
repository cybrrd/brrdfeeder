// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Additive, allocation-free observation decoding. No legacy projection or dedup.
//! Borrowed bytes remain authoritative even when interpretation is unavailable.

use heapless::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value<'a> {
    Null,
    Integer(i64),
    Decimal { coefficient: i64, scale: u8 },
    Bytes(&'a [u8]),
    Text(&'a str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell<'a> {
    pub raw: Value<'a>,
    pub decoded: Value<'a>,
    pub status: &'static str,
    pub unit: &'static str,
    pub resolution: &'static str,
    pub reason: Option<&'static str>,
    pub arithmetic_value: Option<Value<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field<'a> {
    pub name: &'static str,
    pub cell: Cell<'a>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message<'a> {
    pub raw: &'a [u8],
    pub message_type: Option<Cell<'a>>,
    pub protocol_version: Option<Cell<'a>>,
    pub parse_status: &'static str,
    pub body_name: Option<&'static str>,
    pub fields: Vec<Field<'a>, 32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacityError;

fn cell<'a>(
    raw: Value<'a>,
    decoded: Value<'a>,
    status: &'static str,
    unit: &'static str,
    resolution: &'static str,
) -> Cell<'a> {
    Cell {
        raw,
        decoded,
        status,
        unit,
        resolution,
        reason: None,
        arithmetic_value: None,
    }
}
pub fn code(raw: u8, status: &'static str) -> Cell<'static> {
    cell(
        Value::Integer(raw.into()),
        Value::Integer(raw.into()),
        status,
        "code",
        "1",
    )
}
fn number(
    raw: i64,
    coefficient: i64,
    scale: u8,
    status: &'static str,
    unit: &'static str,
    resolution: &'static str,
) -> Cell<'static> {
    cell(
        Value::Integer(raw),
        Value::Decimal { coefficient, scale },
        status,
        unit,
        resolution,
    )
}
fn bytes<'a>(raw: &'a [u8], status: &'static str) -> Cell<'a> {
    cell(Value::Bytes(raw), Value::Bytes(raw), status, "byte", "1")
}
fn na(unit: &'static str, resolution: &'static str) -> Cell<'static> {
    let mut c = cell(Value::Null, Value::Null, "not_applicable", unit, resolution);
    c.reason = Some("opposite_authentication_page_shape");
    c
}
fn enum_status(raw: u8, last: u8, zero_unknown: bool) -> &'static str {
    if raw == 0 && zero_unknown {
        "unknown"
    } else if raw <= last {
        "known"
    } else {
        "reserved"
    }
}
fn byte_enum(raw: u8, last: u8) -> &'static str {
    if raw <= last {
        "known"
    } else if raw <= 200 {
        "reserved"
    } else {
        "private_use"
    }
}
pub fn version(raw: u8) -> Cell<'static> {
    code(
        raw,
        match raw {
            2 => "known",
            15 => "private_use",
            _ => "unsupported_version",
        },
    )
}
pub fn altitude(raw: u16, unit: &'static str) -> Cell<'static> {
    let mut c = number(
        raw.into(),
        i64::from(raw) * 5 - 10000,
        1,
        match raw {
            0 => "unknown",
            65535 => "out_of_stated_range",
            _ => "known",
        },
        unit,
        "0.5",
    );
    c.arithmetic_value = Some(c.decoded);
    if raw == 65535 {
        c.reason = Some("arithmetic_exceeds_stated_range");
    }
    c
}
pub fn vertical(raw: i8) -> Cell<'static> {
    number(
        raw.into(),
        i64::from(raw) * 5,
        1,
        match raw {
            126 => "unknown",
            124 => "saturated_lower",
            -124 => "saturated_upper",
            -123..=123 => "known",
            _ => "out_of_range",
        },
        "m/s",
        "0.5",
    )
}
pub fn horizontal(raw: u8, multiplier: bool) -> Cell<'static> {
    let coefficient = if multiplier {
        i64::from(raw) * 75 + 6375
    } else {
        i64::from(raw) * 25
    };
    number(
        raw.into(),
        coefficient,
        2,
        match coefficient {
            25500 => "unknown",
            25425 => "saturated_lower",
            _ => "known",
        },
        "m/s",
        "0.25 or 0.75 by speed_multiplier",
    )
}
pub fn direction(raw: u8, segment: bool) -> Cell<'static> {
    let n = i64::from(raw) + if segment { 180 } else { 0 };
    number(
        raw.into(),
        n,
        0,
        match n {
            0..=359 => "known",
            361 => "unknown",
            _ => "out_of_range",
        },
        "degree_true",
        "1",
    )
}
pub fn location_time(raw: u16) -> Cell<'static> {
    number(
        raw.into(),
        raw.into(),
        1,
        match raw {
            65535 => "unknown",
            0..=36000 => "known",
            _ => "out_of_range",
        },
        "s_since_UTC_hour",
        "0.1",
    )
}
pub fn epoch_time(raw: u32) -> Cell<'static> {
    number(
        raw.into(),
        raw.into(),
        0,
        "known",
        "s_since_2019_01_01",
        "1",
    )
}
/// Widen before addition: every u32 value has an exact u64 Unix representation.
pub const fn unix_seconds(raw: u32) -> u64 {
    raw as u64 + 1_546_300_800
}
pub fn accuracy(raw: u8, last: u8) -> Cell<'static> {
    let mut c = code(raw, enum_status(raw, last, true));
    c.unit = "95_percent_bound_code";
    c
}
fn coordinate(raw: i32, other: i32, latitude: bool) -> Cell<'static> {
    let limit = if latitude { 900_000_000 } else { 1_800_000_000 };
    number(
        raw.into(),
        raw.into(),
        7,
        if raw == 0 && other == 0 {
            "unknown"
        } else if raw < -limit || raw > limit {
            "out_of_range"
        } else {
            "known"
        },
        "degree",
        "0.0000001",
    )
}
fn text<'a>(raw: &'a [u8], empty: &'static str) -> Cell<'a> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let prefix = &raw[..end];
    let mut c = bytes(raw, "known");
    if prefix.iter().any(|b| !(32..=126).contains(b)) {
        c.decoded = Value::Null;
        c.status = "out_of_range";
        c.reason = Some("invalid_ascii_text");
    } else {
        // ASCII checked above; use fallible conversion without unchecked UTF-8.
        match core::str::from_utf8(prefix) {
            Ok(s) => c.decoded = Value::Text(s),
            Err(_) => {
                c.decoded = Value::Null;
                c.status = "out_of_range";
                c.reason = Some("invalid_ascii_text");
                return c;
            }
        }
        if raw[end..].iter().any(|&b| b != 0) {
            c.status = "out_of_range";
            c.reason = Some("nonzero_after_nul");
        } else if prefix.is_empty() {
            c.status = "unknown";
            c.reason = Some(empty);
        } else if prefix.iter().all(|b| b.is_ascii_whitespace()) {
            c.status = "unknown";
            c.reason = Some("whitespace_only_text");
        }
    }
    c
}
fn identifier(raw: &[u8], kind: u8) -> Cell<'_> {
    if kind == 1 || kind == 2 {
        return text(raw, "empty_identifier");
    }
    let empty = raw.iter().all(|&b| b == 0);
    let mut c = bytes(
        raw,
        if empty || kind == 0 {
            "unknown"
        } else if kind > 4 {
            "reserved"
        } else {
            "known"
        },
    );
    c.reason = if kind == 0 {
        Some(if empty {
            "id_type_none_empty"
        } else {
            "id_type_none_with_bytes"
        })
    } else if empty {
        Some("empty_identifier")
    } else if kind > 4 {
        Some("reserved_id_type_opaque_bytes")
    } else {
        None
    };
    if kind == 4 && !empty {
        match raw.first().copied() {
            Some(0) => {
                c.status = "out_of_range";
                c.reason = Some("session_method_reserved");
            }
            Some(225..=255) => {
                c.status = "private_use";
                c.reason = Some("session_method_private_experimental");
            }
            _ => c.reason = Some("session_method_profile_required"),
        }
    }
    c
}
fn auth_data(raw: &[u8], kind: u8) -> Cell<'_> {
    let empty = raw.iter().all(|&b| b == 0);
    let mut c = bytes(raw, "known");
    match (kind, empty) {
        (0, true) => {
            c.status = "unknown";
            c.reason = Some("auth_type_none_empty");
        }
        (0, false) => {
            c.status = "out_of_range";
            c.reason = Some("auth_type_none_with_data");
        }
        (4, true) => c.reason = Some("network_auth_requires_null_data"),
        (4, false) => {
            c.status = "out_of_range";
            c.reason = Some("network_auth_nonzero_data");
        }
        _ => {}
    };
    c
}

/// Interpret a complete v2 message; all other inputs retain their exact slice.
pub fn decode_message(raw: &[u8]) -> Result<Message<'_>, CapacityError> {
    let mut m = Message {
        raw,
        message_type: None,
        protocol_version: None,
        parse_status: "truncated",
        body_name: None,
        fields: Vec::new(),
    };
    let Some(&h) = raw.first() else {
        return Ok(m);
    };
    let kind = h >> 4;
    m.message_type = Some(code(
        kind,
        if kind <= 5 || kind == 15 {
            "known"
        } else {
            "reserved"
        },
    ));
    m.protocol_version = Some(version(h & 15));
    if h & 15 != 2 {
        m.parse_status = "unsupported_version";
        return Ok(m);
    }
    let Ok(b): Result<&[u8; 25], _> = raw.try_into() else {
        m.parse_status = if raw.len() < 25 {
            "truncated"
        } else {
            "malformed"
        };
        return Ok(m);
    };
    let mut add = |name, cell| {
        m.fields
            .push(Field { name, cell })
            .map_err(|_| CapacityError)
    };
    let u16at = |i| u16::from_le_bytes([b[i], b[i + 1]]);
    let i32at = |i| i32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let u32at = |i| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    m.parse_status = "complete";
    match kind {
        0 => {
            m.body_name = Some("basic_id");
            let id = b[1] >> 4;
            add("id_type", code(id, enum_status(id, 4, true)))?;
            add("ua_type", code(b[1] & 15, enum_status(b[1] & 15, 15, true)))?;
            add("uas_id", identifier(&b[2..22], id))?;
            add("reserved", bytes(&b[22..25], "reserved"))?;
        }
        1 => {
            m.body_name = Some("location");
            add(
                "operational_status",
                code(b[1] >> 4, enum_status(b[1] >> 4, 4, true)),
            )?;
            add("status_reserved", code((b[1] >> 3) & 1, "reserved"))?;
            add("height_type", code((b[1] >> 2) & 1, "known"))?;
            add("ew_direction_segment", code((b[1] >> 1) & 1, "known"))?;
            add("speed_multiplier", code(b[1] & 1, "known"))?;
            add("track_direction", direction(b[2], b[1] & 2 != 0))?;
            add("speed", horizontal(b[3], b[1] & 1 != 0))?;
            add("vertical_speed", vertical(b[4] as i8))?;
            add("latitude", coordinate(i32at(5), i32at(9), true))?;
            add("longitude", coordinate(i32at(9), i32at(5), false))?;
            add(
                "pressure_altitude",
                altitude(u16at(13), "m_pressure_29.92_inHg"),
            )?;
            add("geodetic_altitude_hae", altitude(u16at(15), "m_WGS84_HAE"))?;
            add("height", altitude(u16at(17), "m_per_height_type"))?;
            add("vertical_accuracy", accuracy(b[19] >> 4, 6))?;
            add("horizontal_accuracy", accuracy(b[19] & 15, 12))?;
            add("baro_altitude_accuracy", accuracy(b[20] >> 4, 6))?;
            add("speed_accuracy", accuracy(b[20] & 15, 4))?;
            add("timestamp", location_time(u16at(21)))?;
            add(
                "timestamp_accuracy",
                number(
                    (b[23] & 15).into(),
                    (b[23] & 15).into(),
                    1,
                    if b[23] & 15 == 0 { "unknown" } else { "known" },
                    "s",
                    "0.1",
                ),
            )?;
            add("timestamp_reserved", code(b[23] >> 4, "reserved"))?;
            add("reserved", bytes(&b[24..25], "reserved"))?;
        }
        2 => {
            m.body_name = Some("auth");
            let t = b[1] >> 4;
            let page = b[1] & 15;
            add(
                "auth_type",
                code(
                    t,
                    if t == 0 {
                        "unknown"
                    } else if t <= 5 {
                        "known"
                    } else if t <= 9 {
                        "reserved"
                    } else {
                        "private_use"
                    },
                ),
            )?;
            add("data_page_number", code(page, "known"))?;
            if page == 0 {
                add("page0_reserved", code(b[2] >> 4, "reserved"))?;
                add("last_page", code(b[2] & 15, "known"))?;
                let mut length = code(b[3], "known");
                length.unit = "byte";
                add("length", length)?;
                add("timestamp", epoch_time(u32at(4)))?;
                add("auth_data_page0", auth_data(&b[8..25], t))?;
                add("auth_data_pages_1_15", na("byte", "1"))?;
            } else {
                add("page0_reserved", na("code", "1"))?;
                add("last_page", na("code", "1"))?;
                add("length", na("byte", "1"))?;
                add("timestamp", na("s_since_2019_01_01", "1"))?;
                add("auth_data_page0", na("byte", "1"))?;
                add("auth_data_pages_1_15", auth_data(&b[2..25], t))?;
            }
        }
        3 => {
            m.body_name = Some("self_id");
            add("description_type", code(b[1], byte_enum(b[1], 2)))?;
            add("description", text(&b[2..25], "empty_description"))?;
        }
        4 => {
            m.body_name = Some("system");
            add("flags_reserved", code(b[1] >> 5, "reserved"))?;
            add(
                "classification_type",
                code((b[1] >> 2) & 7, enum_status((b[1] >> 2) & 7, 1, true)),
            )?;
            add(
                "operator_location_altitude_source_type",
                code(b[1] & 3, enum_status(b[1] & 3, 2, false)),
            )?;
            add("operator_latitude", coordinate(i32at(2), i32at(6), true))?;
            add("operator_longitude", coordinate(i32at(6), i32at(2), false))?;
            let count = u16at(10);
            add(
                "area_count",
                cell(
                    Value::Integer(count.into()),
                    Value::Integer(count.into()),
                    if count > 65000 {
                        "out_of_range"
                    } else {
                        "known"
                    },
                    "aircraft",
                    "1",
                ),
            )?;
            add(
                "area_radius",
                number(
                    b[12].into(),
                    i64::from(b[12]) * 10,
                    0,
                    if b[12] > 250 { "out_of_range" } else { "known" },
                    "m",
                    "10",
                ),
            )?;
            add("area_ceiling_hae", altitude(u16at(13), "m_WGS84_HAE"))?;
            add("area_floor_hae", altitude(u16at(15), "m_WGS84_HAE"))?;
            add(
                "category",
                code(b[17] >> 4, enum_status(b[17] >> 4, 3, true)),
            )?;
            add("class", code(b[17] & 15, enum_status(b[17] & 15, 7, true)))?;
            add("operator_altitude_hae", altitude(u16at(18), "m_WGS84_HAE"))?;
            add("timestamp", epoch_time(u32at(20)))?;
            add("reserved", bytes(&b[24..25], "reserved"))?;
        }
        5 => {
            m.body_name = Some("operator_id");
            add("operator_id_type", code(b[1], byte_enum(b[1], 0)))?;
            add("operator_id", text(&b[2..22], "empty_operator_identifier"))?;
            add("reserved", bytes(&b[22..25], "reserved"))?;
        }
        _ => {
            m.parse_status = "unsupported_type";
        }
    }
    Ok(m)
}

/// Unit framing is evidence, not an admission filter. Declared slots retain
/// their size, including incomplete last slots. Excess bytes remain trailing.
#[derive(Clone, Copy, Debug)]
pub struct Unit<'a> {
    pub raw: &'a [u8],
    pub header: Option<u8>,
    pub size: Option<u8>,
    pub count: Option<u8>,
    pub is_pack: bool,
}
impl<'a> Unit<'a> {
    pub fn new(raw: &'a [u8], is_pack: bool) -> Self {
        Self {
            raw,
            is_pack,
            header: raw.first().copied(),
            size: if is_pack { raw.get(1).copied() } else { None },
            count: if is_pack { raw.get(2).copied() } else { None },
        }
    }
    pub fn messages(&self) -> impl Iterator<Item = &'a [u8]> {
        let raw = self.raw;
        let start = if self.is_pack {
            core::cmp::min(3, self.raw.len())
        } else {
            0
        };
        let size = if self.is_pack {
            usize::from(self.size.unwrap_or(0))
        } else {
            self.raw.len()
        };
        let count = if self.is_pack {
            usize::from(self.count.unwrap_or(0))
        } else {
            1
        };
        (0..count).filter_map(move |i| {
            if size == 0 {
                return None;
            }
            let a = start + i * size;
            if a >= raw.len() {
                None
            } else {
                Some(&raw[a..core::cmp::min(a + size, raw.len())])
            }
        })
    }
    pub fn trailing(&self) -> &'a [u8] {
        if !self.is_pack {
            return &[];
        }
        let end = core::cmp::min(
            self.raw.len(),
            3 + usize::from(self.size.unwrap_or(0)) * usize::from(self.count.unwrap_or(0)),
        );
        &self.raw[end..]
    }
    pub fn conformant_framing(&self) -> bool {
        if !self.is_pack {
            return self.raw.len() == 25;
        }
        matches!(
            (self.header, self.size, self.count),
            (Some(0xf2), Some(25), Some(1..=9))
        ) && self.raw.len() == 3 + 25 * usize::from(self.count.unwrap_or(0))
    }
}
