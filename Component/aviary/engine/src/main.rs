// #179: Sentinel-indestructibility, compiler-enforced. Runtime (non-test) code
// must never .unwrap()/.expect() — a panic under panic=abort aborts the whole
// engine. Tests are exempt. Use ? / match / graceful logging instead.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod audit;
mod backhaul;
mod blue_policy; // #185 Phase 3 — E-CNP-001 Blue policy verification (the cryptographic membrane)
mod capabilities;
mod capture;
mod clock_discipline; // #183 — GPS clock discipline (no-RTC time correctness)
mod forensic;
mod dedup;
mod heartbeat;
mod release_currency;
mod identity;
mod silver;
mod hunter;
mod nats_publisher; // #178 — single multiplexed, supervised NATS connection
mod nl80211;
mod node_config;
mod rid_ble;
mod rfkill;
mod sensor;       // Wave 7.2 — Sensor trait (Anastomotic Reticulum scaffold)
mod sensor_gps;   // Wave 7.2 — NmeaGps impl (first non-Wi-Fi producer)
mod status;
mod tick_publisher; // Wave 6.5 — Green Protocol 1 Hz AirspaceState publisher
mod watchdog;
mod upward; // D40 — durable operational reports and independent algedonic Red

use std::sync::Arc;

use arc_swap::ArcSwap;
use audit::DropCounters;
use cybrrd_rid_protocol::models::NormalizedTelemetry;
use heartbeat::RadioState;
use node_config::EngineConfig;
use sensor::{Sensor, SensorContext, SensorState};
use sensor_gps::{GpsFix, NmeaGps};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    // #185 Drop 2 — the host updater's cryptographic pre-flight. Run as a
    // throwaway `verify-blue <file>` invocation (no capabilities, no NATS):
    // parse + Ed25519-verify the draft against the compile-pinned key, exit 0
    // if valid. The host updater refuses to pull/restart on a non-zero exit.
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("verify-blue") {
        std::process::exit(blue_policy::verify_blue_file(argv.get(2).map(String::as_str)));
    }
    if argv.get(1).map(String::as_str) == Some("update-config") {
        let path = argv.get(2).ok_or("update-config requires a config file")?;
        let cfg = EngineConfig::load_with_fallback(path, path)?;
        println!("{}", serde_json::json!({"node_id": cfg.node.id,
            "gps_required": cfg.sensors.gps.required, "status_file": cfg.node.status_file,
            "upward_enabled": cfg.upward.is_some(),
            "ring": cfg.node.channel.as_deref().unwrap_or("general")}));
        return Ok(());
    }
    println!("BRRDfeeder Engine container initializing…");

    // Wave 7.1 substrate-truth gate: refuse to start if the privileged
    // operations we're about to perform (nl80211 channel control, raw
    // monitor capture) would silently fail for lack of the required
    // Linux capabilities. Substrate-honest: better to fail loud at
    // startup than to run dark and miss drone overflights.
    if let Err(failure) = capabilities::check_required() {
        eprintln!("[-] Capability check failed:\n");
        eprintln!("{}", failure);
        std::process::exit(1);
    }
    println!("[+] Linux capabilities verified: cap_net_admin + cap_net_raw");

    let cfg = EngineConfig::load_with_fallback("config.yaml", "../config.yaml")?;
    let node = cfg.to_node();

    let running_identity = identity::RunningIdentity::at_start().await;
    running_identity.log_startup(std::io::stdout(), std::io::stderr())?;
    let running_build_seq = running_identity.verified_policy_floor();
    let policy_state_path = std::path::PathBuf::from("/var/lib/brrdfeeder/policy_state.json");
    // The host package health gate alone commits release watermarks.
    println!(
        "[+] Node identity locked: id={} version={} loc=({:.6},{:.6})",
        node.id, node.version, node.location.lat, node.location.lon
    );
    let interface = cfg.capture.interface.clone();
    let dedup_window_ms = cfg.tuning.edge_processing.deduplication_window_ms;
    let heartbeat_interval = cfg.tuning.heartbeat.interval_secs;
    let broker_urls = cfg.backhaul.broker_urls.clone();
    let credentials_path = cfg.backhaul.credentials_path.clone();

    let counters = DropCounters::new();
    let radio = RadioState::new();
    let (tx, rx) = mpsc::channel::<NormalizedTelemetry>(1024);

    // #178 (2026-06-17, Cy + Gemini + Synth) — the SINGLE multiplexed,
    // supervised NATS connection. Previously the engine opened five independent
    // clients (telemetry / heartbeat / audit / substrate-audit / green-tick) and
    // only telemetry carried the warm-capture armor; the heartbeat liveness nerve
    // rode a bare client that could silently wedge while telemetry pumped fine.
    // Now every publisher routes through this one supervised connection and
    // inherits flush-confirm delivery + wedge-detect + in-place reconnect +
    // edge-buffer. Each task holds a cheap clone of the handle; publish() is a
    // non-blocking enqueue so producers never block on the network.
    // #185 Phase 3 Drop 1c — the E-CNP-001 verification membrane rides the same
    // supervised connection: a durable JetStream consumer on this node's control
    // subject that VERIFIES (does not yet execute) any signed Blue policy.
    let control = nats_publisher::ControlConfig {
        node_id: node.id.clone(),
        stream: "CYBRRD_CONTROL".to_string(),
        subject: format!("cybrrd.control.blue.policy.{}", node.id),
        state_path: policy_state_path.clone(),
        running_build_seq,
    };
    let nats = nats_publisher::start(
        broker_urls.clone(),
        credentials_path.clone(),
        counters.clone(),
        Some(control),
    );

    // Wave 7.1 Inc 8 — shared capture-liveness state. main.rs resolves
    // the initial ifindex once; from here on the capture loop and the
    // watchdog re-resolve it (the 2026-05-13 lesson: a cached ifindex
    // is a substrate lie waiting to happen). A failed initial resolve
    // is non-fatal — seed 0 and let the capture loop's outer iteration
    // re-resolve once the interface appears.
    let initial_ifindex = match watchdog::iface_to_ifindex(&interface) {
        Ok(idx) => {
            println!(
                "[+] Capture interface {} resolved to ifindex {} (initial)",
                interface, idx
            );
            idx
        }
        Err(e) => {
            eprintln!(
                "[!] Could not resolve initial ifindex for {}: {} — \
                 seeding 0; capture loop will re-resolve",
                interface, e
            );
            0
        }
    };
    let liveness = watchdog::CaptureLiveness::new(initial_ifindex);

    // Audit emitter: 1 Hz drain of drop counters → audit subject. Publishes via
    // the shared multiplexed supervisor (#178); publish() is a non-blocking
    // enqueue, so the closure hands off and returns an immediately-ready future.
    let audit_counters = counters.clone();
    let audit_node_id = node.id.clone();
    let audit_nats = nats.clone();
    tokio::spawn(async move {
        let publisher = move |subject: String, bytes: Vec<u8>| {
            audit_nats.publish(subject, bytes);
            async {}
        };
        audit::run_audit_emitter(audit_counters, audit_node_id, publisher).await;
    });

    // Wave 7.1 Hunter state — created here so heartbeat emitter and
    // Hunter task share the same Arc<HunterState> from the start.
    // When Hunter is disabled, the heartbeat emitter receives None
    // and omits the hunter block entirely (pre-Wave-7 compat).
    let hunter_state = hunter::HunterState::new();
    let (hb_hunter_state, hb_wifi_iface): (
        Option<std::sync::Arc<hunter::HunterState>>,
        Option<String>,
    ) = if cfg.capture.hunter.enabled {
        (Some(hunter_state.clone()), Some(interface.clone()))
    } else {
        (None, None)
    };

    // Heartbeat emitter spawn is intentionally delayed (Wave 7.3b
    // 2026-05-26): it needs the GPS sensor's health + latest-fix
    // handles to populate the Self-Diagnostic block, and those are
    // constructed by NmeaGps::start() further down. The spawn order
    // doesn't change runtime behavior (all tokio::spawned tasks run
    // concurrently); only the textual order shifts.

    // Per-frame telemetry pump: drains captured frames, serializes, and
    // publishes via the shared multiplexed supervisor (#178). The warm-capture
    // armor now lives in the supervisor, so this is a thin drain-and-forward.
    let bk_counters = counters.clone();
    let bk_nats = nats.clone();
    tokio::spawn(async move {
        backhaul::run_telemetry_pump(rx, bk_nats, bk_counters).await;
    });

    // Wave 7.1 Inc 7 + Inc 8 — substrate-audit channel. Now created
    // UNCONDITIONALLY: the Capture Liveness Watchdog (Inc 8) emits
    // capture_stall / capture_recovered / capture_recovery_failed
    // events whether or not the Hunter is doing channel rotation, and
    // the Hunter (Inc 7) emits tier_a_recovery / survey_failure when
    // it is enabled. Both ride this one channel through the substrate-
    // audit emitter to NATS audit subjects with full Kittler-lineage
    // provenance.
    let (substrate_audit_tx, substrate_audit_rx) =
        tokio::sync::mpsc::channel::<audit::SubstrateAuditEvent>(256);
    let sa_nats = nats.clone();
    tokio::spawn(async move {
        let publisher = move |subject: String, bytes: Vec<u8>| {
            sa_nats.publish(subject, bytes);
            async {}
        };
        audit::run_substrate_audit_emitter(substrate_audit_rx, publisher).await;
    });

    // Wave 7.1 Inc 8 — Capture Liveness Watchdog. Spawned UNCONDITIONALLY:
    // capture can fall out of monitor mode (USB re-enumeration, driver
    // reset) regardless of whether the Hunter is rotating channels, so
    // the watchdog must witness liveness in every deployment. It owns
    // the Recovering→Up / Recovering→Error radio_status transitions.
    {
        let wd_iface = interface.clone();
        let wd_liveness = Arc::clone(&liveness);
        let wd_radio = radio.clone();
        let wd_node_id = node.id.clone();
        let wd_audit_tx = substrate_audit_tx.clone();
        tokio::spawn(async move {
            watchdog::run_watchdog(
                wd_iface,
                wd_liveness,
                wd_radio,
                wd_node_id,
                Some(wd_audit_tx),
            )
            .await;
        });
    }

    // Wave 7.1 Hunter (Kittler Substrate Defense): channel rotation
    // across the configured channel_set. Spawned only when explicitly
    // enabled in config.yaml — backward-compatible with pre-Wave-7
    // deployments that pre-lock the interface manually. Shares the
    // same Arc<HunterState> with the heartbeat emitter declared above,
    // and (Inc 8) the same Arc<CaptureLiveness> so it always targets
    // the live ifindex.
    let lock_on_duration_ms = cfg.capture.hunter.lock_on_duration_ms;
    let lock_on_min_ms = cfg.capture.hunter.lock_on_min_ms;
    if cfg.capture.hunter.enabled {
        let hunter_cfg = hunter::HunterConfig {
            channel_set: cfg.capture.hunter.channel_set.clone(),
            dwell_default: std::time::Duration::from_millis(cfg.capture.hunter.dwell_default_ms),
            dwell_priority: std::time::Duration::from_millis(cfg.capture.hunter.dwell_priority_ms),
            priority_channels: cfg.capture.hunter.priority_channels.clone(),
            lock_on_duration: std::time::Duration::from_millis(lock_on_duration_ms),
            lock_on_min: std::time::Duration::from_millis(lock_on_min_ms),
        };
        let hunter_iface = interface.clone();
        let hunter_state_for_task = hunter_state.clone();
        let hunter_node_id = node.id.clone();
        let hunter_audit_tx = substrate_audit_tx.clone();
        let hunter_liveness = Arc::clone(&liveness);
        tokio::spawn(async move {
            hunter::run_hunter(
                hunter_iface,
                hunter_cfg,
                hunter_state_for_task,
                hunter_node_id,
                Some(hunter_audit_tx),
                hunter_liveness,
            )
            .await;
        });
    } else {
        println!(
            "[hunter] disabled (capture.hunter.enabled=false); \
             interface remains on its pre-locked channel"
        );
    }

    // Wave 7.2 (2026-05-26) — NmeaGps spawn.
    //
    // First non-Wi-Fi producer in the Anastomotic Reticulum. This cut
    // populates a shared `Arc<ArcSwap<Option<GpsFix>>>` ("latest fix")
    // and logs each fix at INFO; it does NOT yet flow into the audit
    // envelope's NodeLocation stamper (that's the next cut, gated on
    // observing the sensor go Healthy on cardinal).
    //
    // Device path is hardcoded to /dev/ttyACM1 for the MVP — matches
    // the u-blox 7 enumeration on cardinal's Anker hub Port 1. Config-
    // driven selection (sensors.gps.device in config.yaml) is a small
    // follow-on cut once the integration shape stabilizes.
    let gps_cancel = CancellationToken::new();
    let gps_ctx = SensorContext {
        node_id: Arc::from(node.id.as_str()),
        cancel: gps_cancel.clone(),
        substrate_audit: substrate_audit_tx.clone(),
    };
    // Wave 7.4 — config-driven NmeaGps from cfg.sensors.gps.
    let mut gps = NmeaGps::new(&cfg.sensors.gps.device);
    gps.baud = cfg.sensors.gps.baud;
    gps.stale_after = std::time::Duration::from_secs(cfg.sensors.gps.stale_after_secs);
    let mut gps_handle = gps.start(gps_ctx.clone());
    // Clone the health ArcSwap up-front so the heartbeat emitter
    // (declared further down) can capture it without re-borrowing
    // gps_handle (whose readings field gets moved into the
    // fix-keeper task below).
    let gps_handle_health = Arc::clone(&gps_handle.health);

    // #183 — GPS clock-discipline trust flag. The keeper (below) sets it once
    // the system clock is GPS-disciplined (stepped to GPS UTC, or confirmed
    // already NTP-correct). The startup gate blocks operational publishing until time
    // is trustworthy — closing the no-RTC silent-rejection seam where a
    // cold-booted feeder stamps frames with a stale clock that the Bohr Spool
    // drops as a causality violation.
    let time_trust = Arc::new(clock_discipline::TimeTrust::new());

    // Latest-fix shared state + GPS keeper. Spawned BEFORE the startup gate so
    // fixes — and the clock discipline they carry — flow during the grace
    // window (the gate waits on what this task produces). Wave 7.3a consumer is
    // the capture loop's audit-envelope stamper; Wave 7.3b the heartbeat GPS
    // surface.
    let gps_latest: Arc<ArcSwap<Option<GpsFix>>> =
        Arc::new(ArcSwap::from_pointee(None));
    let gps_latest_for_task = Arc::clone(&gps_latest);
    let keeper_time_trust = Arc::clone(&time_trust);
    let clock_policy = cfg.sensors.gps.clock.clone();
    tokio::spawn(async move {
        while let Some(fix) = gps_handle.readings.recv().await {
            gps_latest_for_task.store(Arc::new(Some(fix.clone())));
            // #183 — discipline the system clock from the receiver's UTC
            // (idempotent; no-op once trusted). Releases the publish gate.
            if let Some(gps_utc_ms) = fix.gps_utc_ms {
                clock_discipline::discipline_from_gps(gps_utc_ms, &keeper_time_trust, &clock_policy);
            }
            println!(
                "[gps] fix lat={:.6} lon={:.6} alt_m={:.1} sats={} hdop={:.2} q={}",
                fix.lat, fix.lon, fix.alt_m, fix.sat_count, fix.hdop, fix.fix_quality
            );
        }
        println!("[gps] readings channel closed; keeper task exiting");
    });

    // D32: health reporting starts after GPS handles exist but BEFORE the
    // operational startup gate. Pre-fix nodes report no current position and
    // retain the named D26 update refusal during the gate's grace period.
    let hb_node_id = node.id.clone();
    let hb_radio = radio.clone();
    let hb_nats = nats.clone();
    let hb_gps_health = Arc::clone(&gps_handle_health);
    let hb_gps_latest = Arc::clone(&gps_latest);
    // Starts before the required-GPS gate: distress must escape startup failure.
    // Separate connections/queues deliberately isolate these two upward lanes
    // from each other and from the multiplexed best-effort telemetry channel.
    let upward_cancel = CancellationToken::new();
    let upward_task = if let Some(config) = &cfg.upward {
        match upward::start(config, &node.id, broker_urls.clone(), credentials_path.clone()) {
            Ok(handle) => Some(tokio::spawn(upward::monitor(handle, running_identity.clone(),
                radio.clone(), Arc::clone(&gps_handle_health), Arc::clone(&time_trust),
                cfg.sensors.gps.required, upward_cancel.clone()))),
            Err(reason) => { eprintln!("[upward] startup refused: {reason}; engine continues without upward delivery"); None }
        }
    } else { None };
    // D26: no stale environment fallback; Silver names the refusal each heartbeat.
    let hb_fleet = heartbeat::FleetProprioception {
        identity: running_identity,
        silver: Some(cfg.silver_context()),
        channel: cfg
            .node
            .channel
            .clone()
            .unwrap_or_else(|| "general".to_string()),
        time_trust: Arc::clone(&time_trust),
    };
    let ble_inventory = Arc::new(arc_swap::ArcSwapOption::empty());
    let status_source = cfg
        .node
        .status_file
        .as_ref()
        .map(|path| {
            status::StatusSource::new(
                path.clone(),
                interface.clone(),
                cfg.sensors.gps.device.clone(),
                Arc::clone(&ble_inventory),
                Arc::clone(&nats.status),
                Arc::clone(&liveness.frames),
                cfg.node.effective_status_interval_secs(heartbeat_interval),
            )
        });

    let status_cleanup = status_source.clone();
    let heartbeat_task = tokio::spawn(async move {
        // #178: the heartbeat liveness nerve now rides the SHARED supervised
        // connection — the whole point of the multiplex. No bare client that can
        // silently wedge while telemetry pumps fine.
        let publisher = move |subject: String, bytes: Vec<u8>| {
            hb_nats.publish(subject, bytes);
            async {}
        };
        heartbeat::run_heartbeat_emitter(
            hb_node_id,
            hb_radio,
            hb_hunter_state,
            hb_wifi_iface,
            // Wave 7.3b: GPS health + latest-fix surfaces — Self-
            // Diagnostic block in the heartbeat payload.
            Some(hb_gps_health),
            Some(hb_gps_latest),
            hb_fleet,
            heartbeat_interval,
            status_source,
            publisher,
        )
        .await;
    });

    // Wave 7.4 + #183 — required-GPS + trusted-time startup gate. When
    // sensors.gps.required: true (default), the engine refuses to proceed past
    // this point until BOTH:
    //   (1) the GPS sensor reaches Healthy (first 3D fix) — position trust, and
    //   (2) the system clock is GPS-disciplined (#183) — TIME trust.
    // Position and time are separate trust dimensions; a node that knows WHERE
    // it is but not WHEN can still poison the lineage (every frame it ships in
    // the wrong-clock window is silently rejected by the Bohr Spool).
    //
    // Operators in dev/test environments without GPS hardware must explicitly
    // set `sensors.gps.required: false` in config.yaml, which bypasses this gate
    // AND stamps every emitted frame with `node_position_source: config_static`
    // (self-disclosing untrusted mode; downstream Reputation Gravity refuses
    // trust accumulation against such frames).
    if cfg.sensors.gps.required {
        let grace_secs = cfg.sensors.gps.startup_grace_secs;
        let grace = std::time::Duration::from_secs(grace_secs);
        let deadline = std::time::Instant::now() + grace;
        println!(
            "[gps] sensors.gps.required=true — waiting up to {}s for first 3D fix \
             AND GPS-disciplined system time (#183) before operational publishing",
            grace_secs
        );
        loop {
            let healthy = gps_handle_health.load().state == SensorState::Healthy;
            let timed = time_trust.is_trusted();
            if healthy && timed {
                println!(
                    "[+] required GPS Healthy + system time GPS-disciplined; engine proceeding"
                );
                break;
            }
            if std::time::Instant::now() > deadline {
                eprintln!(
                    "[-] FATAL: sensors.gps.required=true but did not reach \
                     GPS-Healthy + trusted-time within {}s (gps_healthy={} time_trusted={}).\n\
                     [-] Engine refuses to start — operator must either:\n\
                     [-]   (a) verify the GPS receiver at {} is connected with sky view, or\n\
                     [-]   (b) ensure the container has CAP_SYS_TIME so the clock can be \
                     stepped from GPS (or restore NTP), or\n\
                     [-]   (c) set `sensors.gps.required: false` in config.yaml \
                     (DEV MODE — frames tagged node_position_source: config_static and \
                     rejected by downstream Reputation Gravity)",
                    grace_secs, healthy, timed, cfg.sensors.gps.device
                );
                std::process::exit(1);
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    } else {
        println!(
            "[gps] sensors.gps.required=false (DEV MODE) — engine will start without GPS lock \
             or time discipline; ALL emitted frames will carry node_position_source: \
             config_static and are reputation-INELIGIBLE downstream"
        );
    }

    // The original substrate_audit_tx is no longer needed in main —
    // the watchdog, Hunter (optional), and NmeaGps (Wave 7.2) hold
    // their own clones. Drop it so the substrate-audit emitter's
    // channel closes cleanly if every producing task ever exits.
    // Capture now owns the final sender for forensic write-health events.

    // Capture loop (last; runs the radio thread inline). Wave 7.1
    // Inc 6 — passes the HunterState so per-frame protocol-neutral
    // drone-class detections can fire the lock-on signal. Inc 8 —
    // passes the shared CaptureLiveness so the loop can re-establish
    // monitor mode + re-open libpcap under watchdog direction.
    let capture_hunter_state = if cfg.capture.hunter.enabled {
        Some(hunter_state.clone())
    } else {
        None
    };

    // Wave 6.5 Green Protocol — 1 Hz AirspaceState tick publisher.
    // Pack-consensus 2026-06-04 (Cy + Gemini + Synth). Gated on env
    // var BRRDFEEDER_ENABLE_AIRSPACE_PUBLISHER=true. Default-off so
    // this lands safely alongside the existing per-frame backhaul
    // path; flip the env var to cut over once the cardinal-side
    // emission is validated against the Phase 2 globe-web consumer
    // (also default-off behind --enable-airspace-consumer).
    let airspace_publisher_enabled = std::env::var("BRRDFEEDER_ENABLE_AIRSPACE_PUBLISHER")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);
    let observation_store = if airspace_publisher_enabled {
        println!(
            "[green-tick] airspace publisher ENABLED (env BRRDFEEDER_ENABLE_AIRSPACE_PUBLISHER=true)"
        );
        let store = tick_publisher::ObservationStore::new();
        // #178: the tick publisher now rides the SHARED supervised connection
        // rather than a dedicated client. The Green Protocol beacon path inherits
        // wedge-detect + reconnect for free; tick best-effort semantics are
        // preserved (handle.publish is a non-blocking enqueue, a momentarily-full
        // handoff drops to the next tick's fresh state).
        let tp_node_id: String = node.id.clone();
        let tp_static_loc = node.location.clone();
        let tp_store = Arc::clone(&store);
        let tp_gps = Arc::clone(&gps_latest);
        let tp_nats = nats.clone();
        tokio::spawn(async move {
            tick_publisher::run_tick_publisher(
                tp_node_id,
                tp_static_loc,
                tp_store,
                tp_gps,
                tp_nats,
                tokio_util::sync::CancellationToken::new(),
            )
            .await;
        });
        Some(store)
    } else {
        println!(
            "[green-tick] airspace publisher disabled (set BRRDFEEDER_ENABLE_AIRSPACE_PUBLISHER=true to enable)"
        );
        None
    };

    // REQ-BRRD-006: source emits raw AD; the shared host decoder owns ODID.
    let ble_keeper = if cfg.sensors.rid_ble.enabled {
        let mut handle = rid_ble::RidBle(
            cfg.sensors.rid_ble.clone(),
            ble_inventory,
            Arc::clone(&liveness.frames),
        )
        .start(gps_ctx);
        let mut pipeline = capture::PostParse::new(capture::ForwardContext {
            tx: tx.clone(), node: node.clone(), counters: counters.clone(),
            hunter: capture_hunter_state.clone(), lock_on_duration_ms,
            gps: Arc::clone(&gps_latest), observation_store: observation_store.clone(),
        }, dedup_window_ms);
        let cancel = gps_cancel.clone();
        let ble_dropped = Arc::clone(&counters.rid_ble_unassociated_dropped);
        Some(tokio::spawn(async move {
            let mut decoder = cybrrd_rid_protocol::ble::BleDecoder::default();
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        // Wait for the sensor's bounded scan-disable and socket close.
                        while handle.readings.recv().await.is_some() {}
                        break;
                    },
                    reading = handle.readings.recv() => {
                        let Some(ad) = reading else { break; };
                        let keep_running = decoder.ingest(&ad.ad, ad.addr, ad.rssi, ad.meta)
                            .is_none_or(|data| pipeline.forward(data));
                        ble_dropped.fetch_add(decoder.take_unassociated_dropped(), std::sync::atomic::Ordering::Relaxed);
                        if !keep_running { break; }
                    }
                }
            }
            // Dropping the receiver also terminates a failed/closed sensor path.
        }))
    } else {
        println!("[rid_ble] disabled");
        None
    };

    // Race the capture loop against SIGTERM/SIGINT. Per ADR 0010 (Edge
    // Node Service Management), the engine MUST handle SIGTERM via a
    // bounded shutdown so systemd's Restart=always can cleanly recycle
    // us when the cleanup path would otherwise hang on a Drop-blocked
    // resource (the 2026-06-04 SIGTERM-survivor incident: a serial-port
    // task held /dev/ttyACM1 after ps -p showed the process gone).
    let capture_future = capture::start_capture_loop(
        tx,
        node,
        interface,
        dedup_window_ms,
        counters,
        radio,
        capture_hunter_state,
        lock_on_duration_ms,
        liveness,
        // Wave 7.3a: capture loop's audit-envelope stamper consumes the
        // GPS fix swap; when present + fresh, frame.node.location goes
        // dynamic. Falls back to config-static when GPS is cold/stale.
        Arc::clone(&gps_latest),
        // Wave 6.5 Green Protocol: per-drone observation cache for the
        // tick_publisher. None = legacy single-path mode.
        observation_store,
        cfg.capture.savefile.clone(),
        substrate_audit_tx,
    );
    tokio::pin!(capture_future);

    let mut sigterm = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::terminate(),
    )?;
    let mut sigint = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::interrupt(),
    )?;

    tokio::select! {
        _ = &mut capture_future => {
            eprintln!("[shutdown] capture loop returned (unexpected); entering bounded cleanup");
        }
        _ = sigterm.recv() => {
            eprintln!("[shutdown] SIGTERM received; entering bounded cleanup");
        }
        _ = sigint.recv() => {
            eprintln!("[shutdown] SIGINT received; entering bounded cleanup");
        }
    }

    gps_cancel.cancel(); // GPS and BLE share the engine shutdown token.
    upward_cancel.cancel();
    heartbeat_task.abort(); // stop scheduling; an in-flight blocking write may finish

    // Bounded cleanup: 5 second hard ceiling per ADR 0010. Tokio
    // software defense; systemd TimeoutStopSec=10 is the OS-level
    // failsafe. If cleanup hangs (Drop blocked on a serial-port
    // syscall, NATS flush stalled, etc.), std::process::exit(1) fires
    // and systemd's Restart=always brings us back cleanly.
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        async {
            if let Some(task) = upward_task { let _ = task.await; }
            if let Some(source) = status_cleanup {
                // Shares the writer lock: unlink cannot race a late rename.
                if let Err(e) = source.shutdown().await {
                    eprintln!("[status] best-effort shutdown unlink failed: {e}");
                }
            }
            if let Some(keeper) = ble_keeper { let _ = keeper.await; }
            // Phase 1 (current): rely on Drop semantics. The async-nats
            // client, tokio_serial GPS handle, libpcap capture, and the
            // various spawned tasks all release resources in their Drop
            // impls. The 5s timeout is defense-in-depth against any one
            // of those Drops blocking on a syscall.
            //
            // Phase 2 (future): wire explicit drain calls here for
            // NATS publish flush + audit-publisher drain + lake-writer
            // drain. For now, a small yield lets Tokio's runtime
            // schedule the Drop tasks before we proceed.
            eprintln!("[shutdown] running Drop-based resource release");
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
    .await
    {
        Ok(_) => {
            eprintln!("[shutdown] cleanup complete; exit(0)");
            std::process::exit(0);
        }
        Err(_) => {
            eprintln!(
                "[shutdown] cleanup TIMEOUT at 5s; hard-exiting so systemd can restart cleanly"
            );
            std::process::exit(1);
        }
    }
}
