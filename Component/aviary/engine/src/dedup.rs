//! Edge-tier deduplication of Remote-ID frames.
//!
//! Per Wave 6.0b consensus (1000ms window): when 3+ feeders overlap on the
//! same drone, suppressing duplicates at the edge prevents the same drone
//! from arriving N-times-per-second on NATS subject
//! `cybrrd.telemetry.frame.rid.<node_id>`. ASTM Remote-ID broadcasts at
//! 1–2 Hz; a 1s window collapses redundant reports without smearing real
//! drone movement.
//!
//! Identity key is `(drone_id, mac_address)` — paired so a spoofing attack
//! that reuses a MAC with a different serial OR vice-versa still emits
//! both signals downstream.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// State for one BRRDfeeder node's edge dedup gate.
///
/// Single-threaded by construction: instantiated and operated entirely
/// within the capture spawn_blocking thread, so no `Mutex`/`RwLock` needed.
pub struct DedupGate {
    window: Duration,
    last_emit: HashMap<DedupKey, Instant>,
}

/// Compound identity used as the dedup key. Cloning is cheap because
/// `drone_id` is short (≤20 chars) and `mac` is a 6-byte array.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DedupKey {
    drone_id: String,
    mac: [u8; 6],
}

impl DedupGate {
    pub fn new(window_ms: u64) -> Self {
        DedupGate {
            window: Duration::from_millis(window_ms),
            last_emit: HashMap::new(),
        }
    }

    /// Returns `true` if this frame should be emitted. Returns `false`
    /// if it was last emitted less than `window` ago — caller should
    /// silently drop the duplicate.
    ///
    /// Side effect: stamps the key's last-emit time when returning `true`.
    pub fn should_emit(&mut self, drone_id: &str, mac: &[u8; 6]) -> bool {
        let key = DedupKey {
            drone_id: drone_id.to_string(),
            mac: *mac,
        };
        let now = Instant::now();
        match self.last_emit.get(&key) {
            Some(prev) if now.duration_since(*prev) < self.window => false,
            _ => {
                self.last_emit.insert(key, now);
                true
            }
        }
    }

    /// Periodic GC: drop entries older than 10× window so the map doesn't
    /// grow unbounded as drones come and go through the airspace. Cheap
    /// because we only retain in-window state plus a small grace tail.
    pub fn gc(&mut self) {
        let now = Instant::now();
        let max_age = self.window * 10;
        self.last_emit.retain(|_, &mut t| now.duration_since(t) < max_age);
    }

    /// Used by tests and (in future) ops dashboards. Kept public so a Pro
    /// telemetry stream could expose dedup-map size as a per-feeder metric.
    #[allow(dead_code)]
    pub fn tracked_count(&self) -> usize {
        self.last_emit.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn first_frame_passes() {
        let mut gate = DedupGate::new(1000);
        assert!(gate.should_emit("DRONE-A", &[1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn duplicate_within_window_is_suppressed() {
        let mut gate = DedupGate::new(1000);
        let mac = [1, 2, 3, 4, 5, 6];
        assert!(gate.should_emit("DRONE-A", &mac));
        assert!(!gate.should_emit("DRONE-A", &mac));
        assert!(!gate.should_emit("DRONE-A", &mac));
    }

    #[test]
    fn distinct_drones_pass_independently() {
        let mut gate = DedupGate::new(1000);
        let mac_a = [1, 2, 3, 4, 5, 6];
        let mac_b = [9, 8, 7, 6, 5, 4];
        assert!(gate.should_emit("DRONE-A", &mac_a));
        assert!(gate.should_emit("DRONE-B", &mac_b));
        // Same drone-id with different mac is also distinct (spoofing-tolerant)
        assert!(gate.should_emit("DRONE-A", &mac_b));
    }

    #[test]
    fn frame_passes_after_window_expires() {
        let mut gate = DedupGate::new(50); // short window for fast test
        let mac = [1, 2, 3, 4, 5, 6];
        assert!(gate.should_emit("DRONE-A", &mac));
        assert!(!gate.should_emit("DRONE-A", &mac));
        sleep(Duration::from_millis(60));
        assert!(gate.should_emit("DRONE-A", &mac));
    }

    #[test]
    fn gc_removes_stale_entries() {
        let mut gate = DedupGate::new(10); // 10ms window → max_age = 100ms
        for i in 0..5 {
            gate.should_emit(&format!("DRONE-{}", i), &[i, 0, 0, 0, 0, 0]);
        }
        assert_eq!(gate.tracked_count(), 5);
        sleep(Duration::from_millis(150));
        gate.gc();
        assert_eq!(gate.tracked_count(), 0);
    }
}
