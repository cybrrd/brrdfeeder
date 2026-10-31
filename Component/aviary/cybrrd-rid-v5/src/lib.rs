// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Development-only v5 decoder. No capture adapters, I/O, deduplication or custody.
pub mod auth;
pub mod capture;
pub mod codec;
pub mod decoder;
pub mod numeric;
pub mod schema;
pub use capture::{decode_capture, CaptureRecord, UnitSpan};
pub use decoder::decode_message;
pub use serde_json::Value;

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

pub fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    fn digit(b: u8) -> Result<u8, String> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err("noncanonical hex".into()),
        }
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|b| Ok(digit(b[0])? * 16 + digit(b[1])?))
        .collect()
}
