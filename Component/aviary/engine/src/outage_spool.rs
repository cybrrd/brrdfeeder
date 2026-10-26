// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Bounded append-only custody of opaque v4 messages. Read batches are leases:
//! retry never prepends or consumes capacity; only commit advances the cursor.
use crate::nats_publisher::Outbound;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const DEFAULT_CAP: u64 = 2 * 1024 * 1024 * 1024;
const SEGMENT_BYTES: u64 = 1024 * 1024;
const MAX_RECORD: usize = 1024 * 1024;
const HEADER: usize = 46;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub directory: PathBuf,
    pub cap_bytes: u64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            directory: "/var/lib/brrdfeeder/outage".into(),
            cap_bytes: DEFAULT_CAP,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cursor {
    segment: u64,
    offset: u64,
}
struct Segment {
    id: u64,
    bytes: u64,
    valid: u64,
    corrupt: bool,
}
pub struct Batch {
    pub messages: Vec<Outbound>,
    end: Cursor,
}
pub struct Spool {
    root: PathBuf,
    _lock: File,
    directory: File,
    segments: VecDeque<Segment>,
    cursor: Cursor,
    next: u64,
    cap: u64,
    bytes: u64,
    pub corrupt: bool,
    pub max_sync_us: u64,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn open(path: &Path) -> io::Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !f.metadata()?.is_file() {
        return Err(invalid("spool entry is not a regular file"));
    }
    Ok(f)
}
pub fn atomic(root: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    let tmp = root.join(format!("{name}.tmp"));
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&tmp)?;
    if !f.metadata()?.is_file() {
        return Err(invalid("spool temporary is not regular"));
    }
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(tmp, root.join(name))?;
    File::open(root)?.sync_all()
}
fn path(root: &Path, id: u64) -> PathBuf {
    root.join(format!("{id:020}.seg"))
}
fn encode(msg: &Outbound) -> io::Result<Vec<u8>> {
    let subject = msg.subject.as_bytes();
    let len = HEADER + subject.len() + msg.payload.len();
    if subject.len() > u16::MAX as usize || len > MAX_RECORD {
        return Err(invalid("oversized spool record"));
    }
    let mut raw = Vec::with_capacity(len);
    raw.extend_from_slice(&(len as u32).to_le_bytes());
    raw.extend_from_slice(&(subject.len() as u16).to_le_bytes());
    raw.extend_from_slice(&msg.capture_ms.to_le_bytes());
    raw.extend_from_slice(&[0; 32]);
    raw.extend_from_slice(subject);
    raw.extend_from_slice(&msg.payload);
    let mut hash = Sha256::new();
    hash.update(&raw[..14]);
    hash.update(&raw[HEADER..]);
    raw[14..HEADER].copy_from_slice(&hash.finalize());
    Ok(raw)
}
struct RecordHeader {
    raw: [u8; HEADER],
    len: usize,
    subject_len: usize,
}
fn read_header(f: &mut File) -> io::Result<Option<RecordHeader>> {
    let mut raw = [0u8; HEADER];
    if f.read(&mut raw[..1])? == 0 {
        return Ok(None);
    }
    f.read_exact(&mut raw[1..])?;
    let len = u32::from_le_bytes(raw[..4].try_into().map_err(|_| invalid("length"))?) as usize;
    let subject_len = u16::from_le_bytes([raw[4], raw[5]]) as usize;
    if !(HEADER..=MAX_RECORD).contains(&len) || subject_len > len - HEADER {
        return Err(invalid("corrupt length"));
    }
    Ok(Some(RecordHeader {
        raw,
        len,
        subject_len,
    }))
}
fn read_body(f: &mut File, header: RecordHeader) -> io::Result<(Outbound, u64)> {
    let mut body = vec![0; header.len - HEADER];
    f.read_exact(&mut body)?;
    let mut hash = Sha256::new();
    hash.update(&header.raw[..14]);
    hash.update(&body);
    if hash.finalize().as_slice() != &header.raw[14..] {
        return Err(invalid("corrupt checksum"));
    }
    let subject = String::from_utf8(body[..header.subject_len].to_vec())
        .map_err(|_| invalid("subject encoding"))?;
    let capture_ms = u64::from_le_bytes(
        header.raw[6..14]
            .try_into()
            .map_err(|_| invalid("timestamp"))?,
    );
    Ok((
        Outbound {
            subject,
            payload: body[header.subject_len..].to_vec().into(),
            capture_ms,
        },
        header.len as u64,
    ))
}
fn read_record(f: &mut File) -> io::Result<Option<(Outbound, u64)>> {
    match read_header(f)? {
        Some(header) => read_body(f, header).map(Some),
        None => Ok(None),
    }
}
impl Spool {
    pub fn open(config: &Config) -> io::Result<Self> {
        if !config.directory.is_absolute()
            || config.directory.file_name().is_none()
            || config.cap_bytes < HEADER as u64
        {
            return Err(invalid("invalid spool configuration"));
        }
        std::fs::create_dir_all(&config.directory)?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&config.directory)?;
        directory.set_permissions(std::fs::Permissions::from_mode(0o700))?;
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(config.directory.join("lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let cursor = match open(&config.directory.join("cursor.json")) {
            Ok(f) => serde_json::from_reader(f.take(4097))
                .map_err(|_| invalid("corrupt cursor retained"))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Cursor::default(),
            Err(e) => return Err(e),
        };
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&config.directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(id) = name.strip_suffix(".seg") {
                if id.len() != 20 {
                    return Err(invalid("unrecognized segment retained"));
                }
                ids.push(id.parse::<u64>().map_err(|_| invalid("segment name"))?);
            }
        }
        ids.sort_unstable();
        let mut spool = Self {
            root: config.directory.clone(),
            _lock: lock,
            directory,
            segments: VecDeque::new(),
            cursor,
            next: cursor.segment.saturating_add(1).max(1),
            cap: config.cap_bytes,
            bytes: 0,
            corrupt: false,
            max_sync_us: 0,
        };
        for id in ids {
            let mut f = open(&path(&spool.root, id))?;
            let bytes = f.metadata()?.len();
            let mut valid = 0;
            let mut corrupt = false;
            let mut cursor_valid = cursor.segment != id || cursor.offset == 0;
            loop {
                match read_record(&mut f) {
                    Ok(Some((_, n))) => {
                        valid += n;
                        if cursor.offset == valid {
                            cursor_valid = true;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        corrupt = true;
                        break;
                    }
                }
            }
            if !cursor_valid {
                return Err(invalid("cursor is not a record boundary; retained"));
            }
            spool.corrupt |= corrupt;
            spool.bytes = spool
                .bytes
                .checked_add(bytes)
                .ok_or_else(|| invalid("size overflow"))?;
            spool.next = spool.next.max(
                id.checked_add(1)
                    .ok_or_else(|| invalid("segment id exhausted"))?,
            );
            spool.segments.push_back(Segment {
                id,
                bytes,
                valid,
                corrupt,
            });
        }
        Ok(spool)
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn append(&mut self, msg: &Outbound) -> io::Result<bool> {
        let started = Instant::now();
        let raw = encode(msg)?;
        let len = raw.len() as u64;
        if len > self.cap.saturating_sub(self.bytes) {
            return Ok(false);
        }
        let rotate = self.segments.back().is_none_or(|s| {
            s.corrupt || s.bytes + len > SEGMENT_BYTES || s.id < self.cursor.segment
        });
        if rotate {
            // Bound metadata too when repeated partial I/O leaves many sealed
            // fragments. Keep every fragment; refuse additions until repaired.
            if self.segments.len() as u64 >= (self.cap / SEGMENT_BYTES).max(1) + 2 {
                return Err(invalid(
                    "too many retained segments; operator repair required",
                ));
            }
            let id = self.next;
            self.next = self
                .next
                .checked_add(1)
                .ok_or_else(|| invalid("segment id exhausted"))?;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path(&self.root, id))?
                .sync_all()?;
            self.directory.sync_all()?;
            self.segments.push_back(Segment {
                id,
                bytes: 0,
                valid: 0,
                corrupt: false,
            });
        }
        let segment = self
            .segments
            .back_mut()
            .ok_or_else(|| invalid("missing active segment"))?;
        let mut file = OpenOptions::new()
            .append(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path(&self.root, segment.id))?;
        if let Err(e) = file.write_all(&raw).and_then(|_| file.sync_data()) {
            // A partial write is evidence. Seal it; never truncate or overwrite it.
            let actual = file.metadata()?.len();
            self.bytes += actual.saturating_sub(segment.bytes);
            segment.bytes = actual;
            segment.corrupt = actual > segment.valid;
            self.corrupt |= segment.corrupt;
            return Err(e);
        }
        self.max_sync_us = self
            .max_sync_us
            .max(started.elapsed().as_micros().min(u64::MAX as u128) as u64);
        segment.bytes += len;
        segment.valid += len;
        self.bytes += len;
        Ok(true)
    }
    /// Bound stored bytes (header + subject + payload) before allocating each
    /// body. A too-small budget refuses instead of silently stalling replay.
    pub fn peek(&self, count: usize, max_bytes: usize) -> io::Result<Batch> {
        let mut used = 0;
        let mut batch = Batch {
            messages: Vec::new(),
            end: self.cursor,
        };
        for s in &self.segments {
            if s.id < self.cursor.segment {
                continue;
            }
            let mut offset = if s.id == self.cursor.segment {
                self.cursor.offset
            } else {
                0
            };
            let mut f = open(&path(&self.root, s.id))?;
            f.seek(SeekFrom::Start(offset))?;
            while offset < s.valid && batch.messages.len() < count {
                let header =
                    read_header(&mut f)?.ok_or_else(|| invalid("segment shortened; retained"))?;
                if header.len > max_bytes - used {
                    if batch.messages.is_empty() {
                        return Err(invalid("replay byte budget cannot fit next record"));
                    }
                    return Ok(batch);
                }
                used += header.len;
                let (msg, n) = read_body(&mut f, header)?;
                offset += n;
                batch.messages.push(msg);
                batch.end = Cursor {
                    segment: s.id,
                    offset,
                };
            }
            if batch.messages.len() == count {
                break;
            }
        }
        Ok(batch)
    }
    pub fn commit(&mut self, batch: &Batch) -> io::Result<()> {
        if batch.messages.is_empty() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&batch.end)?;
        atomic(&self.root, "cursor.json", &bytes)?;
        self.cursor = batch.end;
        // Cursor is durable BEFORE deletion. Crash may replay duplicates, never
        // forget an unconfirmed batch. Corrupt originals remain byte-identical.
        let mut kept = VecDeque::new();
        while let Some(s) = self.segments.pop_front() {
            if !s.corrupt
                && (s.id < self.cursor.segment
                    || (s.id == self.cursor.segment && s.bytes == self.cursor.offset))
            {
                if let Err(e) = std::fs::remove_file(path(&self.root, s.id)) {
                    kept.push_back(s);
                    kept.append(&mut self.segments);
                    self.segments = kept;
                    return Err(e);
                }
                self.bytes -= s.bytes;
            } else {
                kept.push_back(s);
            }
        }
        self.segments = kept;
        self.directory.sync_all()
    }
}
#[cfg(test)]
#[path = "outage_spool_tests.rs"]
mod tests;
