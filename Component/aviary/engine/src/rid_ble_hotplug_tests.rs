// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use super::*;
use std::cell::Cell;
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "ble-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn add(&self, index: u16, usb: &str) {
        let p = self.0.join(format!("hci{index}"));
        std::fs::create_dir_all(p.join("device")).unwrap();
        let (v, d) = usb.split_once(':').unwrap();
        std::fs::write(p.join("device/idVendor"), v).unwrap();
        std::fs::write(p.join("device/idProduct"), d).unwrap();
        std::fs::write(p.join("address"), format!("AA:BB:CC:DD:EE:{index:02X}")).unwrap();
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn hotplug_selection_matrix_and_pin_remedy() {
    let t = Tree::new();
    let auto = BleAdapterYaml::default();
    assert!(select_adapter_at(&t.0, &auto)
        .unwrap_err()
        .to_string()
        .contains("activate automatically"));
    t.add(0, "1234:5678");
    let reason = select_adapter_at(&t.0, &auto).unwrap_err().to_string();
    for word in ["1234:5678", "not supported", "0bda:876e", "0bda:a728"] {
        assert!(reason.contains(word));
    }
    t.add(1, "0bda:876e");
    assert_eq!(select_adapter_at(&t.0, &auto).unwrap().index, 1);
    let pin = BleAdapterYaml {
        usb_id: Some("0bda:a728".into()),
        bd_addr: None,
    };
    let reason = select_adapter_at(&t.0, &pin).unwrap_err().to_string();
    for word in ["Pinned", "0bda:a728", "0bda:876e", "adapter: {}"] {
        assert!(reason.contains(word));
    }
    t.add(2, "0bda:a728");
    assert!(select_adapter_at(&t.0, &auto)
        .unwrap_err()
        .to_string()
        .contains("Multiple"));
    assert_eq!(select_adapter_at(&t.0, &pin).unwrap().index, 2);
    t.add(3, "0bda:a728");
    assert!(select_adapter_at(&t.0, &pin).is_err());
    let addr = BleAdapterYaml {
        usb_id: None,
        bd_addr: Some("aa:bb:cc:dd:ee:03".into()),
    };
    assert_eq!(select_adapter_at(&t.0, &addr).unwrap().index, 3);
    std::fs::write(t.0.join("hci3/address"), "not ready").unwrap();
    assert!(select_adapter_at(&t.0, &addr).is_err());
}
#[test]
fn hotplug_conflict_latches_until_explicit_reclaim() {
    let t = Tree::new();
    let p = t.0.join("conflict");
    assert!(check_ownership(&p).is_ok());
    std::fs::write(&p, "operator").unwrap();
    let reason = check_ownership(&p).unwrap_err().to_string();
    assert!(reason.contains("ownership conflict"));
    assert!(reason.contains("enabled: false"));
    assert!(p.exists());
}
struct Runtime {
    tree: Tree,
    selections: Cell<usize>,
    captures: usize,
}
impl BleRuntime for Runtime {
    fn select(&self, id: &BleAdapterYaml) -> io::Result<Controller> {
        self.selections.set(self.selections.get() + 1);
        let result = select_adapter_at(&self.tree.0, id);
        if result.is_err() {
            self.tree.add(self.captures as u16, "0bda:876e");
        }
        result
    }
    async fn capture(
        &mut self,
        _: &RidBle,
        _: Controller,
        ctx: &SensorContext,
        _: &mpsc::Sender<Advertisement>,
        health: &Arc<ArcSwap<SensorHealth>>,
    ) -> io::Result<()> {
        self.captures += 1;
        transition(ctx, health, SensorState::Healthy, None, true);
        if self.captures == 1 {
            std::fs::remove_dir_all(self.tree.0.join("hci0")).unwrap();
            Err(io::Error::other("adapter unplugged"))
        } else {
            ctx.cancel.cancel();
            Ok(())
        }
    }
}
#[tokio::test]
async fn hotplug_supervisor_absent_add_unplug_replug_without_config_writes() {
    let (audit, mut events) = mpsc::channel(32);
    let ctx = SensorContext {
        node_id: Arc::from("fixture"),
        cancel: tokio_util::sync::CancellationToken::new(),
        substrate_audit: audit,
    };
    let sensor = RidBle(
        RidBleYaml::absent(),
        Arc::new(arc_swap::ArcSwapOption::empty()),
        Arc::new(crate::status::FrameActivity::default()),
    );
    let health = Arc::new(ArcSwap::from_pointee(SensorHealth::initializing("rid_ble")));
    let (tx, _rx) = mpsc::channel(2);
    let mut runtime = Runtime {
        tree: Tree::new(),
        selections: Cell::new(0),
        captures: 0,
    };
    let config = runtime.tree.0.join("config.yaml");
    std::fs::write(&config, "sensors: {}\n").unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        sensor.supervise(&ctx, &tx, &health, &mut runtime, Duration::from_millis(1)),
    )
    .await
    .unwrap();
    assert_eq!(runtime.captures, 2);
    assert_eq!(runtime.selections.get(), 4);
    assert_eq!(std::fs::read_to_string(config).unwrap(), "sensors: {}\n");
    assert_eq!(health.load().state, SensorState::Healthy);
    assert_eq!(health.load().error_count, 0);
    let mut states = Vec::new();
    while let Ok(event) = events.try_recv() {
        states.push(format!("{event:?}"));
    }
    assert!(states.iter().any(|s| s.contains("Waiting:")));
    assert!(states.iter().all(|s| !s.contains("Failed")));
    let mut inventory = (**sensor.1.load().as_ref().unwrap()).clone();
    inventory.refresh();
    assert_eq!(inventory.state, Some(SensorState::Healthy));
    assert_eq!(inventory.enabled, Some(true));
}
#[tokio::test]
async fn hotplug_explicit_off_never_selects_or_captures() {
    let (audit, _events) = mpsc::channel(8);
    let ctx = SensorContext {
        node_id: Arc::from("fixture"),
        cancel: tokio_util::sync::CancellationToken::new(),
        substrate_audit: audit,
    };
    let sensor = RidBle(
        RidBleYaml {
            enabled: false,
            ..Default::default()
        },
        Arc::new(arc_swap::ArcSwapOption::empty()),
        Arc::new(crate::status::FrameActivity::default()),
    );
    let health = Arc::new(ArcSwap::from_pointee(SensorHealth::initializing("rid_ble")));
    let (tx, _rx) = mpsc::channel(2);
    let mut runtime = Runtime {
        tree: Tree::new(),
        selections: Cell::new(0),
        captures: 0,
    };
    sensor
        .supervise(&ctx, &tx, &health, &mut runtime, Duration::from_millis(1))
        .await;
    assert_eq!(runtime.selections.get(), 0);
    assert_eq!(runtime.captures, 0);
    let mut inventory = (**sensor.1.load().as_ref().unwrap()).clone();
    inventory.refresh();
    let json = serde_json::to_value(inventory).unwrap();
    assert_eq!(json["enabled"], false);
    assert!(json["reason"].as_str().unwrap().contains("enabled: false"));
}

