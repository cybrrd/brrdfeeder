use crate::{Disk, Reading, Snapshot, SnapshotSource, Usage};
use std::{
    collections::HashMap,
    fs,
    time::{Instant, SystemTime},
};
use sysinfo::{DiskRefreshKind, Disks, System};

/// A read-only collector. It never requires feeder services or privileges.
#[derive(Default)]
pub struct SystemReader {
    cpu: Option<System>,
    sampled_at: Option<Instant>,
}
impl SystemReader {
    /// Obtain a one-shot report after the minimum CPU sampling interval.
    pub fn one_shot(&mut self) -> Snapshot {
        self.snapshot();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let mut snapshot = self.snapshot();
        if matches!(snapshot.cpu, Reading::Measuring) {
            snapshot.cpu = Reading::Unavailable;
        }
        snapshot
    }
}
impl SnapshotSource for SystemReader {
    fn snapshot(&mut self) -> Snapshot {
        let mut result = Snapshot::default();
        // sysinfo retains old CPU values on a failed read. Validate this sample
        // before refreshing, and discard the baseline on failure.
        let cpu_ok = fs::read_to_string("/proc/stat").ok().is_some_and(|s| {
            s.lines().next().is_some_and(|line| {
                let mut fields = line.split_whitespace();
                fields.next() == Some("cpu")
                    && fields.take(4).filter(|f| f.parse::<u64>().is_ok()).count() == 4
            })
        });
        if cpu_ok {
            if self.cpu.is_none() {
                self.cpu = System::new().ok();
            }
            if let Some(system) = &mut self.cpu {
                system.refresh_cpu_usage();
                if !system.cpus().is_empty() {
                    result.cores = Reading::Value(system.cpus().len());
                    result.cpu = if self
                        .sampled_at
                        .is_some_and(|t| t.elapsed() >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL)
                    {
                        Reading::Value(system.global_cpu_usage() as f64)
                    } else {
                        Reading::Measuring
                    };
                    self.sampled_at = Some(Instant::now());
                }
            }
        } else {
            self.cpu = None;
            self.sampled_at = None;
        }
        // Load averages have no error channel in sysinfo; parse the small source
        // file directly so an unreadable value cannot masquerade as zero.
        result.load = fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|s| parse_load(&s))
            .map_or(Reading::Unavailable, Reading::Value);
        // A fresh System avoids carrying memory values over a failed refresh.
        if let (Ok(text), Ok(mut system)) = (fs::read_to_string("/proc/meminfo"), System::new()) {
            system.refresh_memory();
            let fields: HashMap<_, _> = text
                .lines()
                .filter_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    Some((name, value.split_whitespace().next()?.parse::<u64>().ok()?))
                })
                .collect();
            if fields.contains_key("MemTotal")
                && fields.contains_key("MemAvailable")
                && system.total_memory() > 0
            {
                result.memory = Reading::Value(Usage {
                    used: system.used_memory(),
                    total: system.total_memory(),
                });
            }
            if fields.contains_key("SwapTotal") && fields.contains_key("SwapFree") {
                result.swap = Reading::Value(Usage {
                    used: system.used_swap(),
                    total: system.total_swap(),
                });
            }
        }
        result.disks = read_disks();
        if !result.cores.unavailable()
            || !result.load.unavailable()
            || !result.memory.unavailable()
            || !result.swap.unavailable()
            || matches!(&result.disks, Reading::Value(ds) if ds.iter().any(|d| !d.usage.unavailable()))
        {
            result.read_at = Reading::Value(SystemTime::now());
        }
        result
    }
}
fn parse_load(text: &str) -> Option<[f64; 3]> {
    let mut values = text.split_whitespace();
    let mut result = [0.0_f64; 3];
    for value in &mut result {
        *value = values.next()?.parse().ok()?;
        if !value.is_finite() || *value < 0.0 {
            return None;
        }
    }
    Some(result)
}

