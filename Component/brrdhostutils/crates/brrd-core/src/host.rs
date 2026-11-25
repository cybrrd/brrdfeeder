use crate::{Disk, Reading, Snapshot, SnapshotSource, Usage};
use std::{
    collections::{HashMap, HashSet},
    fs,
    time::{Instant, SystemTime},
};
use sysinfo::System;

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
#[derive(Debug)]
struct Mount {
    device_id: String,
    root: String,
    path: String,
    source: String,
    kind: String,
}
fn storage_mounts(mut mounts: Vec<Mount>) -> Vec<String> {
    // A device can have multiple mounts of its root; prefer the shallowest
    // mount and then lexical order, independently of mount-table order.
    mounts.sort_by(|a, b| {
        a.path
            .matches('/')
            .count()
            .cmp(&b.path.matches('/').count())
            .then(a.path.cmp(&b.path))
    });
    let mut devices = HashSet::new();
    let mut paths: Vec<_> = mounts
        .into_iter()
        .filter(|mount| mount.root == "/" && storage_backed(&mount.source, &mount.kind))
        .filter(|mount| devices.insert(mount.device_id.clone()))
        .map(|mount| mount.path)
        .collect();
    paths.sort();
    paths
}
fn parse_mountinfo(text: &str) -> Option<Vec<Mount>> {
    text.lines()
        .map(|line| {
            let (mount, filesystem) = line.split_once(" - ")?;
            let fields: Vec<_> = mount.split_whitespace().collect();
            let fs: Vec<_> = filesystem.split_whitespace().collect();
            if fields.len() < 6 || fs.len() < 3 {
                return None;
            }
            let (major, minor) = fields[2].split_once(':')?;
            major.parse::<u32>().ok()?;
            minor.parse::<u32>().ok()?;
            Some(Mount {
                device_id: fields[2].into(),
                root: unescape_mount(fields[3]),
                path: unescape_mount(fields[4]),
                source: unescape_mount(fs[1]),
                kind: fs[0].into(),
            })
        })
        .collect()
}
fn read_disks() -> Reading<Vec<Disk>> {
    let Ok(mounts) = fs::read_to_string("/proc/self/mountinfo") else {
        return Reading::Unavailable;
    };
    // mountinfo records the root within the filesystem, unlike mounts.
    let Some(parsed) = parse_mountinfo(&mounts) else {
        return Reading::Unavailable;
    };
    let mut result = Vec::new();
    for mount in storage_mounts(parsed) {
        // Read only selected top-directory mounts. statvfs reports errors and
        // distinguishes all free space from space available to ordinary users.
        let usage = match rustix::fs::statvfs(mount.as_str()) {
            Ok(stat) => Reading::Value(filesystem_usage(&stat)),
            Err(_) => Reading::Unavailable,
        };
        result.push(Disk { mount, usage });
    }
    result.sort_by(|a, b| a.mount.cmp(&b.mount));
    result.dedup_by(|a, b| a.mount == b.mount);
    Reading::Value(result)
}
fn filesystem_usage(stat: &rustix::fs::StatVfs) -> Usage {
    let total = stat.f_blocks.saturating_mul(stat.f_frsize);
    // Free blocks include the administrator reserve; available blocks do not.
    let free = stat.f_bfree.saturating_mul(stat.f_frsize);
    Usage {
        used: total.saturating_sub(free),
        total,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn feeder_mount_table() {
        // The recorded table uses findmnt's FSTYPE,SOURCE,TARGET columns.
        let mounts = include_str!("../tests/fixtures/feeder-mounts.txt")
            .lines()
            .map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                let (device, root) = fields[1]
                    .split_once('[')
                    .map_or((fields[1], "/"), |(device, root)| {
                        (device, root.strip_suffix(']').unwrap())
                    });
                Mount {
                    device_id: device.into(),
                    root: root.into(),
                    path: fields[2].into(),
                    source: device.into(),
                    kind: fields[0].into(),
                }
            })
            .collect();
        assert_eq!(storage_mounts(mounts), ["/", "/boot/firmware"]);
    }
    #[test]
    fn mountinfo_top_directories() {
        let text = "40 1 179:2 /var/lib/storage /container rw - ext4 /dev/mmcblk0p2 rw\n\
41 1 179:2 / /mirror rw shared:1 - ext4 /dev/disk/by-label/root rw\n\
42 1 179:2 / / rw - ext4 /dev/mmcblk0p2 rw\n\
43 1 179:1 / /boot/firmware rw - vfat /dev/mmcblk0p1 rw\n\
44 1 8:1 / /media/usb\\040disk rw - ext4 /dev/sda1 rw\n\
45 1 0:1 / /run rw - tmpfs tmpfs rw";
        assert_eq!(
            storage_mounts(parse_mountinfo(text).unwrap()),
            ["/", "/boot/firmware", "/media/usb disk"]
        );
        assert!(parse_mountinfo("not a mount record").is_none());
    }
    #[test]
    fn reserve_is_not_used() {
        let blocks_per_gib = (1u64 << 30) / 4096;
        let stat = rustix::fs::StatVfs {
            f_bsize: 4096,
            f_frsize: 4096,
            f_blocks: 234 * blocks_per_gib,
            f_bfree: 459 * blocks_per_gib / 2,
            f_bavail: 220 * blocks_per_gib,
            f_files: 0,
            f_ffree: 0,
            f_favail: 0,
            f_fsid: 0,
            f_flag: rustix::fs::StatVfsMountFlags::empty(),
            f_namemax: 255,
        };
        assert_eq!(filesystem_usage(&stat).text(), "4.5 GiB / 234.0 GiB (2%)");
    }
    #[test]
    fn agreement_with_df() {
        let started = Instant::now();
        let Reading::Value(disks) = read_disks() else {
            panic!("disk list unavailable")
        };
        let root = disks
            .iter()
            .find(|disk| disk.mount == "/")
            .expect("root filesystem missing");
        let Reading::Value(usage) = &root.usage else {
            panic!("root usage unavailable")
        };
        let output = std::process::Command::new("df")
            .env("LC_ALL", "C")
            .args(["-B1", "--output=used", "/"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let used: u64 = text.lines().nth(1).unwrap().trim().parse().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(
            usage.used.abs_diff(used) <= (1u64 << 30) / 10,
            "root used bytes: report={}, df={used}",
            usage.used
        );
    }
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
