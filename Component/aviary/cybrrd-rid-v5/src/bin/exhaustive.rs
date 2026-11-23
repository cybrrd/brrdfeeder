// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Dedicated release gate; never run the 32-bit domains in default cargo test.
use cybrrd_rid_v5::numeric;
use serde_json::json;
use std::{hint::black_box, time::Instant};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let start: u64 = args
        .get(1)
        .expect("usage: v5-exhaustive START END (exclusive, <=4294967296)")
        .parse()
        .expect("start");
    let end: u64 = args.get(2).expect("end").parse().expect("end");
    assert!(start < end && end <= 1_u64 << 32);
    let began = Instant::now();
    let mut checksum = 0_u64;
    for code in start..end {
        let raw = black_box(code as u32);
        // Each of the two bindings uses the same checked numeric primitive;
        // full message offset/endianness is tested independently in default CI.
        for field in 0..2_u64 {
            let actual = black_box(numeric::epoch_timestamp(raw));
            assert_eq!(actual.scaled as u64, code);
            assert_eq!(actual.places, 0);
            assert_eq!(actual.status, "known");
            assert_eq!(actual.reason, None);
            assert_eq!(numeric::unix_seconds(raw), code + 1_546_300_800);
            checksum = checksum.wrapping_add((actual.scaled as u64) ^ field);
        }
    }
    println!(
        "{}",
        json!({"gate":"timestamp_u32","start":start,"end_exclusive":end,"fields":["auth.timestamp","system.timestamp"],"cases":2*(end-start),"checksum":checksum,"elapsed_ms":began.elapsed().as_millis(),"status":"passed"})
    );
}
