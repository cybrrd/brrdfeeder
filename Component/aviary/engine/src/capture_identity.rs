// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Name-independent USB capture selection. Observers never administer a radio.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use serde::Serialize;

pub const RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selector {
    Interface(String),
    Usb(Option<String>),
}

impl Selector {
    pub fn parse(interface: Option<&str>, usb_id: Option<&str>) -> Result<Self, String> {
        match (interface, usb_id) {
            (Some(name), None) if valid_interface(name) => Ok(Self::Interface(name.into())),
            (None, Some("auto")) => Ok(Self::Usb(None)),
            (None, Some(id)) if valid_usb_id(id) => Ok(Self::Usb(Some(id.to_ascii_lowercase()))),
            _ => Err("capture requires exactly one valid interface name or adapter_usb_id (auto or vvvv:pppp); existing interface selections are never converted".into()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Interface(name) => name.clone(),
            Self::Usb(id) => format!("USB({})", id.as_deref().unwrap_or("auto")),
        }
    }
}

fn valid_interface(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

fn valid_usb_id(id: &str) -> bool {
    id.len() == 9
        && id.as_bytes()[4] == b':'
        && id
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || b.is_ascii_hexdigit())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Adapter {
    pub interface: String,
    pub ifindex: u32,
    pub phy: u32,
    pub usb_id: String,
    pub driver: String,
    pub usb_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SelectionStatus {
    pub selector: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter: Option<Adapter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

type Capabilities = Arc<dyn Fn() -> Result<BTreeSet<u32>, String> + Send + Sync>;
type RouteGuard = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

pub struct Identity {
    root: PathBuf,
    wanted: Option<String>,
    capabilities: Capabilities,
    route_guard: RouteGuard,
    status: ArcSwap<SelectionStatus>,
}

impl Identity {
    pub fn new(wanted: Option<String>) -> Arc<Self> {
        Arc::new(Self::with_observers(
            PathBuf::from("/sys"),
            wanted,
            Arc::new(crate::nl80211::monitor_capable_phys),
            Arc::new(crate::route_guard::ensure_usb_capture_is_not_default_route),
        ))
    }

    fn with_observers(
        root: PathBuf,
        wanted: Option<String>,
        capabilities: Capabilities,
        route_guard: RouteGuard,
    ) -> Self {
        let selector = Selector::Usb(wanted.clone()).label();
        Self { root, wanted, capabilities, route_guard,
            status: ArcSwap::from_pointee(SelectionStatus { selector, adapter: None,
                reason: Some("Waiting for a USB Wi-Fi capture adapter with a driver and monitor-mode support".into()) }) }
    }

    pub fn snapshot(&self) -> Arc<SelectionStatus> {
        self.status.load_full()
    }

    pub fn wait(&self, reason: String) {
        let old = self.snapshot();
        if old.adapter.is_some() || old.reason.as_ref() != Some(&reason) {
            eprintln!("[capture] {reason}");
            self.status.store(Arc::new(SelectionStatus {
                selector: old.selector.clone(),
                adapter: None,
                reason: Some(reason),
            }));
        }
    }

    pub fn bind(&self, adapter: Adapter) {
        let old = self.snapshot();
        if old.adapter.as_ref() != Some(&adapter) || old.reason.is_some() {
            println!(
                "[capture] selected {}: USB {} driver {} phy {} ifindex {}",
                adapter.interface, adapter.usb_id, adapter.driver, adapter.phy, adapter.ifindex
            );
            self.status.store(Arc::new(SelectionStatus {
                selector: old.selector.clone(),
                adapter: Some(adapter),
                reason: None,
            }));
        }
    }

    /// Full rescan: ambiguity is refused before route checking, never resolved
    /// by keeping an old pick or excluding an uplink from the candidate count.
    pub fn resolve(&self) -> Result<Adapter, String> {
        let capable = (self.capabilities)()
            .map_err(|e| format!("Cannot verify Wi-Fi monitor capability: {e}; retrying"))?;
        let entries = std::fs::read_dir(self.root.join("class/net"))
            .map_err(|e| format!("Cannot inventory USB Wi-Fi adapters: {e}"))?;
        let mut candidates = Vec::new();
        let mut reasons = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("Incomplete Wi-Fi inventory: {e}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.path().join("phy80211").exists() {
                continue;
            }
            match observe(&self.root, &name) {
                Ok(Some(adapter)) => {
                    if self.wanted.as_ref().is_some_and(|id| id != &adapter.usb_id) {
                        reasons.push(format!("{name}: USB {} does not match requested {}", adapter.usb_id, self.wanted.as_deref().unwrap_or("auto")));
                    } else if !capable.contains(&adapter.phy) {
                        reasons.push(format!("{name}: USB {} driver {} does not advertise monitor-mode support", adapter.usb_id, adapter.driver));
                    } else { candidates.push(adapter); }
                }
                Ok(None) => reasons.push(format!("{name}: not a USB Wi-Fi adapter")),
                // An incompletely observed wireless device could be a second
                // candidate. Do not guess around unreadable or racing sysfs.
                Err(e) => return Err(format!("Cannot verify Wi-Fi adapter {name}: {e}; retrying without selecting another adapter")),
            }
        }
        candidates.sort_by(|a, b| a.interface.cmp(&b.interface));
        if candidates.len() > 1 {
            return Err(format!("Ambiguous USB Wi-Fi capture adapters: {}. Configure capture.interface explicitly; refusing to guess", candidates.iter().map(|a| format!("{} ({} / {})", a.interface, a.usb_id, a.driver)).collect::<Vec<_>>().join(", ")));
        }
        let adapter = candidates.pop().ok_or_else(|| format!("Waiting for {}USB Wi-Fi capture adapter with a bound driver and monitor-mode support{}", self.wanted.as_ref().map(|id| format!("{id} ")).unwrap_or_default(), if reasons.is_empty() { String::new() } else { format!(": {}", reasons.join("; ")) }))?;
        (self.route_guard)(&adapter.interface)?;
        Ok(adapter)
    }

    /// Recheck identity and route immediately before administration. Never
    /// resolve a new device here: only capture's outer loop may bind one.
    pub fn authorize(&self, expected: &Adapter) -> Result<(), String> {
        let observed = observe(&self.root, &expected.interface)?;
        if observed.as_ref() != Some(expected) {
            return Err(format!(
                "USB capture identity changed for {}; waiting for a fresh binding",
                expected.interface
            ));
        }
        (self.route_guard)(&expected.interface)
    }

    pub fn authorize_index(&self, ifindex: u32) -> Result<(), String> {
        let before = self.snapshot();
        let adapter = before
            .adapter
            .as_ref()
            .filter(|a| a.ifindex == ifindex)
            .ok_or_else(|| "USB capture has no current authorized binding".to_string())?;
        self.authorize(adapter)?;
        if self.snapshot().adapter.as_ref() != Some(adapter) {
            return Err("USB capture binding was invalidated before radio administration".into());
        }
        Ok(())
    }

    /// The same preparation path is exercised with mocked netlink in tests.
    /// A failed capability/identity/route gate never invokes administration.
    pub fn prepare_with(
        &self,
        establish: impl FnOnce(&Adapter) -> Result<(), String>,
    ) -> Result<Adapter, String> {
        let adapter = self.resolve()?;
        self.authorize(&adapter)?;
        establish(&adapter)?;
        if self.resolve()? != adapter {
            return Err(
                "USB capture selection changed during monitor establishment; retrying".into(),
            );
        }
        self.authorize(&adapter)?;
        self.bind(adapter.clone());
        Ok(adapter)
    }

    /// Independent of packets: a busy or never-successful capture cannot hide
    /// removal, renaming or added ambiguity. Tests use a short injected period.
    pub async fn wait_for_change(
        self: Arc<Self>,
        expected: Adapter,
        interval: std::time::Duration,
    ) -> String {
        loop {
            tokio::time::sleep(interval).await;
            let observer = Arc::clone(&self);
            match tokio::task::spawn_blocking(move || observer.resolve()).await {
                Ok(Ok(current)) if current == expected => {}
                Ok(Ok(_)) => return "USB capture adapter identity changed; rebinding".into(),
                Ok(Err(e)) => return e,
                Err(e) => return format!("USB capture rescan failed: {e}"),
            }
        }
    }
}

fn attr(path: impl AsRef<Path>) -> Result<String, String> {
    std::fs::read_to_string(path.as_ref())
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("{}: {e}", path.as_ref().display()))
}

fn positive_index(path: impl AsRef<Path>) -> Result<u32, String> {
    let text = attr(path)?;
    text.parse::<u32>()
        .ok()
        .filter(|i| *i > 0)
        .ok_or_else(|| format!("invalid interface index {text}"))
}

fn event_value<'a>(event: &'a str, key: &str) -> Option<&'a str> {
    event.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix('='))
    })
}

