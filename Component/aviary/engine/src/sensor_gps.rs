// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! `NmeaGps` — Sensor impl reading NMEA 0183 from a u-blox 7 GPS
//! over CDC-ACM. First impl of the `Sensor` trait.
//!
//! Hardware verified 2026-05-26 on cardinal (CM4):
//!   - u-blox 7 GPS/GNSS Receiver enumerated as Bus 001 Device 009
//!   - Driver: `cdc_acm` @ `/dev/ttyACM1`
//!   - Stable symlink:
//!     `/dev/serial/by-id/usb-u-blox_AG_-_www.u-blox.com_u-blox_7_-_GPS_GNSS_Receiver-if00`
//!
//! Protocol choice (pack-ratified 2026-05-26): **NMEA 0183**.
//! UBX is a future Phase-B optimization; NMEA keeps the
//! future-receiver-swap option open. The `nmea` crate accumulates
//! state across GGA/RMC/GSA sentences; we emit a `GpsFix` when GGA
//! reports a non-zero fix quality with valid lat/lon.
//!
//! State machine:
//!   Initializing ─first 3D fix──▶ Healthy
//!   Healthy      ─stale > N s──▶ Degraded   (fires sensor_gps_lock_lost)
//!   Degraded     ─fresh fix───▶ Healthy     (fires sensor_gps_lock_acquired)
//!   any          ─serial died─▶ retry-loop  (does not exit task; logs
//!                                            errors via bump_error)

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use nmea::Nmea;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_serial::SerialPortBuilderExt;

use crate::audit::{SubstrateAuditEvent, KITTLER_LINEAGE};
use crate::sensor::{
    now_unix_ms, Sensor, SensorContext, SensorHandle, SensorHealth, SensorState,
};

/// One position fix. Lake-writer computes H3 res7/res8 from lat/lon at
/// ingest (Wave 6.3 lock). Wave 7.2 keeps GpsFix engine-internal —
/// promote to `cybrrd-rid-protocol` only when it enters the wire
/// AuditEnvelope (separate cut).
#[derive(Clone, Debug, Serialize)]
pub struct GpsFix {
    pub lat: f64,
    pub lon: f64,
    pub alt_m: f32,
    /// NMEA fix quality. 0=invalid, 1=GPS, 2=DGPS, 3=PPS,
    /// 4=RTK-fixed, 5=RTK-float, 6=estimated, 7=manual, 8=simulation.
    pub fix_quality: u8,
    pub sat_count: u8,
    /// Horizontal Dilution of Precision. Lower = more accurate.
    pub hdop: f32,
    /// Unix-ms when the engine emitted the fix, from the system wall
    /// clock. On a no-RTC edge node this is UNTRUSTWORTHY until the clock
    /// is disciplined (see `gps_utc_ms` + `clock_discipline`, #183).
    pub fix_at_ms: i64,
    /// #183 — the GPS receiver's own UTC, assembled from the NMEA RMC date
    /// + time-of-day, as Unix-ms. `None` until the parser has seen an RMC
    /// (GGA alone carries time-of-day but no date). This is the authoritative
    /// time anchor used to discipline the system clock — atomic-clock truth,
    /// independent of the node's (possibly stale, no-RTC) wall clock.
    pub gps_utc_ms: Option<i64>,
}

/// The sensor. Hold + spawn via `Sensor::start`.
pub struct NmeaGps {
    pub device: String,
    pub baud: u32,
    /// If no fresh fix within this, transition Healthy → Degraded.
    /// Default 30s (u-blox 7 cold-start outdoors ~30s; warm-start <5s).
    pub stale_after: Duration,
}

impl NmeaGps {
    pub fn new(device: impl Into<String>) -> Self {
        NmeaGps {
            device: device.into(),
            baud: 9600,
            stale_after: Duration::from_secs(30),
        }
    }
}

impl Sensor for NmeaGps {
    type Reading = GpsFix;

    fn name(&self) -> &'static str {
        "gps:nmea-0183"
    }

    fn start(self, ctx: SensorContext) -> SensorHandle<GpsFix> {
        // 16-slot ring ≈ 16s buffer at 1 Hz fix rate. If the consumer
        // stalls, `try_send` returns Full — we drop the new fix and
        // bump error_count. Substrate-truth: a stale consumer is its
        // own problem; we don't block the GPS thread on its laziness.
        let (tx, rx) = mpsc::channel::<GpsFix>(16);
        let health = Arc::new(ArcSwap::from_pointee(SensorHealth::initializing(
            self.name(),
        )));
        let health_for_task = Arc::clone(&health);

        tokio::spawn(run_ublox_gps(self, ctx, tx, health_for_task));

        SensorHandle { readings: rx, health }
    }
}

