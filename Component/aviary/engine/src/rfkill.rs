//! Targeted rfkill preflight for the identity-resolved Bluetooth controller.
//! Linux UAPI rfkill_event v1: native-endian u32 index + type/op/soft/hard.
//! Never CHANGE_ALL, never power down a controller or evict bluetoothd.
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;

/// Read-only watch of the exact kernel switch resolved for this controller.
/// An unplug/replacement invalidates it rather than borrowing another radio's state.
#[derive(Debug, Clone)]
pub struct RfkillObserver {
    path: PathBuf,
    hci: u16,
    device: u64,
    inode: u64,
}

impl RfkillObserver {
    pub fn for_controller(hci: u16) -> io::Result<Option<Self>> {
        Self::at(Path::new("/sys/class/rfkill"), hci)
    }

    pub(crate) fn at(root: &Path, hci: u16) -> io::Result<Option<Self>> {
        let mut found = None;
        for entry in std::fs::read_dir(root)? {
            let path = entry?.path();
            if std::fs::read_to_string(path.join("name"))?.trim() != format!("hci{hci}")
                || std::fs::read_to_string(path.join("type"))?.trim() != "bluetooth"
            {
                continue;
            }
            if found.is_some() {
                return Err(io::Error::other("ambiguous rfkill controller"));
            }
            let meta = std::fs::metadata(&path)?;
            found = Some(Self {
                path,
                hci,
                device: meta.dev(),
                inode: meta.ino(),
            });
        }
        Ok(found)
    }

    pub fn read(&self) -> Option<(bool, bool)> {
        let meta = std::fs::metadata(&self.path).ok()?;
        if (meta.dev(), meta.ino()) != (self.device, self.inode)
            || std::fs::read_to_string(self.path.join("name")).ok()?.trim()
                != format!("hci{}", self.hci)
            || std::fs::read_to_string(self.path.join("type")).ok()?.trim() != "bluetooth"
        {
            return None;
        }
        let bit = |name| match std::fs::read_to_string(self.path.join(name)).ok()?.trim() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        };
        let value = (bit("soft")?, bit("hard")?);
        let after = std::fs::metadata(&self.path).ok()?;
        ((after.dev(), after.ino()) == (self.device, self.inode)).then_some(value)
    }
}

pub fn prepare_controller(
    hci: u16,
    unblock: bool,
    observe: &mut dyn FnMut(bool, bool),
) -> io::Result<()> {
    prepare_observed(
        Path::new("/sys/class/rfkill"),
        Path::new("/dev/rfkill"),
        hci,
        unblock,
        observe,
    )
}

#[cfg(test)]
fn prepare_at(root: &Path, device: &Path, hci: u16, unblock: bool) -> io::Result<()> {
    prepare_observed(root, device, hci, unblock, &mut |_, _| {})
}

fn prepare_observed(
    root: &Path,
    device: &Path,
    hci: u16,
    unblock: bool,
    observe: &mut dyn FnMut(bool, bool),
) -> io::Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = std::fs::read_to_string(path.join("name"))?;
        if name.trim() != format!("hci{hci}") {
            continue;
        }
        if std::fs::read_to_string(path.join("type"))?.trim() != "bluetooth" {
            continue;
        }
        let soft = std::fs::read_to_string(path.join("soft"))?;
        let hard = std::fs::read_to_string(path.join("hard"))?;
        // Retain exactly the preflight observation, not a claim about the
        // result of an unblock write or the current state on a later tick.
        observe(soft.trim() == "1", hard.trim() == "1");
        if soft.trim() == "1" {
            eprintln!("rid_ble: hci{hci} is rfkill soft-blocked — run 'rfkill unblock bluetooth' (or set sensors.rid_ble.unblock_rfkill: true)");
        }
        if hard.trim() == "1" {
            return Err(io::Error::other(format!(
                "rid_ble: hci{hci} is rfkill hard-blocked; operator action required"
            )));
        }
        if soft.trim() != "1" {
            return Ok(());
        }
        if !unblock {
            return Err(io::Error::from_raw_os_error(libc::ERFKILL));
        }
        let index = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_prefix("rfkill"))
            .and_then(|index| index.parse::<u32>().ok())
            .ok_or_else(|| io::Error::other("invalid rfkill device index"))?;
        let mut event = [0u8; 8];
        event[..4].copy_from_slice(&index.to_ne_bytes());
        event[4] = 2; // RFKILL_TYPE_BLUETOOTH
        event[5] = 2; // RFKILL_OP_CHANGE (this index only), soft=0
        let mut file = std::fs::OpenOptions::new().write(true).open(device)?;
        // rfkill is message-oriented: a short write is an error, not a stream retry.
        if file.write(&event)? != event.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short rfkill event write",
            ));
        }
        println!("[rid_ble] hci{hci} requested rfkill unblock for index {index}");
        return Ok(()); // USER bind remains the authority; it can still fail closed.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targeted_unblock_is_opt_in_and_never_clears_hard_block() {
        let root = std::env::temp_dir().join(format!("cybrrd-rfkill-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("rfkill17")).unwrap();
        let path = root.join("rfkill17");
        for (key, value) in [
            ("name", "hci6\n"),
            ("type", "bluetooth\n"),
            ("soft", "1\n"),
            ("hard", "0\n"),
        ] {
            std::fs::write(path.join(key), value).unwrap();
        }
        let device = root.join("device");
        // Device outside the fake class directory so enumeration sees only switches.
        let dev = root.with_extension("device");
        std::fs::write(&dev, []).unwrap();
        let mut observations = Vec::new();
        assert_eq!(
            prepare_observed(&root, &dev, 6, false, &mut |soft, hard| observations
                .push((soft, hard)))
            .unwrap_err()
            .raw_os_error(),
            Some(libc::ERFKILL)
        );
        assert_eq!(observations, vec![(true, false)]);
        assert!(std::fs::read(&dev).unwrap().is_empty());
        prepare_at(&root, &dev, 5, true).unwrap(); // different HCI untouched
        assert!(std::fs::read(&dev).unwrap().is_empty());
        std::fs::write(path.join("hard"), "1").unwrap();
        assert!(prepare_at(&root, &dev, 6, true)
            .unwrap_err()
            .to_string()
            .contains("hard-blocked"));
        assert!(std::fs::read(&dev).unwrap().is_empty());
        std::fs::write(path.join("hard"), "0").unwrap();
        prepare_at(&root, &dev, 6, true).unwrap();
        let mut expected = 17u32.to_ne_bytes().to_vec();
        expected.extend([2, 2, 0, 0]);
        assert_eq!(std::fs::read(&dev).unwrap(), expected);
        std::fs::write(path.join("soft"), "0").unwrap();
        prepare_at(&root, &device, 6, false).unwrap(); // no /dev access when clear
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_file(dev).unwrap();
    }
}