fn storage_backed(device: &str, kind: &str) -> bool {
    if matches!(
        kind,
        "tmpfs"
            | "ramfs"
            | "rootfs"
            | "proc"
            | "sysfs"
            | "devtmpfs"
            | "devpts"
            | "cgroup"
            | "cgroup2"
            | "pstore"
            | "securityfs"
            | "debugfs"
            | "tracefs"
            | "configfs"
            | "fusectl"
            | "mqueue"
            | "hugetlbfs"
            | "bpf"
            | "rpc_pipefs"
            | "autofs"
            | "nsfs"
    ) {
        return false;
    }
    device.starts_with("/dev/") && !device.starts_with("/dev/ram")
}
fn unescape_mount(text: &str) -> String {
    text.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}
fn read_disks() -> Reading<Vec<Disk>> {
    let Ok(mounts) = fs::read_to_string("/proc/self/mounts") else {
        return Reading::Unavailable;
    };
    // Start fresh: sysinfo's disk refresh also retains old values on failure.
    let disks =
        Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage()).ok();
    let mut result = Vec::new();
    for line in mounts.lines() {
        let fields: Vec<_> = line.split_whitespace().take(3).collect();
        if fields.len() != 3 {
            return Reading::Unavailable;
        }
        if !storage_backed(fields[0], fields[2]) {
            continue;
        }
        let mount = unescape_mount(fields[1]);
        // statvfs supplies an error channel and covers storage mounts sysinfo
        // omits, such as optical media and storage mounted below /run.
        let usage = match rustix::fs::statvfs(mount.as_str()) {
            Ok(stat) => {
                let fresh = disks.as_ref().and_then(|ds| {
                    ds.iter()
                        .find(|d| d.mount_point() == std::path::Path::new(&mount))
                });
                let (total, available) = match fresh {
                    Some(disk) if disk.total_space() > 0 => {
                        (disk.total_space(), disk.available_space())
                    }
                    _ => (
                        stat.f_blocks.saturating_mul(stat.f_frsize),
                        stat.f_bavail.saturating_mul(stat.f_frsize),
                    ),
                };
                Reading::Value(Usage {
                    used: total.saturating_sub(available),
                    total,
                })
            }
            Err(_) => Reading::Unavailable,
        };
        result.push(Disk { mount, usage });
    }
    result.sort_by(|a, b| a.mount.cmp(&b.mount));
    result.dedup_by(|a, b| a.mount == b.mount);
    Reading::Value(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disk_filter() {
        let mounts = [
            ("/dev/mmcblk0p2", "/", "ext4"),
            ("proc", "/proc", "proc"),
            ("sysfs", "/sys", "sysfs"),
            ("tmpfs", "/run", "tmpfs"),
            ("/dev/loop0", "/media", "squashfs"),
            ("/dev/sda1", "/run/storage", "ext4"),
            ("/dev/ram0", "/ram", "ext4"),
            ("overlay", "/container", "overlay"),
        ];
        let kept: Vec<_> = mounts
            .into_iter()
            .filter(|(d, _, f)| storage_backed(d, f))
            .map(|(_, m, _)| m)
            .collect();
        assert_eq!(kept, ["/", "/media", "/run/storage"]);
    }
    #[test]
    fn invalid_load_is_unavailable() {
        assert_eq!(parse_load("0.1 2.0 3.4 1/2 4"), Some([0.1, 2.0, 3.4]));
        for input in ["", "0.1 0.2", "NaN 0 0", "-1 0 0", "failed"] {
            assert_eq!(parse_load(input), None);
        }
    }
    #[test]
    fn host_core_count() {
        let mut reader = SystemReader::default();
        let snapshot = reader.snapshot();
        assert!(matches!(snapshot.cores, Reading::Value(n) if n > 0));
        assert_eq!(snapshot.cpu, Reading::Measuring);
        let next = reader.one_shot();
        assert!(matches!(next.cpu, Reading::Value(n) if (0.0..=100.0).contains(&n)));
        assert!(matches!(next.memory, Reading::Value(Usage {total, ..}) if total > 0));
        assert!(matches!(next.disks, Reading::Value(ref ds) if ds.iter().any(|d| d.mount == "/")));
    }
}
