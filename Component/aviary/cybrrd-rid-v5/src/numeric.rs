// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Integer-only arithmetic shared by message decoding and exhaustive gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Number {
    pub scaled: i64,
    pub places: u8,
    pub status: &'static str,
    pub reason: Option<&'static str>,
}
impl Number {
    pub fn decimal(self) -> String {
        decimal(self.scaled, self.places)
    }
}
pub fn decimal(scaled: i64, places: u8) -> String {
    if scaled == 0 {
        return "0".into();
    }
    let mut digits = scaled.unsigned_abs().to_string();
    if places > 0 {
        while digits.len() <= places as usize {
            digits.insert(0, '0');
        }
        digits.insert(digits.len() - places as usize, '.');
        while digits.ends_with('0') {
            digits.pop();
        }
        if digits.ends_with('.') {
            digits.pop();
        }
    }
    if scaled < 0 {
        digits.insert(0, '-');
    }
    digits
}
fn n(scaled: i64, places: u8, status: &'static str) -> Number {
    Number {
        scaled,
        places,
        status,
        reason: None,
    }
}
#[inline]
pub fn altitude(raw: u16) -> Number {
    let mut out = n(
        i64::from(raw) * 5 - 10000,
        1,
        match raw {
            0 => "unknown",
            65535 => "out_of_stated_range",
            _ => "known",
        },
    );
    if raw == 65535 {
        out.reason = Some("arithmetic_exceeds_stated_range");
    }
    out
}
#[inline]
pub fn vertical(raw: i8) -> Number {
    n(
        i64::from(raw) * 5,
        1,
        match raw {
            126 => "unknown",
            124 => "saturated_lower",
            -124 => "saturated_upper",
            -123..=123 => "known",
            _ => "out_of_range",
        },
    )
}
#[inline]
pub fn speed(raw: u8, multiplier: bool) -> Number {
    let value = if multiplier {
        i64::from(raw) * 75 + 6375
    } else {
        i64::from(raw) * 25
    };
    n(
        value,
        2,
        match value {
            25500 => "unknown",
            25425 => "saturated_lower",
            _ => "known",
        },
    )
}
#[inline]
pub fn direction(raw: u8, segment: bool) -> Number {
    let value = i64::from(raw) + if segment { 180 } else { 0 };
    n(
        value,
        0,
        match value {
            361 => "unknown",
            0..=359 => "known",
            _ => "out_of_range",
        },
    )
}
#[inline]
pub fn hour_timestamp(raw: u16) -> Number {
    n(
        i64::from(raw),
        1,
        match raw {
            65535 => "unknown",
            36001..=65534 => "out_of_range",
            _ => "known",
        },
    )
}
#[inline]
pub fn epoch_timestamp(raw: u32) -> Number {
    n(i64::from(raw), 0, "known")
}
#[inline]
pub fn coordinate(raw: i32, other: i32, latitude: bool) -> Number {
    let limit = if latitude { 900000000_i64 } else { 1800000000 };
    n(
        i64::from(raw),
        7,
        if raw == 0 && other == 0 {
            "unknown"
        } else if i64::from(raw).abs() > limit {
            "out_of_range"
        } else {
            "known"
        },
    )
}
pub fn timestamp_accuracy(raw: u8) -> Number {
    n(
        i64::from(raw),
        1,
        if raw == 0 { "unknown" } else { "known" },
    )
}
pub fn accuracy_status(raw: u8, highest: u8) -> &'static str {
    if raw == 0 {
        "unknown"
    } else if raw > highest {
        "reserved"
    } else {
        "known"
    }
}

/// The epoch conversion is derived; the original seconds remain in the cell.
pub fn unix_seconds(raw: u32) -> u64 {
    u64::from(raw) + 1_546_300_800
}

/// A receipt clock can support more than one hour. Return candidates rather
/// than inventing an exact timestamp. No clock means no epoch inference.
pub fn hour_candidates(raw: u16, receive_unix_tenths: Option<u64>, error_tenths: u64) -> Vec<u64> {
    if raw > 36000 {
        return vec![];
    }
    let Some(now) = receive_unix_tenths else {
        return vec![];
    };
    let lo = now.saturating_sub(error_tenths);
    let hi = now.saturating_add(error_tenths);
    // Bounded output even when clock error is unbounded; ambiguity is explicit.
    if hi / 36000 - lo / 36000 > 2 {
        return vec![];
    }
    let mut out = vec![];
    for hour in lo / 36000..=hi / 36000 {
        let start = hour * 36000;
        let lower = lo.max(start);
        let upper = hi.min(start.saturating_add(35999));
        let current = start.checked_add(u64::from(raw));
        if let Some(current) = current {
            if current <= upper && !out.contains(&current) {
                out.push(current);
            }
        }
        if current.is_none_or(|current| lower < current) {
            if let Some(previous) = start
                .checked_sub(36000)
                .and_then(|s| s.checked_add(u64::from(raw)))
            {
                if !out.contains(&previous) {
                    out.push(previous);
                }
            }
        }
    }
    out.sort_unstable();
    out
}