/// The sensor task. Loops until `ctx.cancel` fires.
///
/// On serial errors, the task does NOT exit — it retries opening the
/// device every 2s. Substrate-truth: USB devices can disappear and
/// re-enumerate (we just saw this happen on cardinal boot at dmesg
/// t=85s); the engine must witness that without giving up.
async fn run_ublox_gps(
    cfg: NmeaGps,
    ctx: SensorContext,
    tx: mpsc::Sender<GpsFix>,
    health: Arc<ArcSwap<SensorHealth>>,
) {
    println!(
        "[gps] starting NMEA-0183 GPS on {} @ {} baud (stale_after={}s)",
        cfg.device,
        cfg.baud,
        cfg.stale_after.as_secs()
    );

    'outer: loop {
        // Substrate-honest serial-open retry. udev may not have created
        // the device node yet at engine startup; wait it out.
        let mut port = loop {
            if ctx.cancel.is_cancelled() {
                println!("[gps] canceled before serial open");
                set_state(&health, SensorState::Failed, Some("canceled".into()));
                return;
            }
            match tokio_serial::new(&cfg.device, cfg.baud).open_native_async() {
                Ok(p) => {
                    println!("[gps] serial port opened: {}", cfg.device);
                    break p;
                }
                Err(e) => {
                    bump_error(&health, format!("serial open: {}", e));
                    eprintln!(
                        "[gps] serial open failed ({}); retrying in 2s",
                        e
                    );
                    tokio::select! {
                        _ = ctx.cancel.cancelled() => {
                            set_state(&health, SensorState::Failed, Some("canceled".into()));
                            return;
                        }
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    }
                }
            }
        };

        let mut parser = Nmea::default();
        let mut last_fix_ms: Option<i64> = None;
        let reader = BufReader::new(&mut port);
        let mut lines = reader.lines();
        let mut stale_ticker = tokio::time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                _ = ctx.cancel.cancelled() => {
                    println!("[gps] cancellation received; shutting down");
                    set_state(&health, SensorState::Failed, Some("canceled".into()));
                    return;
                }

                line_res = lines.next_line() => {
                    match line_res {
                        Ok(Some(line)) => {
                            if let Some(fix) = parse_nmea_line(&mut parser, &line) {
                                last_fix_ms = Some(fix.fix_at_ms);
                                handle_fresh_fix(&fix, &ctx, &health, &tx).await;
                            } else if matches!(nmea::parse_str(line.trim()), Ok(nmea::ParseResult::GGA(_))) {
                                // An explicit no-fix/incomplete GGA is not fresh position.
                                // Silver checks health as well as age, so cached coordinates
                                // cannot remain current until the stale timer expires.
                                last_fix_ms = None;
                                set_state(&health, SensorState::Degraded, Some("GGA position unavailable".into()));
                            }
                        }
                        Ok(None) => {
                            // EOF on the serial stream — the kernel
                            // dropped the cdc_acm endpoint. Re-open.
                            bump_error(&health, "serial EOF; will reopen".into());
                            eprintln!("[gps] serial EOF; reopening in 2s");
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            continue 'outer;
                        }
                        Err(e) => {
                            bump_error(&health, format!("serial read: {}", e));
                            eprintln!("[gps] serial read error: {}; reopening in 2s", e);
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            continue 'outer;
                        }
                    }
                }

                _ = stale_ticker.tick() => {
                    check_staleness(&cfg, last_fix_ms, &ctx, &health).await;
                }
            }
        }
    }
}

