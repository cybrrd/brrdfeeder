// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Operator-run hardware test. Default/non-root execution opens NO resources.
//! Uses a child executable with the same file caps as the image, UID 1001:20,
//! and only the three capability bounding bits. Never steps the host clock.
#![cfg(target_os = "linux")]
use caps::{CapSet, Capability};
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    process::Command,
};
#[allow(dead_code, unused_assignments, clippy::manual_is_multiple_of)]
#[path = "../src/nl80211.rs"]
mod nl80211;

#[test]
fn uid1001_privileged_resources() {
    // SAFETY: geteuid has no arguments or side effects.
    if unsafe { libc::geteuid() } != 0 || std::env::var("D5_HARDWARE_PROOF").as_deref() != Ok("1") {
        eprintln!("SKIP PRIV1 hardware: requires root plus D5_HARDWARE_PROOF=1 and dedicated test devices");
        return;
    }
    for key in [
        "D5_WIFI_INTERFACE",
        "D5_GPS_DEVICE",
        "D5_HCI_INDEX",
        "D5_RFKILL_DEVICE",
    ] {
        assert!(std::env::var(key).is_ok(), "missing {key}");
    }
    let dir = std::env::temp_dir().join(format!("d5-privilege-proof-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let executable = dir.join("probe");
    fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Command::new("setcap")
        .args(["cap_net_admin,cap_net_raw,cap_sys_time=ep"])
        .arg(&executable)
        .status()
        .unwrap()
        .success());
    let mut child = Command::new(&executable);
    child
        .args(["--exact", "privileged_resource_child", "--nocapture"])
        .env("D5_PRIV_CHILD", "1");
    // SAFETY: only async-signal-safe syscalls between fork and exec; no logging
    // or allocation in the callback. Mutation is confined to this child.
    unsafe {
        child.pre_exec(|| {
            for cap in 0..=40 {
                if ![12, 13, 25].contains(&cap)
                    && libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if libc::setgroups(0, std::ptr::null()) != 0
                || libc::setgid(20) != 0
                || libc::setuid(1001) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let status = child.status().unwrap();
    fs::remove_file(&executable).unwrap();
    fs::remove_dir(&dir).unwrap();
    assert!(status.success(), "UID/file-cap/resource proof failed");
}

#[test]
fn privileged_resource_child() {
    if std::env::var("D5_PRIV_CHILD").as_deref() != Ok("1") {
        return;
    }
    // SAFETY: read-only uid/gid checks.
    assert_eq!(unsafe { libc::geteuid() }, 1001);
    assert_eq!(unsafe { libc::getegid() }, 20);
    let expected = [
        Capability::CAP_NET_ADMIN,
        Capability::CAP_NET_RAW,
        Capability::CAP_SYS_TIME,
    ]
    .into_iter()
    .collect();
    assert_eq!(caps::read(None, CapSet::Bounding).unwrap(), expected);
    assert_eq!(caps::read(None, CapSet::Effective).unwrap(), expected);
    let iface = std::env::var("D5_WIFI_INTERFACE").unwrap();
    assert!(!iface.contains('/') && !iface.is_empty());
    let index: u32 = fs::read_to_string(format!("/sys/class/net/{iface}/ifindex"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    nl80211::establish_monitor_mode(index).unwrap();
    assert!(nl80211::interface_is_monitor(index).unwrap());
    let _pcap = pcap::Capture::from_device(iface.as_str())
        .unwrap()
        .promisc(true)
        .timeout(100)
        .open()
        .unwrap();
    use std::os::unix::fs::OpenOptionsExt;
    let _gps = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(std::env::var("D5_GPS_DEVICE").unwrap())
        .unwrap();
    let _rfkill = fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(std::env::var("D5_RFKILL_DEVICE").unwrap())
        .unwrap();
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    #[repr(C)]
    struct HciAddr {
        family: libc::sa_family_t,
        index: u16,
        channel: u16,
    }
    let index = std::env::var("D5_HCI_INDEX")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    // SAFETY: fixed Linux HCI sockaddr; OwnedFd guarantees close. No commands,
    // scans or rfkill events are sent; binding requires a dedicated DOWN HCI.
    let raw = unsafe {
        libc::socket(
            libc::AF_BLUETOOTH,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            1,
        )
    };
    assert!(raw >= 0, "HCI socket: {}", std::io::Error::last_os_error());
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let addr = HciAddr {
        family: libc::AF_BLUETOOTH as libc::sa_family_t,
        index,
        channel: 1,
    };
    assert_eq!(
        unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&addr as *const HciAddr).cast(),
                std::mem::size_of::<HciAddr>() as libc::socklen_t,
            )
        },
        0,
        "HCI USER bind: {}",
        std::io::Error::last_os_error()
    );
    println!("OBSERVED UID=1001 GID=20 exact bounding/effective caps; nl80211 monitor + pcap + GPS tty + HCI USER + rfkill open; clock NOT changed");
}

// D42 contract tests below are std-only so mutation probes can execute the exact
// assertions without running the opt-in hardware tests above.
const CUSTOMER_INSTALLER: &str = include_str!("../../../brrdfeeder/install/brrdfeeder-install.sh");

fn heredoc<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source.split_once(start).expect("missing heredoc start").1
        .split_once(end).expect("missing heredoc end").0
}

#[test]
fn deployment_contract_is_nonroot_and_devices_are_not_world_writable() {
    let quadlet = include_str!("../../deploy/quadlet/brrdfeeder-engine.container");
    assert!(quadlet.contains("User=1001:20\n"));
    assert!(quadlet.contains("DropCapability=ALL\n"));
    assert!(quadlet.contains("AddCapability=CAP_NET_ADMIN CAP_NET_RAW CAP_SYS_TIME\n"));
    assert!(!quadlet
        .lines()
        .any(|l| l == "NoNewPrivileges=true" || l == "User=0:0"));
    assert!(quadlet.contains("LimitCORE=0"));
    for rules in [
        include_str!("../../deploy/udev/99-cybrrd-brrdfeeder.rules"),
        CUSTOMER_INSTALLER,
    ] {
        assert!(!rules.contains("MODE=\"0666\""));
        assert!(rules.contains("MODE=\"0660\""));
        assert!(rules.contains("GROUP=\"dialout\""));
    }
}

#[test]
fn customer_udev_is_the_verbatim_hardened_block() {
    let start = "NEW_UDEV_CONTENT=$(cat <<EOF\n";
    let end = "\nEOF\n)";
    let rules = heredoc(CUSTOMER_INSTALLER, start, end);
    let reference = heredoc(include_str!("../../deploy/bootstrap/brrdfeeder-install.sh"), start, end);
    // P0-4 deliberately replaced the legacy single GPS ID with the tested
    // supported-family generator. Keep checking every other hardened byte.
    let gps = "# u-blox 7 GPS / GNSS receiver\nSUBSYSTEM==\"tty\", ATTRS{idVendor}==\"${UBLOX_VENDOR}\", ATTRS{idProduct}==\"${UBLOX_PRODUCT}\", SYMLINK+=\"${GPS_SYMLINK}\", GROUP=\"dialout\", MODE=\"0660\"";
    let reference = reference.replace(gps, "# Supported u-blox USB family; one connected GPS, no wildcard clone probing.\n${GPS_UDEV_RULES}");
    assert_eq!(rules, reference, "customer non-GPS udev hardening drifted");
    let products = "readonly -a UBLOX_PRODUCTS=(01a5 01a6 01a7 01a8 01a9)";
    assert_eq!(CUSTOMER_INSTALLER.matches(products).count(), 1);
    let generator = heredoc(CUSTOMER_INSTALLER, "GPS_UDEV_RULES=$(", "\nNEW_UDEV_CONTENT=");
    assert_eq!(generator, "for product in \"${UBLOX_PRODUCTS[@]}\"; do\n  printf 'SUBSYSTEM==\"tty\", ATTRS{idVendor}==\"%s\", ATTRS{idProduct}==\"%s\", SYMLINK+=\"%s\", GROUP=\"dialout\", MODE=\"0660\"\\n' \"$UBLOX_VENDOR\" \"$product\" \"$GPS_SYMLINK\"\ndone)");
    // Execute only this exact constant-checked generator: no hardware/effectors.
    let generated = std::process::Command::new("bash").arg("-c").arg(format!(
        "{products}\nUBLOX_VENDOR=1546; GPS_SYMLINK=cybrrd_gps\nGPS_UDEV_RULES=$({generator}\nprintf '%s\\n' \"$GPS_UDEV_RULES\""
    )).output().expect("bash GPS generator fixture");
    assert!(generated.status.success());
    let expected = ["01a5", "01a6", "01a7", "01a8", "01a9"].map(|product| format!(
        "SUBSYSTEM==\"tty\", ATTRS{{idVendor}}==\"1546\", ATTRS{{idProduct}}==\"{product}\", SYMLINK+=\"cybrrd_gps\", GROUP=\"dialout\", MODE=\"0660\"\n"
    )).join("");
    assert_eq!(String::from_utf8(generated.stdout).unwrap(), expected);
    let devices: Vec<_> = rules.lines().filter(|l| l.starts_with("SUBSYSTEM==") || l.starts_with("KERNEL==")).collect();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices.len() + expected.lines().count(), 7); // two fixed + five GPS
    for rule in devices {
        assert!(rule.contains("GROUP=\"dialout\", MODE=\"0660\""), "unsafe device: {rule}");
    }
    assert!(rules.contains("KERNEL==\"rfkill\", SUBSYSTEM==\"misc\""));
}

#[test]
fn customer_quadlet_has_the_nonroot_identity_contract() {
    let quadlet = heredoc(CUSTOMER_INSTALLER, "NEW_QUADLET=$(cat <<EOF\n", "\nEOF\n)");
    for line in [
        "User=${TARGET_UID}:${TARGET_GID}", "GroupAdd=${DIALOUT_GID}",
        "DropCapability=ALL", "AddCapability=CAP_NET_ADMIN CAP_NET_RAW CAP_SYS_TIME",
        "ReadOnly=true", "LimitCORE=0", "Pull=never", "Network=host",
        "PodmanArgs=--cidfile=%t/%N.cid", "Notify=false",
        "Environment=BRRDFEEDER_IDENTITY_INVOCATION=\\${INVOCATION_ID}",
        "Volume=%t/brrdfeeder-identity:/run/brrdfeeder-identity:ro",
        "RuntimeDirectory=brrdfeeder-identity", "RuntimeDirectoryMode=0755",
        "RuntimeDirectoryPreserve=no",
        "ExecStartPre=-/usr/local/libexec/brrdfeeder-image-identity prepare %t/brrdfeeder-identity",
        "ExecStartPost=-/usr/local/libexec/brrdfeeder-image-identity resolve %t/brrdfeeder-identity %t/%N.cid",
        "AddDevice=/dev/rfkill:/dev/rfkill:rw",
        "AddDevice=/run/brrdfeeder-gps/device:/dev/${GPS_SYMLINK}:rw",
        "Environment=BRRDFEEDER_GPS_TRANSPORT=/dev/${GPS_SYMLINK}",
        "ExecStartPre=/usr/local/libexec/brrdfeeder-gps-runtime prepare",
        "Volume=${SECRETS_DIR}:/etc/brrdfeeder/secrets:ro",
        "Volume=${STATE_DIR}:/var/lib/brrdfeeder",
        "Documentation=https://github.com/cybrrd/brrdfeeder",
    ] {
        assert!(quadlet.lines().any(|l| l == line), "missing customer contract: {line}");
    }
    assert!(!quadlet.lines().any(|l| l == "User=0:0" || l == "NoNewPrivileges=true"));
    assert_eq!(quadlet.lines().filter(|l| l.starts_with("User=")).count(), 1);
    assert_eq!(quadlet.lines().filter(|l| l.starts_with("AddCapability=")).count(), 1);
    assert_eq!(quadlet.lines().filter(|l| l.starts_with("Image=")).collect::<Vec<_>>(), ["Image=${CONTAINER_IMAGE}"]);
}

#[test]
fn customer_installs_the_canonical_identity_helper() {
    assert_eq!(
        heredoc(CUSTOMER_INSTALLER, "<<'IDENTITY_SH_EOF'\n", "\nIDENTITY_SH_EOF"),
        include_str!("../../deploy/bootstrap/brrdfeeder-image-identity.sh").trim_end()
    );
    assert!(CUSTOMER_INSTALLER.contains("chmod 0755 \"$IDENTITY_INSTALL\""));
}

#[test]
fn customer_identity_and_image_are_fail_closed() {
    for required in [
        "readonly TARGET_USER=\"brrdfeeder\"",
        "useradd --system --user-group --no-create-home",
        "getent passwd \"$TARGET_USER\"",
        "ACCOUNT_NAME _ TARGET_UID TARGET_GID _ TARGET_HOME TARGET_SHELL",
        "\"$TARGET_UID\" -gt 0", "\"$TARGET_GID\" -gt 0",
        "run install -d -m 0750 -o \"$TARGET_UID\" -g \"$TARGET_GID\" \"$STATE_DIR\"",
        "run chown root:\"$TARGET_GID\" \"$CREDS_PATH\"",
        "run chmod 0640 \"$CREDS_PATH\"",
        "valid_image_pin \"$CONTAINER_IMAGE\" || fatal",
        "CONTAINER_IMAGE=$INSTALLED_IMAGE",
        "\"$REQUESTED_IMAGE\" == \"$INSTALLED_IMAGE\"",
        "podman pull \"$CONTAINER_IMAGE\" || fatal",
        "podman image inspect \"$CONTAINER_IMAGE\" --format '{{range .RepoDigests}}{{println .}}{{end}}'",
        "grep -qxF \"$CONTAINER_IMAGE\"",
    ] {
        assert!(CUSTOMER_INSTALLER.contains(required), "missing safety boundary: {required}");
    }
    assert!(!CUSTOMER_INSTALLER.contains("brrdfeeder-open:latest"));
    assert!(!CUSTOMER_INSTALLER.contains("/home/operator"));
    assert!(CUSTOMER_INSTALLER.find("Pinned image RepoDigests mismatch").unwrap()
        < CUSTOMER_INSTALLER.find("NEW_QUADLET=").unwrap());
}
