// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Loss accounting is independent of both telemetry capacity and the 32-item
//! Red queue. Preallocated alternating checksummed slots preserve exact counts
//! on a full filesystem; alerts retire only after their own broker PubAck.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;

const SLOT: usize = 64 * 1024;
const PREFIX: usize = 44;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    CaptureBackhaul,
    ProducerHandoff,
    OutageSpool,
    SubstrateAudit,
    UpwardRed,
    UpwardOperational,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Gap {
    pub boundary: Boundary,
    pub count: u64,
    pub first_capture_ms: u64,
    pub last_capture_ms: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    gaps: Vec<Gap>,
    pending: Vec<Gap>,
    #[serde(default)]
    active: Vec<Boundary>,
}
pub struct Ledger {
    file: Option<File>,
    sequence: u64,
    state: State,
    pub persistence_failed: bool,
    pub storage_fault: bool,
}
impl std::fmt::Debug for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ledger")
            .field("gaps", &self.state.gaps)
            .finish()
    }
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            file: None,
            sequence: 0,
            state: State::default(),
            persistence_failed: false,
            storage_fault: false,
        }
    }
}
fn invalid(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn decode(raw: &[u8]) -> Option<(u64, State)> {
    if raw.len() != SLOT {
        return None;
    }
    let seq = u64::from_le_bytes(raw[..8].try_into().ok()?);
    let len = u32::from_le_bytes(raw[8..12].try_into().ok()?) as usize;
    if seq == 0 || len > SLOT - PREFIX {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(&raw[..12]);
    hash.update(&raw[PREFIX..PREFIX + len]);
    if hash.finalize().as_slice() != &raw[12..PREFIX] {
        return None;
    }
    Some((
        seq,
        serde_json::from_slice(&raw[PREFIX..PREFIX + len]).ok()?,
    ))
}
impl Ledger {
    pub fn open(root: &Path) -> io::Result<Self> {
        let path = root.join("gaps.slots");
        let fresh = !path.try_exists()?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("gap store not regular"));
        }
        let mut ledger = Self {
            file: Some(file),
            ..Self::default()
        };
        if fresh {
            let file = ledger.file.as_ref().ok_or_else(|| invalid("gap file"))?;
            use std::os::fd::AsRawFd;
            // Reserve real blocks BEFORE capture starts; set_len alone is sparse.
            let rc =
                unsafe { libc::posix_fallocate(file.as_raw_fd(), 0, (2 * SLOT) as libc::off_t) };
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc));
            }
            ledger.save()?;
            File::open(root)?.sync_all()?;
        } else {
            let mut raw = Vec::new();
            ledger
                .file
                .as_ref()
                .ok_or_else(|| invalid("gap file"))?
                .take((2 * SLOT + 1) as u64)
                .read_to_end(&mut raw)?;
            if raw.len() != 2 * SLOT {
                return Err(invalid("corrupt gap file retained"));
            }
            let a = decode(&raw[..SLOT]);
            let b = decode(&raw[SLOT..]);
            let (sequence, state) = a
                .into_iter()
                .chain(b)
                .max_by_key(|(n, _)| *n)
                .ok_or_else(|| invalid("corrupt gap slots retained"))?;
            ledger.sequence = sequence;
            ledger.state = state;
        }
        Ok(ledger)
    }
    fn save(&mut self) -> io::Result<()> {
        let Some(file) = self.file.as_ref() else {
            return Ok(());
        };
        let bytes = serde_json::to_vec(&self.state)?;
        if bytes.len() > SLOT - PREFIX {
            return Err(invalid("gap ledger capacity"));
        }
        let seq = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("gap generation exhausted"))?;
        let mut raw = vec![0; PREFIX + bytes.len()];
        raw[..8].copy_from_slice(&seq.to_le_bytes());
        raw[8..12].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        raw[PREFIX..PREFIX + bytes.len()].copy_from_slice(&bytes);
        let mut hash = Sha256::new();
        hash.update(&raw[..12]);
        hash.update(&bytes);
        raw[12..PREFIX].copy_from_slice(&hash.finalize());
        file.write_all_at(&raw, (seq % 2) * SLOT as u64)?;
        file.sync_data()?;
        self.sequence = seq;
        Ok(())
    }
    pub fn record(&mut self, boundary: Boundary, capture_ms: u64) -> io::Result<()> {
        if !self.state.active.contains(&boundary) {
            self.state.active.push(boundary);
        }
        let gap = if let Some(g) = self.state.gaps.iter_mut().find(|g| g.boundary == boundary) {
            g.count = g
                .count
                .checked_add(1)
                .ok_or_else(|| invalid("gap count exhausted"))?;
            g.first_capture_ms = g.first_capture_ms.min(capture_ms);
            g.last_capture_ms = g.last_capture_ms.max(capture_ms);
            g.clone()
        } else {
            let g = Gap {
                boundary,
                count: 1,
                first_capture_ms: capture_ms,
                last_capture_ms: capture_ms,
            };
            self.state.gaps.push(g.clone());
            g
        };
        if is_decade(gap.count) {
            self.state.pending.push(gap.clone());
        }
        if is_decade(gap.count) {
            eprintln!(
                "[telemetry] coverage=impaired gap={}",
                serde_json::to_string(&gap).unwrap_or_default()
            );
        }
        let result = self.save();
        self.persistence_failed = result.is_err();
        result
    }
    pub fn pending(&self) -> Vec<Gap> {
        self.state.pending.clone()
    }
    pub fn gaps(&self) -> Vec<Gap> {
        self.state.gaps.clone()
    }
    pub fn impaired(&self) -> bool {
        self.persistence_failed || self.storage_fault || !self.state.active.is_empty()
    }
    pub fn resumed(&mut self, boundary: Boundary) -> io::Result<()> {
        if !self.state.active.contains(&boundary) {
            return Ok(());
        }
        let old = self.state.active.clone();
        self.state.active.retain(|b| *b != boundary);
        if let Err(e) = self.save() {
            self.state.active = old;
            self.persistence_failed = true;
            return Err(e);
        }
        self.persistence_failed = false;
        Ok(())
    }
    pub fn acknowledge(&mut self, gap: &Gap) -> io::Result<()> {
        let previous = self.state.clone();
        self.state.pending.retain(|g| g != gap);
        if let Err(e) = self.save() {
            self.state = previous;
            self.persistence_failed = true;
            return Err(e);
        }
        self.persistence_failed = false;
        Ok(())
    }
}
pub fn is_decade(mut n: u64) -> bool {
    if n == 0 {
        return false;
    }
    while n % 10 == 0 {
        n /= 10;
    }
    n == 1
}
