// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Optional bounded pcap ring. No libpcap Savefile::write (which hides errors).
//! Files are private, fixed-name slots; mtime deliberately records slot birth
//! for age expiry across restart. Capture continues if recording fails.
use crate::{
    audit::{SubstrateAuditEvent, KITTLER_LINEAGE},
    heartbeat::RadioState,
    node_config::SavefileYaml,
};
use std::{
    fs::{self, File, FileTimes, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;

struct Active {
    file: File,
    slot: usize,
    bytes: u64,
    born: SystemTime,
    opened: Instant,
}
pub struct Ring {
    cfg: SavefileYaml,
    link: u32,
    active: Option<Active>,
    last_sync: Instant,
}
impl Ring {
    pub fn open(cfg: SavefileYaml, link: u32) -> io::Result<Self> {
        cfg.validate().map_err(io::Error::other)?;
        fs::create_dir_all(&cfg.directory)?;
        // Exact owned names only, including obsolete slots after lowering N.
        for slot in 0..16 {
            let path = cfg.directory.join(format!("capture-{slot:02}.pcap"));
            match fs::symlink_metadata(&path) {
                Ok(meta) if !meta.file_type().is_file() => {
                    return Err(io::Error::other(
                        "pcap slot must be a regular file, not a symlink",
                    ))
                }
                Ok(meta)
                    if slot >= cfg.max_files
                        || expired(meta.modified()?, cfg.max_age_secs)
                        || meta.len() > cfg.max_bytes =>
                {
                    fs::remove_file(path)?
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(Self {
            cfg,
            link,
            active: None,
            last_sync: Instant::now(),
        })
    }
    pub fn poll(&mut self) -> io::Result<()> {
        if self.active.as_ref().is_some_and(|a| {
            a.opened.elapsed() >= Duration::from_secs(self.cfg.max_age_secs)
                || expired(a.born, self.cfg.max_age_secs)
        }) {
            if let Some(a) = self.active.take() {
                a.file.sync_data()?;
                fs::remove_file(self.path(a.slot))?;
            }
        }
        for slot in 0..self.cfg.max_files {
            if self.active.as_ref().is_some_and(|a| a.slot == slot) {
                continue;
            }
            match fs::symlink_metadata(self.path(slot)) {
                Ok(m) if !m.file_type().is_file() => {
                    return Err(io::Error::other("non-regular pcap slot"))
                }
                Ok(m) if expired(m.modified()?, self.cfg.max_age_secs) => {
                    fs::remove_file(self.path(slot))?
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        if self.last_sync.elapsed() >= Duration::from_secs(1) {
            if let Some(a) = &self.active {
                a.file.sync_data()?;
            }
            self.last_sync = Instant::now();
        }
        Ok(())
    }
    fn path(&self, slot: usize) -> std::path::PathBuf {
        self.cfg.directory.join(format!("capture-{slot:02}.pcap"))
    }
    fn rotate(&mut self) -> io::Result<()> {
        if let Some(a) = self.active.take() {
            a.file.sync_data()?;
        }
        let mut chosen = 0;
        let mut oldest = SystemTime::now();
        for slot in 0..self.cfg.max_files {
            match fs::symlink_metadata(self.path(slot)) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    chosen = slot;
                    break;
                }
                Err(e) => return Err(e),
                Ok(m) if !m.file_type().is_file() => {
                    return Err(io::Error::other("non-regular pcap slot"))
                }
                Ok(m) => {
                    let birth = m.modified()?;
                    if birth <= oldest {
                        oldest = birth;
                        chosen = slot;
                    }
                }
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.path(chosen))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        let mut header = Vec::with_capacity(24);
        header.extend_from_slice(&0xa1b2c3d4u32.to_le_bytes());
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&4u16.to_le_bytes());
        header.extend_from_slice(&[0u8; 8]);
        header.extend_from_slice(&65535u32.to_le_bytes());
        header.extend_from_slice(&self.link.to_le_bytes());
        file.write_all(&header)?;
        self.active = Some(Active {
            file,
            slot: chosen,
            bytes: 24,
            born: SystemTime::now(),
            opened: Instant::now(),
        });
        Ok(())
    }
    pub fn write(&mut self, packet: &pcap::Packet<'_>) -> io::Result<()> {
        // Poll is also called on pcap timeouts, so silence cannot keep old files.
        if self.last_sync.elapsed() >= Duration::from_secs(1) {
            self.poll()?;
        }
        let length = packet.data.len().min(65535);
        let needed = 16 + length as u64;
        if needed + 24 > self.cfg.max_bytes {
            return Err(io::Error::other(
                "packet exceeds configured pcap slot budget",
            ));
        }
        if self
            .active
            .as_ref()
            .is_none_or(|a| a.bytes + needed > self.cfg.max_bytes)
        {
            self.rotate()?;
        }
        let Some(a) = self.active.as_mut() else {
            return Err(io::Error::other("pcap slot missing after rotation"));
        };
        let mut header = Vec::with_capacity(16);
        header.extend_from_slice(&(packet.header.ts.tv_sec as u32).to_le_bytes());
        header.extend_from_slice(&(packet.header.ts.tv_usec as u32).to_le_bytes());
        header.extend_from_slice(&(length as u32).to_le_bytes());
        header.extend_from_slice(&packet.header.len.to_le_bytes());
        a.file.write_all(&header)?;
        a.file.write_all(&packet.data[..length])?;
        a.bytes += needed;
        a.file.set_times(FileTimes::new().set_modified(a.born))?;
        Ok(())
    }
}
fn expired(born: SystemTime, age: u64) -> bool {
    born.elapsed()
        .map_or(true, |d| d >= Duration::from_secs(age))
}

pub struct Recorder {
    ring: Option<Ring>,
    failed: bool,
    node: String,
    radio: RadioState,
    audit: mpsc::Sender<SubstrateAuditEvent>,
}
impl Recorder {
    pub fn new(
        cfg: Option<SavefileYaml>,
        link: u32,
        node: String,
        radio: RadioState,
        audit: mpsc::Sender<SubstrateAuditEvent>,
    ) -> Self {
        let mut out = Self {
            ring: None,
            failed: false,
            node,
            radio,
            audit,
        };
        if let Some(cfg) = cfg {
            match Ring::open(cfg, link) {
                Ok(r) => {
                    out.ring = Some(r);
                    out.radio.set_forensic_failed(false);
                }
                Err(e) => out.fail(e),
            }
        }
        out
    }
    fn fail(&mut self, error: io::Error) {
        if self.failed {
            return;
        }
        self.failed = true;
        self.ring = None;
        self.radio.set_forensic_failed(true);
        eprintln!("[forensic] health healthy/initializing -> failed; recording disabled until capture restart: {error}");
        let event = SubstrateAuditEvent {
            node_id: self.node.clone(),
            timestamp_utc: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            lineage: KITTLER_LINEAGE.into(),
            event_type: "forensic_write_failed".into(),
            channel: None,
            success: Some(false),
            error_message: Some(format!(
                "forensic_pcap health=failed; Red-eligible; {error}"
            )),
            elapsed_ms: None,
        };
        if let Err(e) = self.audit.try_send(event) {
            eprintln!("[forensic] failed to enqueue health audit: {e}");
        }
    }
    pub fn poll(&mut self) {
        if let Some(r) = &mut self.ring {
            if let Err(e) = r.poll() {
                self.fail(e);
            }
        }
    }
    pub fn write(&mut self, p: &pcap::Packet<'_>) {
        if let Some(r) = &mut self.ring {
            if let Err(e) = r.write(p) {
                self.fail(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ring_size_age_restart_and_readable_pcap() {
        let dir = std::env::temp_dir().join(format!("d5-pcap-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let cfg = SavefileYaml {
            directory: dir.clone(),
            max_bytes: 65536,
            max_files: 2,
            max_age_secs: 10,
        };
        let mut ring = Ring::open(cfg.clone(), 127).unwrap();
        let data = vec![0u8; 32000];
        let hdr = pcap::PacketHeader {
            ts: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            caplen: 32000,
            len: 32000,
        };
        for _ in 0..20 {
            ring.write(&pcap::Packet {
                header: &hdr,
                data: &data,
            })
            .unwrap();
        }
        drop(ring);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        for slot in 0..2 {
            let p = dir.join(format!("capture-{slot:02}.pcap"));
            assert!(fs::metadata(&p).unwrap().len() <= 65536);
            let mut cap = pcap::Capture::from_file(&p).unwrap();
            assert_eq!(cap.next_packet().unwrap().data.len(), 32000);
            File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_times(
                    FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(11)),
                )
                .unwrap();
        }
        let _ = Ring::open(cfg, 127).unwrap();
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn errors_latch_health_and_one_red_eligible_event() {
        let (tx, mut rx) = mpsc::channel(4);
        let radio = RadioState::new();
        let mut r = Recorder::new(None, 127, "node".into(), radio.clone(), tx);
        r.fail(io::Error::from_raw_os_error(libc::ENOSPC));
        r.fail(io::Error::other("again"));
        radio.set(crate::heartbeat::RadioStatus::Up);
        assert_eq!(radio.get(), crate::heartbeat::RadioStatus::Error);
        let event = rx.try_recv().unwrap();
        assert_eq!(event.event_type, "forensic_write_failed");
        assert_eq!(event.success, Some(false));
        assert!(rx.try_recv().is_err());
    }
}
