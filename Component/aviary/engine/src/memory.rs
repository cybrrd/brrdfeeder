// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Optional byte-valued Silver observations. No authority, process inspection,
//! journal access or host-proc mount is granted to the engine.
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Memory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_rss_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_cgroup_current_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_cgroup_peak_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_rss_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_mem_total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_mem_available_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volatile_journal_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_cap_events: Option<u64>,
}

fn read(path: &Path) -> Option<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(16385).read_to_string(&mut text).ok()?;
    (text.len() <= 16384).then_some(text)
}

fn kib(text: &str, key: &str) -> Option<u64> {
    let mut matches = text.lines().filter(|line| line.starts_with(key));
    let line = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let mut fields = line.split_whitespace();
    if fields.next()? != key {
        return None;
    }
    let bytes = fields.next()?.parse::<u64>().ok()?.checked_mul(1024)?;
    (fields.next()? == "kB" && fields.next().is_none()).then_some(bytes)
}

fn cgroup_value(cgroup: &Path, name: &str) -> Option<u64> {
    let value = read(&cgroup.join(name))?;
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

#[derive(Deserialize)]
struct Host {
    schema_version: u8,
    boot_id: String,
    sampled_boottime_secs: u64,
    #[serde(flatten)]
    memory: Memory,
}

fn host(raw: &str, boot: &str, now: u64) -> Option<Memory> {
    let snapshot: Host = serde_json::from_str(raw).ok()?;
    if snapshot.schema_version != 1
        || boot.trim().is_empty()
        || snapshot.boot_id != boot.trim()
        || now.checked_sub(snapshot.sampled_boottime_secs)? > 90
    {
        return None;
    }
    Some(snapshot.memory)
}

fn boottime() -> Option<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid writable timespec; CLOCK_BOOTTIME includes suspend and is
    // shared with the host helper. Wall-clock changes cannot freshen old data.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) } != 0 {
        return None;
    }
    time.tv_sec.try_into().ok()
}

pub fn sample() -> Memory {
    let mut result = read(Path::new("/run/brrdfeeder-memory/host.json"))
        .and_then(|raw| {
            host(
                &raw,
                &read(Path::new("/proc/sys/kernel/random/boot_id"))?,
                boottime()?,
            )
        })
        .unwrap_or_default();
    result.engine_rss_bytes =
        read(Path::new("/proc/self/status")).and_then(|text| kib(&text, "VmRSS:"));
    // Resolve this process's v2 cgroup only. In Podman's private namespace it
    // is "/"; on a native host it is a relative path below the cgroup mount.
    let group = read(Path::new("/proc/self/cgroup")).and_then(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("0::"))
            .map(str::to_owned)
    });
    if let Some(group) =
        group.filter(|g| g.starts_with('/') && !g.split('/').any(|p| p == ".." || p == "."))
    {
        let cgroup = Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/'));
        result.engine_cgroup_current_bytes = cgroup_value(&cgroup, "memory.current");
        result.engine_cgroup_peak_bytes = cgroup_value(&cgroup, "memory.peak");
    } else {
        result.engine_cgroup_current_bytes = None;
        result.engine_cgroup_peak_bytes = None;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_absence_are_not_fabricated() {
        assert_eq!(kib("VmRSS: 14 kB\n", "VmRSS:"), Some(14336));
        for raw in [
            "",
            "VmRSS: 14 MB",
            "VmRSS: -1 kB",
            "VmRSS: 14 kB extra",
            "VmRSS: 1 kB\nVmRSS: 2 kB",
            "VmRSS: 18446744073709551615 kB",
        ] {
            assert_eq!(kib(raw, "VmRSS:"), None, "{raw}");
        }
        assert_eq!(
            serde_json::to_value(Memory::default()).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn host_snapshot_is_fresh_boot_bound_and_additive() {
        let raw = r#"{"schema_version":1,"boot_id":"boot-a","sampled_boottime_secs":100,"memory_cap_events":2,"console_rss_bytes":4096,"future_field":true}"#;
        let result = host(raw, "boot-a\n", 190).unwrap();
        assert_eq!(result.memory_cap_events, Some(2));
        assert_eq!(result.console_rss_bytes, Some(4096));
        assert_eq!(result.volatile_journal_bytes, None);
        assert!(host(raw, "boot-a", 191).is_none());
        assert!(host(raw, "boot-a", 99).is_none());
        assert!(host(raw, "boot-b", 100).is_none());
        assert!(host(
            &raw.replace("\"memory_cap_events\":2", "\"memory_cap_events\":-1"),
            "boot-a",
            100
        )
        .is_none());
    }
}
