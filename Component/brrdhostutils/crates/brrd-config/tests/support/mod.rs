use brrd_core::{Disk, Reading, Snapshot, SnapshotSource, Usage};
use std::time::{Duration, UNIX_EPOCH};

pub fn snapshot() -> Snapshot {
    Snapshot {
        cores: Reading::Value(4),
        cpu: Reading::Value(37.4),
        load: Reading::Value([0.25, 0.5, 0.75]),
        memory: Reading::Value(Usage {
            used: 1 << 30,
            total: 3_884_000 * 1024,
        }),
        swap: Reading::Value(Usage {
            used: 256 << 20,
            total: 1 << 30,
        }),
        disks: Reading::Value(vec![Disk {
            mount: "/".into(),
            usage: Reading::Value(Usage {
                used: 8 << 30,
                total: 32 << 30,
            }),
        }]),
        read_at: Reading::Value(UNIX_EPOCH + Duration::from_secs(45_296)),
    }
}
pub struct Fixed(pub Snapshot);
impl SnapshotSource for Fixed {
    fn snapshot(&mut self) -> Snapshot {
        self.0.clone()
    }
}