#[test]
fn hotplug_address_from_kernel_snapshot_without_sysfs_address() {
    let t = Tree::new();
    t.add(7, "0bda:876e");
    std::fs::remove_file(t.0.join("hci7/address")).unwrap();
    let mut info = [0u8; 92];
    info[..2].copy_from_slice(&7u16.to_ne_bytes());
    info[10..16].copy_from_slice(&[6, 5, 4, 3, 2, 1]);
    let c = select_adapter_with(&t.0, &BleAdapterYaml::default(), |index, _| {
        decode_controller_address(index, &info)
    })
    .unwrap();
    assert_eq!(c.address, "01:02:03:04:05:06");
    assert!(decode_controller_address(8, &info).is_err());
}

#[tokio::test]
async fn hotplug_running_auto_to_off_after_documented_restart_without_receipt() {
    struct AbsentRuntime {
        selections: Cell<usize>,
    }
    impl BleRuntime for AbsentRuntime {
        fn select(&self, _: &BleAdapterYaml) -> io::Result<Controller> {
            self.selections.set(self.selections.get() + 1);
            Err(io::Error::new(io::ErrorKind::NotFound, "no adapter"))
        }
        async fn capture(
            &mut self,
            _: &RidBle,
            _: Controller,
            _: &SensorContext,
            _: &mpsc::Sender<Advertisement>,
            _: &Arc<ArcSwap<SensorHealth>>,
        ) -> io::Result<()> {
            panic!("absent/off receiver must never capture");
        }
    }
    let tree = Tree::new();
    let config = tree.0.join("config.yaml");
    std::fs::write(&config, "{}\n").unwrap();
    let load = || {
        serde_yaml::from_str::<crate::node_config::SensorsYaml>(
            &std::fs::read_to_string(&config).unwrap(),
        )
        .unwrap()
        .rid_ble
    };
    let (audit, _events) = mpsc::channel(32);
    let mut ctx = SensorContext {
        node_id: Arc::from("fixture"),
        cancel: tokio_util::sync::CancellationToken::new(),
        substrate_audit: audit,
    };
    let inventory = Arc::new(arc_swap::ArcSwapOption::empty());
    let sensor = RidBle(
        load(),
        inventory.clone(),
        Arc::new(crate::status::FrameActivity::default()),
    );
    let health = Arc::new(ArcSwap::from_pointee(SensorHealth::initializing("rid_ble")));
    let (tx, _rx) = mpsc::channel(2);
    let mut runtime = AbsentRuntime {
        selections: Cell::new(0),
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(
            sensor.supervise(&ctx, &tx, &health, &mut runtime, Duration::from_millis(1)),
            async {
                while health.load().state != SensorState::Degraded {
                    tokio::task::yield_now().await;
                }
                assert!(sensor.0.enabled);
                std::fs::write(&config, "rid_ble: {enabled: false}\n").unwrap();
                // Editing the file cannot update the already-cloned sensor.
                assert!(sensor.0.enabled);
                ctx.cancel.cancel(); // systemctl restart first stops the old supervisor.
            }
        );
    })
    .await
    .unwrap();
    assert!(runtime.selections.get() > 0);
    let selections = runtime.selections.get();
    ctx.cancel = tokio_util::sync::CancellationToken::new();
    let restarted = RidBle(
        load(),
        inventory.clone(),
        Arc::new(crate::status::FrameActivity::default()),
    );
    restarted
        .supervise(&ctx, &tx, &health, &mut runtime, Duration::from_millis(1))
        .await;
    assert_eq!(runtime.selections.get(), selections);
    let mut local = (**inventory.load().as_ref().unwrap()).clone();
    local.refresh();
    let status = serde_json::to_value(local).unwrap();
    assert_eq!(status["enabled"], false);
    assert!(status["reason"].as_str().unwrap().contains("Disabled"));
    assert!(!tree.0.join(".bluetooth-prior.json").exists());
}