/// Apply a fresh GpsFix to health + forward to the readings channel.
async fn handle_fresh_fix(
    fix: &GpsFix,
    ctx: &SensorContext,
    health: &Arc<ArcSwap<SensorHealth>>,
    tx: &mpsc::Sender<GpsFix>,
) {
    let prev = health.load();
    let was_healthy = prev.state == SensorState::Healthy;

    // Transition into Healthy on first fix or on recovery from Degraded.
    if !was_healthy {
        println!(
            "[gps] fix acquired: lat={:.6} lon={:.6} alt_m={:.1} sats={} hdop={:.2}",
            fix.lat, fix.lon, fix.alt_m, fix.sat_count, fix.hdop
        );
        set_state(health, SensorState::Healthy, None);
        fire_state_change(ctx, "sensor_gps_lock_acquired", true).await;
    } else {
        // Just refresh last_reading_ms; no state-change noise.
        let mut h = (**prev).clone();
        h.last_reading_ms = Some(fix.fix_at_ms);
        h.detail = None;
        health.store(Arc::new(h));
    }

    if let Err(e) = tx.try_send(fix.clone()) {
        bump_error(health, format!("readings buffer: {}", e));
    }
}

/// Per-second staleness check. If we've been Healthy but no fix
/// arrived in `cfg.stale_after`, drop to Degraded + fire audit event.
async fn check_staleness(
    cfg: &NmeaGps,
    last_fix_ms: Option<i64>,
    ctx: &SensorContext,
    health: &Arc<ArcSwap<SensorHealth>>,
) {
    let Some(last) = last_fix_ms else { return };
    let now = now_unix_ms();
    let stale_threshold = cfg.stale_after.as_millis() as i64;
    if now - last <= stale_threshold {
        return;
    }
    let prev = health.load();
    if prev.state != SensorState::Healthy {
        return;
    }
    println!(
        "[gps] fix stale ({} ms > {} ms threshold); marking Degraded",
        now - last,
        stale_threshold
    );
    set_state(
        health,
        SensorState::Degraded,
        Some(format!("stale {}ms", now - last)),
    );
    fire_state_change(ctx, "sensor_gps_lock_lost", false).await;
}

/// Parse one NMEA sentence; returns Some iff it was a GGA reporting a
/// non-zero fix quality with valid lat/lon. Substrate-honest: we never
/// emit a GpsFix when fix_quality == Invalid (cold-start receivers
/// would otherwise publish a "false-positive zero-island" position).
fn parse_nmea_line(parser: &mut Nmea, line: &str) -> Option<GpsFix> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let parsed = nmea::parse_str(line).ok()?;
    if parser.parse(line).is_err() {
        // Malformed mid-stream sentence — common during serial bring-up.
        // Skip silently; next sentence will catch us up.
        return None;
    }
    // D32-F1: accumulated Nmea state is not a new observation. RMC still
    // updates the parser's date, but only the current GGA supplies a new fix.
    let nmea::ParseResult::GGA(gga) = parsed else { return None };
    gga.fix_time?;
    let lat = gga.latitude?;
    let lon = gga.longitude?;
    if !crate::silver::valid_coordinates(lat, lon) { return None; }
    let fix_type = gga.fix_type?;
    let fix_quality = fix_quality_from(fix_type)?;
    let alt_m = gga.altitude.unwrap_or(0.0);
    let sat_count = gga.fix_satellites.unwrap_or(0) as u8;
    let hdop = gga.hdop.unwrap_or(0.0);
    Some(GpsFix {
        lat,
        lon,
        alt_m,
        fix_quality,
        sat_count,
        hdop,
        fix_at_ms: now_unix_ms(),
        gps_utc_ms: gps_utc_ms_from(parser),
    })
}

/// #183 — assemble the receiver's UTC (epoch ms) from the parser's
/// accumulated RMC date + time-of-day. Returns `None` until both are
/// present (the u-blox sentence cycle delivers RMC within ~1 s of the
/// first GGA). Time is treated as UTC (NMEA is always UTC).
fn gps_utc_ms_from(parser: &Nmea) -> Option<i64> {
    use chrono::{TimeZone, Utc};
    let date = parser.fix_date?;
    let time = parser.fix_time?;
    let naive = date.and_time(time);
    Some(Utc.from_utc_datetime(&naive).timestamp_millis())
}

fn fix_quality_from(ft: nmea::sentences::FixType) -> Option<u8> {
    use nmea::sentences::FixType::*;
    match ft {
        Invalid => None,
        Gps => Some(1),
        DGps => Some(2),
        Pps => Some(3),
        Rtk => Some(4),
        FloatRtk => Some(5),
        Estimated => Some(6),
        Manual => Some(7),
        Simulation => Some(8),
    }
}

