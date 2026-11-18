// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Specification-derived oracles, independent of the production rule table.
use super::{advertising_reports, valid_extended_metadata};

fn header(flags: u16, sid: u8, interval: u16) -> [u8; 24] {
    let mut h = [0; 24];
    h[..2].copy_from_slice(&flags.to_le_bytes());
    h[9] = 1;
    h[10] = 3;
    h[11] = sid;
    h[12] = 0x7f;
    h[13] = 0x7f;
    h[14..16].copy_from_slice(&interval.to_le_bytes());
    h
}

fn event(headers: &[[u8; 24]]) -> Vec<u8> {
    let mut p = vec![4, 0x3e, 0, 0x0d, headers.len() as u8];
    for h in headers {
        p.extend(h);
        p.resize(p.len() + usize::from(h[23]), 0); // synthetic payload
    }
    p[2] = u8::try_from(p.len() - 3).unwrap();
    p
}

#[derive(Default)]
struct Matrix {
    total: usize,
    positive: usize,
    mismatches: usize,
    examples: Vec<String>,
}
impl Matrix {
    fn check(&mut self, h: [u8; 24], expected: bool) {
        self.total += 1;
        self.positive += usize::from(expected);
        let valid = valid_extended_metadata(&h);
        let (count, ads, coded) = advertising_reports(&event(&[h])).unwrap();
        let expected_coded = expected && (h[9] == 3 || h[10] == 3);
        if valid != expected || coded != expected_coded || count != 1 || (!expected && !ads.is_empty()) {
            self.mismatches += 1;
            if self.examples.len() < 8 {
                self.examples.push(format!("h={h:02x?}: expected valid={expected}, actual={valid}, coded={coded}"));
            }
        }
    }
    fn finish(self, name: &str, total: usize, positive: usize) {
        assert_eq!(self.total, total);
        assert_eq!(self.positive, positive);
        println!("{name}: total={}, positive={}, negative={}, mismatches={}", self.total, self.positive, self.total-self.positive, self.mismatches);
        assert_eq!(self.mismatches, 0, "{}", self.examples.join("\n"));
    }
}

#[test]
fn periodic_flags_and_every_sid_matrix() {
    let mut m = Matrix::default();
    for flags in 0..128 {
        for sid in 0..=255 {
            for interval in [6, u16::MAX] {
                // Vol 6 B Table 2.5: both nonconnectable/nonscannable rows
                // allow SyncInfo. Complete and both incomplete statuses exist.
                let valid = [0x00, 0x04, 0x20, 0x24, 0x40, 0x44].contains(&flags) && sid <= 15;
                m.check(header(flags, sid, interval), valid);
            }
        }
    }
    m.finish("periodic flags/SID", 65_536, 192);
}

#[test]
fn nonperiodic_flags_and_every_sid_matrix() {
    let mut m = Matrix::default();
    for flags in 0..128 {
        for sid in 0..=255 {
            // AUX_ADV_IND needs ADI (Tables 2.4/2.5). AUX_SCAN_RSP may omit
            // it (Table 2.10); HCI recommends retaining the original SID.
            let advertisement = [0, 4, 0x20, 0x24, 0x40, 0x44, 1, 5, 0x21, 0x25, 2, 6].contains(&flags);
            let response = [0x0a, 0x0e, 0x2a, 0x2e, 0x4a, 0x4e].contains(&flags);
            let valid = (advertisement && sid <= 15) || (response && (sid <= 15 || sid == 255));
            m.check(header(flags, sid, 0), valid);
        }
    }
    m.finish("nonperiodic flags/SID", 32_768, 294);
}

#[test]
fn primary_only_presence_matrix() {
    let mut m = Matrix::default();
    for properties in [0, 4] {
        for primary in [1, 3] {
            for address in [0, 1, 2, 3, 255] {
                for sid in 0..=255 {
                    for data_len in [0, 1, 31] {
                        for status in [0, 0x20, 0x40, 0x60] {
                            for interval in [0, 6] {
                                let mut h = header(properties | status, sid, interval);
                                h[2] = address; h[9] = primary; h[10] = 0; h[23] = data_len;
                                // Vol 6 B Table 2.4, both no-auxiliary rows:
                                // AdvA mandatory; ADI, SyncInfo, AdvData absent.
                                m.check(h, address != 255 && sid == 255 && data_len == 0 && status == 0 && interval == 0);
                            }
                        }
                    }
                }
            }
        }
    }
    m.finish("primary-only presence", 122_880, 16);
}

