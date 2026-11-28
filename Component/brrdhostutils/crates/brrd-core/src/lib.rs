// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Read-only Linux system information, shared by both front ends.
//!
//! Keep a [`SystemReader`] and call [`SnapshotSource::snapshot`] once per second.
//! Each call returns fresh values; failures are local to a field and never reuse
//! a previous value. CPU usage starts as [`Reading::Measuring`]. For a one-shot
//! report use [`SystemReader::one_shot`], which waits for a second CPU sample.
//!
//! ```
//! use brrd_core::{SnapshotSource, SystemReader};
//! let mut reader = SystemReader::default();
//! let snapshot = reader.snapshot();
//! for line in snapshot.report_lines() {
//!     println!("{line}");
//! }
//! ```

mod host;
pub use host::SystemReader;
use std::time::{SystemTime, UNIX_EPOCH};

/// A field's current observation, without substituting zero for a failure.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading<T> {
    Value(T),
    Unavailable,
    Measuring,
}
impl<T> Reading<T> {
    pub fn text(&self, format: impl FnOnce(&T) -> String) -> String {
        match self {
            Self::Value(value) => format(value),
            Self::Unavailable => "unavailable".into(),
            Self::Measuring => "measuring".into(),
        }
    }
    pub fn unavailable(&self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

/// Used and total bytes for memory, swap, or one filesystem.
#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    pub used: u64,
    pub total: u64,
}
impl Usage {
    pub fn text(&self) -> String {
        format!(
            "{} / {} ({})",
            format_size(self.used),
            format_size(self.total),
            format_percentage(if self.total == 0 {
                0.0
            } else {
                self.used as f64 * 100.0 / self.total as f64
            })
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Disk {
    pub mount: String,
    pub usage: Reading<Usage>,
}

/// A current read. `read_at` is unavailable if no measurement succeeded.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub cores: Reading<usize>,
    pub cpu: Reading<f64>,
    pub load: Reading<[f64; 3]>,
    pub memory: Reading<Usage>,
    pub swap: Reading<Usage>,
    pub disks: Reading<Vec<Disk>>,
    pub read_at: Reading<SystemTime>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            cores: Reading::Unavailable,
            cpu: Reading::Unavailable,
            load: Reading::Unavailable,
            memory: Reading::Unavailable,
            swap: Reading::Unavailable,
            disks: Reading::Unavailable,
            read_at: Reading::Unavailable,
        }
    }
}
impl Snapshot {
    pub fn has_unavailable(&self) -> bool {
        self.cores.unavailable()
            || self.cpu.unavailable()
            || self.load.unavailable()
            || self.memory.unavailable()
            || self.swap.unavailable()
            || self.disks.unavailable()
            || matches!(&self.disks, Reading::Value(disks) if disks.iter().any(|d| d.usage.unavailable()))
    }
    /// The canonical names, values and units for both front ends.
    pub fn report_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("CPU cores: {}", self.cores.text(|v| v.to_string())),
            format!("CPU usage: {}", self.cpu.text(|v| format_percentage(*v))),
            format!(
                "Load (1, 5, 15 min): {}",
                self.load
                    .text(|v| format!("{:.2}, {:.2}, {:.2}", v[0], v[1], v[2]))
            ),
            format!("Memory: {}", self.memory.text(Usage::text)),
            format!(
                "Swap: {}",
                self.swap.text(|v| if v.total == 0 {
                    "none".into()
                } else {
                    v.text()
                })
            ),
        ];
        match &self.disks {
            Reading::Value(disks) => {
                lines.push("Disks (used / total):".into());
                for disk in disks {
                    lines.push(format!(
                        "  {}: {}",
                        plain_label(&disk.mount),
                        disk.usage.text(Usage::text)
                    ));
                }
            }
            other => lines.push(format!("Disks: {}", other.text(|_| unreachable!()))),
        }
        lines.push(format!(
            "Last reading: {}",
            self.read_at.text(|v| {
                match v.duration_since(UNIX_EPOCH) {
                    Ok(time) => {
                        let s = time.as_secs();
                        format!("{:02}:{:02}:{:02} UTC", s / 3600 % 24, s / 60 % 60, s % 60)
                    }
                    Err(_) => "unavailable".into(),
                }
            })
        ));
        lines
    }
}

/// Replaced by a deterministic source in screen tests.
pub trait SnapshotSource {
    fn snapshot(&mut self) -> Snapshot;
}

/// Binary MiB or GiB, always with one decimal place.
pub fn format_size(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1u64 << 20) as f64)
    }
}
pub fn format_percentage(percent: f64) -> String {
    format!("{percent:.0}%")
}
fn plain_label(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_units() {
        assert_eq!(format_size(3_884_000 * 1024), "3.7 GiB");
        assert_eq!(format_size(1_048_576), "1.0 MiB");
        assert_eq!(format_size(0), "0.0 MiB");
        assert_eq!(format_percentage(42.6), "43%");
    }
    #[test]
    fn reading_states() {
        assert_eq!(Reading::Value(12).text(|v| v.to_string()), "12");
        assert_eq!(
            Reading::<u64>::Unavailable.text(|v| v.to_string()),
            "unavailable"
        );
        assert_eq!(
            Reading::<u64>::Measuring.text(|v| v.to_string()),
            "measuring"
        );
        assert!(
            Snapshot::default()
                .report_lines()
                .iter()
                .all(|l| l.ends_with("unavailable"))
        );
    }
    #[test]
    fn no_swap() {
        let snapshot = Snapshot {
            swap: Reading::Value(Usage { used: 0, total: 0 }),
            ..Snapshot::default()
        };
        assert!(snapshot.report_lines().contains(&"Swap: none".into()));
    }
    #[test]
    fn mount_labels_are_plain() {
        assert_eq!(plain_label("/disk\n\u{1b}[31m"), "/disk\\n\\u{1b}[31m");
    }
}
