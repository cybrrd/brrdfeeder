// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Allocation traffic, not elapsed time or peak memory, is the regression oracle.
#![cfg(feature = "wire-v5")]
use cybrrd_rid_protocol::v5::{adapters, producer::*, schema, Result};
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[derive(Clone, Copy, Default)]
struct Traffic {
    frame_size: usize,
    frame_allocations: usize,
    allocated_bytes: usize,
}
thread_local! {
    // Per-thread, allocation-free bookkeeping excludes the test harness and
    // other tests. A zero frame size disables measurement.
    static TRAFFIC: Cell<Traffic> = const { Cell::new(Traffic {
        frame_size: 0, frame_allocations: 0, allocated_bytes: 0,
    }) };
}
fn account(size: usize, align: usize) {
    let _ = TRAFFIC.try_with(|cell| {
        let mut t = cell.get();
        if t.frame_size != 0 {
            t.allocated_bytes = t.allocated_bytes.saturating_add(size);
            if size == t.frame_size && align == 1 {
                t.frame_allocations += 1;
            }
            cell.set(t);
        }
    });
}
struct Counted;
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        account(layout.size(), layout.align());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        account(layout.size(), layout.align());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        account(size, layout.align());
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counted = Counted;

struct Measurement;
impl Measurement {
    fn start(frame_size: usize) -> Self {
        TRAFFIC.with(|t| {
            t.set(Traffic {
                frame_size,
                ..Traffic::default()
            })
        });
        Self
    }
    fn finish(self) -> Traffic {
        TRAFFIC.with(|t| t.replace(Traffic::default()))
    }
}
impl Drop for Measurement {
    fn drop(&mut self) {
        TRAFFIC.with(|t| t.set(Traffic::default()));
    }
}
#[derive(Default)]
struct Ledger(Vec<Vec<u8>>);
impl RawLedger for Ledger {
    fn append(&mut self, event: &Event) -> Result<Receipt> {
        self.0.push(event.bytes.clone());
        Ok(Receipt {
            ledger_record: "resource-test".into(),
        })
    }
}
fn capture_input(k: usize) -> Capture {
    assert!(k <= 327, "resource fixture exceeds the radio body budget");
    let mut bytes = vec![
        0, 0, 17, 0, 0x6e, 0x08, 0, 0, 0x50, 12, 0x6c, 0x09, 0, 0, 214, 166, 1,
    ];
    let mut frame = vec![0; 36];
    frame[0] = 0x80;
    for counter in 0..k {
        frame.extend([0xdd, 5, 0xfa, 0x0b, 0xbc, 0x0d, (counter & 255) as u8]);
    }
    bytes.extend(frame);
    bytes.extend([0xde, 0xad, 0xbe, 0xef]);
    assert_eq!(bytes.len(), 57 + 7 * k);
    Capture {
        event: Event {
            identity: json!({"node_id":"resource-node","boot_id":"resource-boot","capture_sequence":"1"}),
            boundary: "resource-test".into(),
            metadata: json!({}),
            bytes,
        },
        format: "raw-adapter".into(),
        source_kind: "synthetic".into(),
        adapter_version: "0".into(),
        completeness: "full_at_adapter_boundary".into(),
        original_length: None,
        unavailable_reason: None,
        transport_kind: "unknown".into(),
        spans: Vec::new(),
        receiver_cells: Default::default(),
        transport_cells: Default::default(),
        related_captures: Vec::new(),
    }
}

#[test]
fn service_allocation_traffic_is_linear() {
    // Warm the shared contract before measuring. No wall-clock assertion.
    schema::contract().unwrap();
    let mut previous: Option<(usize, usize)> = None;
    for k in [32, 64, 128, 256] {
        let input = capture_input(k);
        let n = input.event.bytes.len();
        let mut ledger = Ledger::default();
        let measurement = Measurement::start(n);
        let retained =
            adapters::capture(&mut ledger, input, adapters::Profile::WifiRadiotapBasic).unwrap();
        let traffic = measurement.finish();
        let Interpretation::V5(observation) = &retained.interpretation else {
            panic!("resource fixture must reach the complete model");
        };
        assert_eq!(
            observation.record()["rid_units"].as_array().unwrap().len(),
            k
        );
        assert_eq!(ledger.0.len(), 1);
        assert_eq!(ledger.0[0], retained.event.bytes);
        println!("P0_SERVICE_ALLOCATION k={k} input_bytes={n} frame_allocations={} frame_copy_bytes={} total_allocated_bytes={}",
            traffic.frame_allocations, traffic.frame_allocations * n, traffic.allocated_bytes);
        // One complete byte copy in this ledger, one in the adapter walker.
        // Each Vec<u8>::clone of the whole input allocates exactly n bytes.
        // The old per-service clone adds k allocations and fails this bound.
        assert_eq!(
            traffic.frame_allocations, 2,
            "per-occurrence whole-frame copy at k={k}"
        );
        assert_eq!(traffic.frame_allocations * n, 2 * (57 + 7 * k));
        if let Some((old_k, old_bytes)) = previous {
            // The entire pipeline also stays within a linear allocation envelope.
            // Ten percent permits fixed-size allocator/container growth effects;
            // this is an integer byte-volume check, never a timing threshold.
            assert!(
                traffic.allocated_bytes * old_k * 10 <= old_bytes * k * 11,
                "superlinear allocation traffic at k={k}"
            );
        }
        previous = Some((k, traffic.allocated_bytes));
    }
    println!("P0_SERVICE_ALLOCATION_LADDER=4/4; LINEAR_GROWTH_PAIRS=3/3");
}

#[test]
fn largest_in_cap_rid_frame_allocation() {
    schema::contract().unwrap();
    // Independent wire geometry: beacon fixed body 12, minimum RID IE 7.
    let k = (2304 - 12) / 7;
    assert_eq!(k, 327);
    assert!(12 + (k + 1) * 7 > 2304);
    let mut input = capture_input(k);
    // Use the full 64-byte radiotap allowance and the remaining three body
    // bytes as a non-RID IE, yielding exactly the 2396-byte capture cap.
    input.event.bytes.splice(17..17, [0; 47]);
    input.event.bytes[2] = 64;
    let end = input.event.bytes.len() - 4;
    input.event.bytes.splice(end..end, [0, 1, 0]);
    assert_eq!(input.event.bytes.len(), 2396);
    let mut ledger = Ledger::default();
    let measurement = Measurement::start(input.event.bytes.len());
    let retained =
        adapters::capture(&mut ledger, input, adapters::Profile::WifiRadiotapBasic).unwrap();
    let traffic = measurement.finish();
    let Interpretation::V5(observation) = &retained.interpretation else {
        panic!(
            "largest in-cap frame refused: {:?}",
            retained.interpretation
        );
    };
    assert_eq!(
        observation.record()["rid_units"].as_array().unwrap().len(),
        k
    );
    assert_eq!(ledger.0.len(), 1);
    assert_eq!(ledger.0[0], retained.event.bytes);
    assert_eq!(
        traffic.frame_allocations, 2,
        "largest in-cap whole-frame copy"
    );
    println!("P0_LARGEST_RADIO_FRAME=1/1; BYTES=2396; RID_IES={k}; ALLOCATED_BYTES={}; FRAME_ALLOCATIONS={}", traffic.allocated_bytes, traffic.frame_allocations);
}