#[test]
fn legacy_field_combinations_matrix() {
    let mut m = Matrix::default();
    for flags in 0..128 {
        for sid in [0, 255] {
            for primary in [1, 3] {
                for secondary in [0, 3] {
                    for len in [0, 31, 32] {
                        // Enumerate the 64 distinct legacy bit combinations.
                        if flags & 0x10 == 0 { continue; }
                        let legacy_flags = flags;
                        let mut h = header(legacy_flags, sid, 0);
                        h[9] = primary; h[10] = secondary; h[23] = len;
                        let valid = [0x10, 0x12, 0x13, 0x15, 0x1a, 0x1b].contains(&legacy_flags)
                            && sid == 255 && primary == 1 && secondary == 0
                            && len <= 31 && (legacy_flags != 0x15 || len == 0);
                        m.check(h, valid);
                    }
                }
            }
        }
    }
    m.finish("legacy field combinations", 1_536, 11);
}

#[test]
fn metadata_scalar_domains_and_reserved_bits() {
    let mut m = Matrix::default();
    for byte in 0..=255 {
        let good = header(0, 0, 6);
        let mut h = good; h[2] = byte; m.check(h, byte <= 3);
        let mut h = good; h[9] = byte; m.check(h, byte == 1 || byte == 3);
        let mut h = good; h[9] = 3; h[10] = byte; m.check(h, (1..=3).contains(&byte));
        let mut h = good; h[0] = 4; h[16] = byte; m.check(h, byte <= 3 || byte == 0xfe);
        let mut h = good; h[16] = byte; m.check(h, true); // ignored when undirected
        for index in [12, 13] {
            let mut h = good; h[index] = byte;
            m.check(h, byte == 127 || (-127..=20).contains(&(byte as i8)));
        }
    }
    for interval in 0..=u16::MAX {
        m.check(header(0, 0, interval), interval == 0 || interval >= 6);
    }
    for bit in 7..16 {
        for sid in [0, 255] { m.check(header(1 << bit, sid, 6), false); }
    }
    m.finish("scalar domains/reserved bits", 67_346, 66_099);
}

#[test]
fn periodic_missing_sid_reproducer() {
    let mut h = header(0, 255, 6);
    assert!(!advertising_reports(&event(&[h])).unwrap().2);
    h[13] = (-40i8) as u8;
    let (_, ads, coded) = advertising_reports(&event(&[h])).unwrap();
    assert!(!coded); assert!(ads.is_empty()); // no bypass through emitted ADs
}

#[test]
fn directed_periodic_is_valid_with_sid() {
    // A report describing the announcing AUX_ADV_IND can be directed. This is
    // not an HCI LE Periodic Advertising Report for an AUX_SYNC_IND packet.
    // Core v5.4 Vol 6 B 2.3.1.6 Table 2.5 explicitly allows SyncInfo in this row.
    let h = header(4, 0, 6);
    assert!(advertising_reports(&event(&[h])).unwrap().2);
    let mut one_m = header(0, 0, 0); one_m[10] = 1;
    for reports in [[h, one_m], [one_m, h]] {
        assert!(advertising_reports(&event(&reports)).unwrap().2);
    }
}

#[test]
fn periodic_mixed_reproducers_and_framing() {
    let missing_sid = header(0, 255, 6);
    let mut one_m = header(0, 0, 0); one_m[10] = 1;
    for reports in [[missing_sid, one_m], [one_m, missing_sid]] {
        assert!(!advertising_reports(&event(&reports)).unwrap().2);
    }
    for flags in [0, 4] {
        let valid_periodic = header(flags, 0, 6);
        for reports in [[missing_sid, valid_periodic], [valid_periodic, missing_sid]] {
            let (count, _, coded) = advertising_reports(&event(&reports)).unwrap();
            assert_eq!(count, 2); assert!(coded);
        }
    }
    for flags in [0, 4, 0x20, 0x24, 0x40, 0x44] {
        let mut h = header(flags, 0, 6); h[10] = 1;
        assert!(!advertising_reports(&event(&[h])).unwrap().2);
    }
    let p = event(&[header(0, 0, 6)]);
    for n in 2..p.len() { assert!(advertising_reports(&p[..n]).is_err()); }
    let mut trailing = p.clone(); trailing.push(0); trailing[2] += 1;
    assert!(advertising_reports(&trailing).is_err());
    let mut short = p; short[28] = 1;
    assert!(advertising_reports(&short).is_err());
}