fn observe(root: &Path, name: &str) -> Result<Option<Adapter>, String> {
    if !valid_interface(name) {
        return Err("invalid interface name".into());
    }
    let net = root.join("class/net").join(name);
    let device = std::fs::canonicalize(net.join("device"))
        .map_err(|e| format!("device ancestry unavailable: {e}"))?;
    let devices = std::fs::canonicalize(root.join("devices")).map_err(|e| e.to_string())?;
    if !device.starts_with(&devices) {
        return Err("device ancestry escapes sysfs devices".into());
    }
    let mut usb = None;
    for ancestor in device.ancestors().take_while(|p| p.starts_with(&devices)) {
        if ancestor.join("idVendor").exists() || ancestor.join("idProduct").exists() {
            let id = format!(
                "{}:{}",
                attr(ancestor.join("idVendor"))?,
                attr(ancestor.join("idProduct"))?
            )
            .to_ascii_lowercase();
            if !valid_usb_id(&id) {
                return Err("invalid USB idVendor/idProduct".into());
            }
            let event = attr(ancestor.join("uevent"))?;
            let product = event_value(&event, "PRODUCT").ok_or("USB PRODUCT uevent missing")?;
            let parts: Vec<_> = product.split('/').collect();
            if event_value(&event, "DEVTYPE") != Some("usb_device")
                || parts.len() != 3
                || u16::from_str_radix(parts[0], 16).ok() != u16::from_str_radix(&id[..4], 16).ok()
                || u16::from_str_radix(parts[1], 16).ok() != u16::from_str_radix(&id[5..], 16).ok()
            {
                return Err("USB identity and kernel uevent disagree".into());
            }
            usb = Some((ancestor.to_path_buf(), id));
            break;
        }
    }
    let Some((usb_path, usb_id)) = usb else {
        return Ok(None);
    };
    let driver_path = std::fs::canonicalize(device.join("driver"))
        .map_err(|e| format!("USB Wi-Fi driver is not bound: {e}"))?;
    let driver_root =
        std::fs::canonicalize(root.join("bus/usb/drivers")).map_err(|e| e.to_string())?;
    if driver_path.parent() != Some(driver_root.as_path()) {
        return Err("driver is not a USB driver".into());
    }
    let driver = driver_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("USB driver name unavailable")?
        .to_string();
    let event = attr(device.join("uevent"))?;
    if event_value(&event, "DEVTYPE") != Some("usb_interface")
        || event_value(&event, "DRIVER") != Some(driver.as_str())
    {
        return Err("USB driver link and kernel uevent disagree".into());
    }
    let ifindex = positive_index(net.join("ifindex"))?;
    let phy = attr(net.join("phy80211/index"))?
        .parse::<u32>()
        .map_err(|e| format!("invalid wiphy index: {e}"))?;
    Ok(Some(Adapter {
        interface: name.into(),
        ifindex,
        phy,
        usb_id,
        driver,
        usb_path,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    pub(crate) struct Tree(pub PathBuf);
    impl Tree {
        pub(crate) fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "capture-identity-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            for path in ["devices", "class/net", "bus/usb/drivers"] {
                fs::create_dir_all(root.join(path)).unwrap();
            }
            Self(root)
        }
        pub(crate) fn add(
            &self,
            name: &str,
            port: &str,
            index: u32,
            phy: u32,
            id: &str,
            driver: &str,
        ) {
            let usb = self.0.join("devices/usb1").join(port);
            let device = usb.join(format!("{port}:1.0"));
            let net = device.join("net").join(name);
            let phy_dir = device.join("ieee80211").join(format!("phy{phy}"));
            let driver_dir = self.0.join("bus/usb/drivers").join(driver);
            for dir in [&net, &phy_dir, &driver_dir] {
                fs::create_dir_all(dir).unwrap();
            }
            fs::write(usb.join("idVendor"), &id[..4]).unwrap();
            fs::write(usb.join("idProduct"), &id[5..]).unwrap();
            fs::write(
                usb.join("uevent"),
                format!(
                    "DEVTYPE=usb_device\nPRODUCT={}/{}/100\n",
                    &id[..4],
                    &id[5..]
                ),
            )
            .unwrap();
            fs::write(
                device.join("uevent"),
                format!("DEVTYPE=usb_interface\nDRIVER={driver}\n"),
            )
            .unwrap();
            if !device.join("driver").exists() {
                symlink(&driver_dir, device.join("driver")).unwrap();
            }
            fs::write(net.join("ifindex"), index.to_string()).unwrap();
            fs::write(phy_dir.join("index"), phy.to_string()).unwrap();
            symlink(&device, net.join("device")).unwrap();
            symlink(&phy_dir, net.join("phy80211")).unwrap();
            symlink(&net, self.0.join("class/net").join(name)).unwrap();
        }
        pub(crate) fn identity(&self, wanted: Option<&str>) -> Arc<Identity> {
            Arc::new(Identity::with_observers(
                self.0.clone(),
                wanted.map(str::to_string),
                Arc::new(|| Ok((0..32).collect())),
                Arc::new(|_| Ok(())),
            ))
        }
        fn net(&self, name: &str) -> PathBuf {
            self.0.join("class/net").join(name)
        }
        fn remove(&self, name: &str) {
            fs::remove_file(self.net(name)).unwrap();
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn wifi_selector_keeps_explicit_name_and_refuses_conflicting_configuration() {
        assert_eq!(
            Selector::parse(Some("wlan1"), None).unwrap(),
            Selector::Interface("wlan1".into())
        );
        assert_eq!(
            Selector::parse(None, Some("0E8D:7961")).unwrap(),
            Selector::Usb(Some("0e8d:7961".into()))
        );
        assert_eq!(
            Selector::parse(None, Some("auto")).unwrap(),
            Selector::Usb(None)
        );
        for (name, id) in [
            (None, None),
            (Some("wlan1"), Some("auto")),
            (Some(""), None),
            (Some("../wlan1"), None),
            (None, Some("0e8d:xyz1")),
            (None, Some("")),
        ] {
            assert!(Selector::parse(name, id).is_err());
        }
    }

    #[test]
    fn wifi_absent_add_remove_and_renamed_reindexed_replacement_recovers() {
        let tree = Tree::new();
        let identity = tree.identity(None);
        assert!(identity.resolve().unwrap_err().contains("Waiting"));
        tree.add("wlan1", "1-1", 5, 0, "0e8d:7961", "mt7921u");
        let first = identity
            .prepare_with(|a| {
                assert_eq!(a.ifindex, 5);
                Ok(())
            })
            .unwrap();
        tree.remove("wlan1");
        assert!(identity.authorize(&first).is_err());
        assert!(identity.resolve().is_err());
        tree.add("wlxnew", "1-2", 9, 1, "0bda:8812", "rtw88_8812au");
        let second = identity
            .prepare_with(|a| {
                assert_eq!(a.interface, "wlxnew");
                Ok(())
            })
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(second.ifindex, 9);
        assert!(identity.authorize_index(5).is_err());
        assert!(identity.authorize_index(9).is_ok());
    }

    #[test]
    fn wifi_two_capable_adapters_or_two_interfaces_never_administers_a_first_match() {
        for same_device in [false, true] {
            let tree = Tree::new();
            tree.add("wlan1", "1-1", 5, 0, "0e8d:7961", "mt7921u");
            tree.add(
                "wlan2",
                if same_device { "1-1" } else { "1-2" },
                6,
                if same_device { 0 } else { 1 },
                "0e8d:7961",
                "mt7921u",
            );
            let identity = tree.identity(None);
            let mut calls = 0;
            let reason = identity
                .prepare_with(|_| {
                    calls += 1;
                    Ok(())
                })
                .unwrap_err();
            assert!(
                reason.contains("Ambiguous")
                    && reason.contains("wlan1")
                    && reason.contains("wlan2")
            );
            assert_eq!(calls, 0);
            assert!(identity.snapshot().adapter.is_none());
        }
    }

    #[test]
    fn wifi_pin_never_adopts_a_different_model() {
        let tree = Tree::new();
        tree.add("wlan2", "1-2", 6, 1, "0bda:8812", "rtw88_8812au");
        let identity = tree.identity(Some("0e8d:7961"));
        let error = identity
            .prepare_with(|_| panic!("wrong model must not be administered"))
            .unwrap_err();
        assert!(error.contains("does not match requested 0e8d:7961"));
        tree.add("wlxnew", "1-1", 9, 0, "0e8d:7961", "mt7921u");
        assert_eq!(identity.resolve().unwrap().interface, "wlxnew");
    }

    #[test]
    fn wifi_unsupported_unknown_and_missing_driver_refuse_without_administration() {
        let tree = Tree::new();
        tree.add("wlan1", "1-1", 5, 0, "0e8d:7961", "mt7921u");
        for caps in [Ok(BTreeSet::new()), Err("mock netlink unavailable".into())] {
            let identity = Identity::with_observers(
                tree.0.clone(),
                None,
                Arc::new(move || caps.clone()),
                Arc::new(|_| Ok(())),
            );
            assert!(identity
                .prepare_with(|_| panic!("unverified capability must not be administered"))
                .is_err());
        }
        fs::remove_file(tree.net("wlan1").join("device/driver")).unwrap();
        assert!(tree
            .identity(None)
            .prepare_with(|_| panic!("no driver must not be administered"))
            .unwrap_err()
            .contains("not bound"));
    }

    #[test]
    fn wifi_usb_uevent_and_driver_disagreement_refuse() {
        let tree = Tree::new();
        tree.add("wlan1", "1-1", 5, 0, "0e8d:7961", "mt7921u");
        let device = fs::canonicalize(tree.net("wlan1").join("device")).unwrap();
        fs::write(
            device.join("uevent"),
            "DEVTYPE=usb_interface\nDRIVER=other\n",
        )
        .unwrap();
        assert!(tree
            .identity(None)
            .resolve()
            .unwrap_err()
            .contains("disagree"));
        fs::write(
            device.join("uevent"),
            "DEVTYPE=usb_interface\nDRIVER=mt7921u\n",
        )
        .unwrap();
        fs::write(
            device.parent().unwrap().join("uevent"),
            "DEVTYPE=usb_device\nPRODUCT=0bda/8812/100\n",
        )
        .unwrap();
        assert!(tree
            .identity(None)
            .resolve()
            .unwrap_err()
            .contains("disagree"));
    }

    #[test]
    fn wifi_builtin_radio_is_not_a_usb_candidate() {
        let tree = Tree::new();
        let net = tree.net("wlan0");
        fs::create_dir_all(&net).unwrap();
        let device = tree.0.join("devices/platform/wifi");
        fs::create_dir_all(&device).unwrap();
        symlink(&device, net.join("device")).unwrap();
        fs::create_dir(net.join("phy80211")).unwrap();
        assert!(tree
            .identity(None)
            .resolve()
            .unwrap_err()
            .contains("not a USB"));
    }

    #[test]
    fn wifi_uplink_guard_runs_before_admin_and_again_on_every_authorization() {
        let tree = Tree::new();
        tree.add("wlan1", "1-1", 5, 0, "0e8d:7961", "mt7921u");
        let blocked = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let guard = blocked.clone();
        let identity = Identity::with_observers(
            tree.0.clone(),
            None,
            Arc::new(|| Ok([0].into_iter().collect())),
            Arc::new(move |_| {
                if guard.load(Ordering::Relaxed) {
                    Err("default-route uplink".into())
                } else {
                    Ok(())
                }
            }),
        );
        assert!(identity
            .prepare_with(|_| panic!("uplink must never be administered"))
            .is_err());
        blocked.store(false, Ordering::Relaxed);
        identity.prepare_with(|_| Ok(())).unwrap();
        blocked.store(true, Ordering::Relaxed);
        assert!(identity.authorize_index(5).is_err());
    }

    #[test]
    fn wifi_same_name_and_reused_index_do_not_authorize_a_different_device_or_driver() {
        let tree = Tree::new();
        tree.add("wlan1", "1-1", 5, 0, "0e8d:7961", "mt7921u");
        let identity = tree.identity(None);
        let before = identity.prepare_with(|_| Ok(())).unwrap();
        tree.remove("wlan1");
        tree.add("wlan1", "1-2", 5, 0, "0bda:8812", "rtw88_8812au");
        assert!(identity.authorize(&before).is_err());
        assert!(identity.authorize_index(5).is_err());
    }

    #[test]
    fn wifi_change_during_mocked_monitor_setup_is_not_bound() {
        let tree = Tree::new();
        tree.add("wlan1", "1-1", 5, 0, "0e8d:7961", "mt7921u");
        let identity = tree.identity(None);
        let error = identity
            .prepare_with(|_| {
                tree.add("wlan2", "1-2", 6, 1, "0e8d:7961", "mt7921u");
                Ok(())
            })
            .unwrap_err();
        assert!(error.contains("Ambiguous"));
        assert!(identity.snapshot().adapter.is_none());
    }

    #[tokio::test]
    async fn wifi_active_rescan_detects_added_ambiguity_without_waiting_for_a_packet() {
        let tree = Tree::new();
        tree.add("wlan1", "1-1", 5, 0, "0e8d:7961", "mt7921u");
        let identity = tree.identity(None);
        let bound = identity.prepare_with(|_| Ok(())).unwrap();
        let watcher = tokio::spawn(
            identity
                .clone()
                .wait_for_change(bound, Duration::from_millis(1)),
        );
        tree.add("wlan2", "1-2", 6, 1, "0e8d:7961", "mt7921u");
        let reason = tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .unwrap()
            .unwrap();
        assert!(reason.contains("Ambiguous"));
    }

    #[test]
    fn wifi_post_invalidation_authority_is_empty_and_open_alone_never_sets_up() {
        let tree = Tree::new();
        tree.add("wlan1", "1-1", 5, 0, "0e8d:7961", "mt7921u");
        let identity = tree.identity(None);
        identity.prepare_with(|_| Ok(())).unwrap();
        let live = crate::watchdog::CaptureLiveness::with_identity(identity.clone());
        let radio = crate::heartbeat::RadioState::new();
        live.current_ifindex.store(5, Ordering::Relaxed);
        live.mark_capture_open(&radio);
        assert_eq!(radio.get(), crate::heartbeat::RadioStatus::Recovering);
        live.invalidate_identity("Ambiguous adapters".into(), &radio);
        live.mark_packet(5, &radio);
        assert_eq!(live.current_ifindex.load(Ordering::Relaxed), 0);
        assert!(identity.authorize_index(5).is_err());
        assert_eq!(radio.get(), crate::heartbeat::RadioStatus::Error);
    }
}
