// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::var_os("B06_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "outage-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn config(&self, cap: u64) -> Config {
        Config {
            directory: self.0.clone(),
            cap_bytes: cap,
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn message(i: u64, size: usize) -> Outbound {
    Outbound {
        subject: "test.frames".into(),
        payload: vec![i as u8; size].into(),
        capture_ms: i,
    }
}
#[test]
fn append_rotate_recover_and_retire_in_order() {
    let tmp = Scratch::new();
    let config = tmp.config(DEFAULT_CAP);
    let mut spool = Spool::open(&config).unwrap();
    for i in 0..30 {
        assert!(spool.append(&message(i, 100_000)).unwrap());
    }
    assert_eq!(spool.segments.len(), 3);
    let bytes = spool.bytes();
    drop(spool);
    let mut spool = Spool::open(&config).unwrap();
    assert_eq!(spool.bytes(), bytes);
    for start in [0, 10, 20] {
        let batch = spool.peek(10).unwrap();
        assert_eq!(
            batch
                .messages
                .iter()
                .map(|m| m.capture_ms)
                .collect::<Vec<_>>(),
            (start..start + 10).collect::<Vec<_>>()
        );
        spool.commit(&batch).unwrap();
    }
    assert_eq!(spool.bytes(), 0);
    drop(spool);
    let mut spool = Spool::open(&config).unwrap();
    assert!(spool.append(&message(100, 1)).unwrap());
    assert_eq!(spool.peek(1).unwrap().messages[0].capture_ms, 100);
}
#[test]
fn full_keeps_oldest_bytes_and_resumes_after_commit() {
    let tmp = Scratch::new();
    let size = encode(&message(0, 10)).unwrap().len() as u64;
    let mut spool = Spool::open(&tmp.config(size * 3)).unwrap();
    for i in 0..3 {
        assert!(spool.append(&message(i, 10)).unwrap());
    }
    let file = path(&tmp.0, 1);
    let before = std::fs::read(&file).unwrap();
    for i in 3..103 {
        assert!(!spool.append(&message(i, 10)).unwrap());
    }
    assert_eq!(std::fs::read(&file).unwrap(), before);
    assert_eq!(spool.bytes(), size * 3);
    let batch = spool.peek(3).unwrap();
    spool.commit(&batch).unwrap();
    assert!(spool.append(&message(103, 10)).unwrap());
}
#[test]
fn leased_batch_cannot_prepend_or_overshoot_capacity() {
    let tmp = Scratch::new();
    let size = encode(&message(0, 1)).unwrap().len() as u64;
    let mut spool = Spool::open(&tmp.config(size * 256)).unwrap();
    for i in 0..256 {
        assert!(spool.append(&message(i, 1)).unwrap());
    }
    for _ in 0..5 {
        let batch = spool.peek(256).unwrap();
        assert_eq!(batch.messages.len(), 256);
        assert!(!spool.append(&message(300, 1)).unwrap());
        assert_eq!(spool.bytes(), size * 256);
    }
    drop(spool);
    let spool = Spool::open(&tmp.config(size * 256)).unwrap();
    assert_eq!(spool.peek(300).unwrap().messages.len(), 256);
}
#[test]
fn corrupt_tail_is_retained_byte_identical_and_valid_prefix_replayed() {
    let tmp = Scratch::new();
    let config = tmp.config(DEFAULT_CAP);
    let mut spool = Spool::open(&config).unwrap();
    assert!(spool.append(&message(1, 10)).unwrap());
    drop(spool);
    OpenOptions::new()
        .append(true)
        .open(path(&tmp.0, 1))
        .unwrap()
        .write_all(b"partial record")
        .unwrap();
    let original = std::fs::read(path(&tmp.0, 1)).unwrap();
    let mut spool = Spool::open(&config).unwrap();
    assert!(spool.corrupt);
    assert!(spool.append(&message(2, 10)).unwrap());
    let batch = spool.peek(100).unwrap();
    assert_eq!(
        batch
            .messages
            .iter()
            .map(|m| m.capture_ms)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    spool.commit(&batch).unwrap();
    assert_eq!(std::fs::read(path(&tmp.0, 1)).unwrap(), original);
    assert_eq!(spool.bytes(), original.len() as u64);
}
#[test]
fn exclusive_lease_private_files_and_symlink_refusal() {
    let tmp = Scratch::new();
    let config = tmp.config(DEFAULT_CAP);
    let mut spool = Spool::open(&config).unwrap();
    assert!(Spool::open(&config).is_err());
    assert!(spool.append(&message(1, 10)).unwrap());
    assert_eq!(
        std::fs::metadata(path(&tmp.0, 1))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(spool);
    std::os::unix::fs::symlink("missing", tmp.0.join("cursor.json")).unwrap();
    assert!(Spool::open(&config).is_err());
}
#[test]
fn byte_arithmetic_and_fsync_latency() {
    let tmp = Scratch::new();
    let mut spool = Spool::open(&tmp.config(DEFAULT_CAP)).unwrap();
    let msg = Outbound {
        subject: "cybrrd.telemetry.frame.rid.test-node".into(),
        payload: crate::nats_publisher::tests::fixture_frame(1).into(),
        capture_ms: 1_800_000_000_040,
    };
    let size = encode(&msg).unwrap().len() as u64;
    for _ in 0..100 {
        assert!(spool.append(&msg).unwrap());
    }
    println!(
        "fixture_payload={} subject={} overhead={} stored={} max_append_fsync_us={}",
        msg.payload.len(),
        msg.subject.len(),
        HEADER,
        size,
        spool.max_sync_us
    );
    for rate in [1, 5, 25] {
        for seconds in [300, 3600, 86400] {
            println!(
                "rate={rate} seconds={seconds} messages={} stored_bytes={}",
                rate * seconds,
                rate * seconds * size
            );
        }
    }
    assert!(
        spool.max_sync_us <= 250_000,
        "measured fsync exceeded 250 ms: {} us",
        spool.max_sync_us
    );
}
#[test]
fn exact_gap_survives_restart_and_full_red_queue_equivalent() {
    use crate::telemetry_loss::{Boundary, Ledger};
    let tmp = Scratch::new();
    let mut ledger = Ledger::open(&tmp.0).unwrap();
    for i in 0..137 {
        ledger.record(Boundary::OutageSpool, 1000 + i).unwrap();
    }
    let pending = ledger.pending();
    assert_eq!(
        pending.iter().map(|g| g.count).collect::<Vec<_>>(),
        vec![1, 10, 100]
    );
    drop(ledger);
    let mut ledger = Ledger::open(&tmp.0).unwrap();
    assert_eq!(ledger.pending(), pending);
    let gap = &ledger.gaps()[0];
    assert_eq!(
        (gap.count, gap.first_capture_ms, gap.last_capture_ms),
        (137, 1000, 1136)
    );
    ledger.acknowledge(&pending[0]).unwrap();
    drop(ledger);
    let ledger = Ledger::open(&tmp.0).unwrap();
    assert_eq!(ledger.pending(), pending[1..]);
}
#[test]
fn torn_gap_update_recovers_previous_checksummed_slot() {
    use crate::telemetry_loss::{Boundary, Ledger};
    let tmp = Scratch::new();
    let mut ledger = Ledger::open(&tmp.0).unwrap();
    ledger.record(Boundary::OutageSpool, 100).unwrap();
    ledger.record(Boundary::OutageSpool, 200).unwrap();
    drop(ledger);
    use std::os::unix::fs::FileExt;
    OpenOptions::new()
        .write(true)
        .open(tmp.0.join("gaps.slots"))
        .unwrap()
        .write_all_at(b"torn", 64 * 1024 + 44)
        .unwrap();
    let ledger = Ledger::open(&tmp.0).unwrap();
    assert_eq!(ledger.gaps()[0].count, 1);
}

#[test]
#[ignore = "requires an isolated small tmpfs, B06_ENOSPC_ROOT; never fills a host filesystem"]
fn actual_enospc_preserves_oldest_and_preallocated_gap_counts() {
    use crate::telemetry_loss::{Boundary, Ledger};
    let root = PathBuf::from(std::env::var_os("B06_ENOSPC_ROOT").expect("isolated tmpfs required"));
    assert_eq!(
        root,
        Path::new("/full-spool"),
        "only the test container tmpfs is permitted"
    );
    let config = Config {
        directory: root.clone(),
        cap_bytes: DEFAULT_CAP,
    };
    let mut spool = Spool::open(&config).unwrap();
    let mut ledger = Ledger::open(&root).unwrap();
    assert!(spool.append(&message(1, 500)).unwrap());
    let original = std::fs::read(path(&root, 1)).unwrap();
    let mut filler = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(root.join("test-only-filler"))
        .unwrap();
    let block = [0u8; 4096];
    let mut filled = 0;
    loop {
        match filler.write_all(&block) {
            Ok(()) => {
                filled += 4096;
                assert!(
                    filled <= 32 * 1024 * 1024,
                    "fixture filesystem is not bounded"
                );
            }
            Err(e) => {
                assert_eq!(e.raw_os_error(), Some(libc::ENOSPC));
                break;
            }
        }
    }
    filler.sync_all().unwrap();
    // Filling the last partial page is legal: force the next append to require
    // new pages rather than fitting in the original segment's allocated page.
    for i in 0..137 {
        let error = spool.append(&message(2 + i, 16384)).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOSPC));
        ledger.record(Boundary::OutageSpool, 2000 + i).unwrap();
    }
    assert!(
        std::fs::read(path(&root, 1))
            .unwrap()
            .starts_with(&original),
        "ENOSPC overwrote the oldest saved copy"
    );
    let retained = std::fs::read(path(&root, 1)).unwrap();
    drop(ledger);
    drop(spool);
    let ledger = Ledger::open(&root).unwrap();
    assert_eq!(ledger.gaps()[0].count, 137);
    assert_eq!(
        ledger.pending().iter().map(|g| g.count).collect::<Vec<_>>(),
        vec![1, 10, 100]
    );
    let mut spool = Spool::open(&config).unwrap();
    assert_eq!(spool.peek(10).unwrap().messages[0].capture_ms, 1);
    assert_eq!(std::fs::read(path(&root, 1)).unwrap(), retained);
    drop(filler);
    std::fs::remove_file(root.join("test-only-filler")).unwrap();
    assert!(spool.append(&message(3, 100)).unwrap());
    println!("ENOSPC PASS oldest retained; durable exact count=137; pending Red=1,10,100; append resumed");
}

#[test]
fn recovery_clears_active_impairment_but_keeps_history_and_unacked_red() {
    use crate::telemetry_loss::{Boundary, Ledger};
    let tmp = Scratch::new();
    let mut ledger = Ledger::open(&tmp.0).unwrap();
    ledger.record(Boundary::OutageSpool, 100).unwrap();
    assert!(ledger.impaired());
    ledger.resumed(Boundary::OutageSpool).unwrap();
    assert!(!ledger.impaired());
    assert_eq!(ledger.gaps()[0].count, 1);
    assert_eq!(ledger.pending().len(), 1);
    drop(ledger);
    let ledger = Ledger::open(&tmp.0).unwrap();
    assert!(!ledger.impaired());
    assert_eq!(ledger.gaps()[0].count, 1);
    assert_eq!(ledger.pending().len(), 1);
}