/// ArcSwap helper: read-modify-store the SensorHealth snapshot.
fn set_state(
    health: &Arc<ArcSwap<SensorHealth>>,
    state: SensorState,
    detail: Option<String>,
) {
    let prev = health.load();
    let mut h = (**prev).clone();
    h.state = state;
    h.detail = detail;
    if matches!(state, SensorState::Healthy | SensorState::Degraded) {
        h.last_reading_ms = Some(now_unix_ms());
    }
    health.store(Arc::new(h));
}

fn bump_error(health: &Arc<ArcSwap<SensorHealth>>, detail: String) {
    let prev = health.load();
    let mut h = (**prev).clone();
    h.error_count = h.error_count.saturating_add(1);
    h.detail = Some(detail);
    health.store(Arc::new(h));
}

async fn fire_state_change(ctx: &SensorContext, event_type: &str, success: bool) {
    let ev = SubstrateAuditEvent {
        node_id: ctx.node_id.to_string(),
        timestamp_utc: now_unix_ms() as u64,
        lineage: KITTLER_LINEAGE.to_string(),
        event_type: event_type.to_string(),
        channel: None,
        success: Some(success),
        error_message: None,
        elapsed_ms: None,
    };
    if let Err(e) = ctx.substrate_audit.try_send(ev) {
        eprintln!(
            "[gps] substrate-audit channel full; dropped {} event: {}",
            event_type, e
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a checksum-correct NMEA sentence from its body. NMEA
    /// checksum = XOR of all bytes between `$` and `*` exclusive,
    /// printed as uppercase hex. Computing it at test time keeps the
    /// fixtures self-validating against the parser's checksum gate.
    fn nmea_with_checksum(body: &str) -> String {
        let cs = body.bytes().fold(0u8, |acc, b| acc ^ b);
        format!("${}*{:02X}", body, cs)
    }

    #[test]
    fn parse_gga_with_fix_emits_gpsfix_with_expected_position() {
        // Substrate-truth GGA matching Cardinal's actual position at
        // Saker's Acres (config.yaml node.location). Quality = 1 (GPS
        // fix), 8 satellites, HDOP 0.9, altitude 320 m MSL.
        // Format: GPGGA,UTC,lat,N,lon,W,quality,sats,hdop,alt,M,geoid,M,age,refid
        let line = nmea_with_checksum(
            "GPGGA,123519,4052.9554,N,09541.4552,W,1,08,0.9,320.0,M,46.9,M,,",
        );
        let mut parser = Nmea::default();
        let fix = parse_nmea_line(&mut parser, &line)
            .expect("expected a GpsFix from a fix-quality-1 GGA");
        // 4052.9554 N = 40 + 52.9554/60 = 40.882590°
        assert!(
            (fix.lat - 40.882590).abs() < 0.001,
            "lat outside tolerance: got {}",
            fix.lat
        );
        // 09541.4552 W = -(95 + 41.4552/60) = -95.690920°
        assert!(
            (fix.lon - (-95.690920)).abs() < 0.001,
            "lon outside tolerance: got {}",
            fix.lon
        );
        assert!(
            (fix.alt_m - 320.0).abs() < 1.0,
            "alt outside tolerance: got {}",
            fix.alt_m
        );
        assert_eq!(fix.fix_quality, 1, "fix_quality = 1 (GPS)");
        assert_eq!(fix.sat_count, 8, "sat_count = 8");
        assert!(fix.hdop > 0.0 && fix.hdop < 5.0, "hdop in plausible range");
    }

    /// D32 freshness boundary: parser state is not a new position observation.
    /// PNT-001 9.3/10.2: do not assert fresh timing on an old position.
    /// Failed on the unchanged source (evidence commit 7ab3119); must stay green.
    #[test]
    fn d32_non_position_sentence_does_not_refresh_cached_fix() {
        let mut parser = Nmea::default();
        let gga = nmea_with_checksum(
            "GPGGA,123519,4052.9554,N,09541.4552,W,1,08,0.9,320.0,M,46.9,M,,",
        );
        let first = parse_nmea_line(&mut parser, &gga).expect("valid GGA fixture");
        let vtg = nmea_with_checksum("GPVTG,054.7,T,034.4,M,005.5,N,010.2,K");
        // Verify the fixture is parseable, rather than accepting a checksum or
        // unsupported-sentence error as proof that freshness was preserved.
        assert!(Nmea::default().parse(&vtg).is_ok(), "valid VTG fixture required");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let repeated = parse_nmea_line(&mut parser, &vtg);
        assert!(repeated.is_none(),
            "non-position VTG refreshed cached GGA: original={first:?}, repeated={repeated:?}");
    }

    /// Substrate-truth: a GGA with fix_quality=0 (cold-start receiver,
    /// no satellites locked) must NOT yield a GpsFix. Emitting (0, 0)
    /// or last-known position with quality 0 would seed a false-positive
    /// location into the substrate.
    #[test]
    fn parse_no_fix_gga_returns_none() {
        let cold_start =
            nmea_with_checksum("GPGGA,123519,,,,,0,00,99.99,,,,,,");
        let mut parser = Nmea::default();
        assert!(
            parse_nmea_line(&mut parser, &cold_start).is_none(),
            "GGA with fix_quality=0 must not emit a GpsFix"
        );
    }

    /// D32-F1 also forbids a new incomplete GGA borrowing old coordinates.
    #[test]
    fn d32_incomplete_gga_cannot_borrow_previous_position() {
        let mut parser = Nmea::default();
        let good = nmea_with_checksum("GPGGA,123519,4052.9554,N,09541.4552,W,1,08,0.9,320.0,M,46.9,M,,");
        assert!(parse_nmea_line(&mut parser, &good).is_some());
        let incomplete = nmea_with_checksum("GPGGA,123520,,,,,1,08,0.9,320.0,M,46.9,M,,");
        assert!(nmea::parse_str(&incomplete).is_ok());
        assert!(parse_nmea_line(&mut parser, &incomplete).is_none());
    }

    #[test]
    fn parse_garbage_returns_none() {
        let mut parser = Nmea::default();
        assert!(parse_nmea_line(&mut parser, "not nmea").is_none());
        assert!(parse_nmea_line(&mut parser, "$GPGGA,garbage").is_none());
        assert!(parse_nmea_line(&mut parser, "").is_none());
        assert!(parse_nmea_line(&mut parser, "   ").is_none());
    }

    #[test]
    fn sensor_name_is_stable_canonical_form() {
        let g = NmeaGps::new("/dev/ttyACM1");
        assert_eq!(g.name(), "gps:nmea-0183");
    }

    /// #183 — GPS UTC must be assembled from the RMC date + time-of-day and
    /// surfaced on the GpsFix. RMC alone carries no fix-quality so it emits no
    /// fix; once a GGA with a fix arrives, `gps_utc_ms` reflects the RMC date.
    #[test]
    fn gps_utc_assembled_from_rmc_date_plus_time() {
        let mut parser = Nmea::default();
        // RMC: time 123519 (12:35:19), date 230394 (1994-03-23 UTC).
        let rmc = nmea_with_checksum(
            "GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W",
        );
        // RMC carries no GGA fix-quality → parse_nmea_line emits no GpsFix,
        // but it does accumulate fix_date/fix_time into the parser state.
        let _ = parse_nmea_line(&mut parser, &rmc);
        // Now a fix-quality-1 GGA at the same time-of-day → emits a fix.
        let gga = nmea_with_checksum(
            "GPGGA,123519,4052.9554,N,09541.4552,W,1,08,0.9,320.0,M,46.9,M,,",
        );
        let fix = parse_nmea_line(&mut parser, &gga)
            .expect("GGA with a fix should emit a GpsFix");
        // 1994-03-23T12:35:19Z = 764_426_119_000 ms (hand-computed, no chrono
        // API in the assertion so it can't drift with the crate version).
        assert_eq!(
            fix.gps_utc_ms,
            Some(764_426_119_000),
            "gps_utc_ms must combine RMC date + time as UTC epoch ms"
        );
    }

    /// Substrate-truth: with only a GGA (no RMC seen), there's a time-of-day
    /// but no date — so `gps_utc_ms` MUST be None (we never guess the date).
    #[test]
    fn gps_utc_is_none_without_rmc_date() {
        let mut parser = Nmea::default();
        let gga = nmea_with_checksum(
            "GPGGA,123519,4052.9554,N,09541.4552,W,1,08,0.9,320.0,M,46.9,M,,",
        );
        let fix = parse_nmea_line(&mut parser, &gga).expect("GGA emits a fix");
        assert_eq!(fix.gps_utc_ms, None, "no RMC date → gps_utc_ms None");
    }
}
