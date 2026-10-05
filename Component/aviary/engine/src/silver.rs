// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! D32 Silver v1: system health, not operational telemetry. A configured
//! installation is never a replacement for a current position observation.
use serde::{Deserialize, Serialize};

use crate::sensor::SensorState;
use crate::sensor_gps::GpsFix;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConfiguredPosition {
    pub latitude: f64,
    pub longitude: f64,
    pub elevation_meters: f32,
    pub source: ConfigSource,
    // No observed_at: config load is not a survey or a position measurement.
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    ConfigStatic,
}

#[derive(Clone)]
pub struct Context {
    pub configured_position: ConfiguredPosition,
    pub config_hash: Option<String>,
    pub fresh_for_ms: i64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ObservationTimeSource {
    GpsUtc,
    SystemReceive,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CurrentPosition {
    pub latitude: f64,
    pub longitude: f64,
    pub source: LiveSource,
    pub observed_at_ms: i64,
    pub received_at_ms: i64,
    pub time_source: ObservationTimeSource,
    // Horizontal position only. Legacy GpsFix altitude lacks presence metadata;
    // do not publish its missing-altitude zero as a Silver measurement.
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LiveSource {
    GpsLive,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PositionState {
    Current,
    NoFix,
    Stale,
    InvalidFix,
    FutureObservation,
    ClockUntrusted,
    SensorUnavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PositionStatus {
    pub state: PositionState,
    pub fresh_for_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_source: Option<ObservationTimeSource>,
}

pub fn valid_coordinates(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
        && !(lat == 0.0 && lon == 0.0)
    // Known-default classifier is owed, not an empty registry: D30 Amendment 1,
    // governance/2026-09-19-OPEN-DIMENSIONS-REGISTER.md Q-9. No approved points.
}

pub fn position(
    fix: Option<&GpsFix>,
    health: Option<SensorState>,
    clock_trusted: bool,
    now_ms: i64,
    fresh_for_ms: i64,
) -> (Option<CurrentPosition>, PositionStatus) {
    let fresh_for_ms = fresh_for_ms.max(1);
    let mut status = PositionStatus {
        state: PositionState::NoFix,
        fresh_for_ms,
        observed_at_ms: None,
        received_at_ms: None,
        time_source: None,
    };
    let Some(fix) = fix else {
        return (None, status);
    };
    let (observed, source) = match fix.gps_utc_ms {
        Some(time) => (time, ObservationTimeSource::GpsUtc),
        None => (fix.fix_at_ms, ObservationTimeSource::SystemReceive),
    };
    status.observed_at_ms = Some(observed);
    status.received_at_ms = Some(fix.fix_at_ms);
    status.time_source = Some(source);
    status.state = if !valid_coordinates(fix.lat, fix.lon)
        || !(1..=5).contains(&fix.fix_quality)
        || observed <= 0
        || fix.fix_at_ms <= 0
    {
        PositionState::InvalidFix
    } else if !clock_trusted {
        PositionState::ClockUntrusted
    } else if observed > now_ms || fix.fix_at_ms > now_ms {
        PositionState::FutureObservation
    } else if now_ms.saturating_sub(observed) > fresh_for_ms
        || now_ms.saturating_sub(fix.fix_at_ms) > fresh_for_ms
    {
        PositionState::Stale
    } else if health != Some(SensorState::Healthy) {
        PositionState::SensorUnavailable
    } else {
        PositionState::Current
    };
    let current = if status.state == PositionState::Current {
        Some(CurrentPosition {
            latitude: fix.lat,
            longitude: fix.lon,
            source: LiveSource::GpsLive,
            observed_at_ms: observed,
            received_at_ms: fix.fix_at_ms,
            time_source: source,
        })
    } else {
        None
    };
    (current, status)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ManagementStatus {
    Blocked,
    Attention,
    Ok,
}

pub fn management(
    blocked_reason: Option<&str>,
    position: PositionState,
    radio_up: bool,
    clock_trusted: bool,
) -> ManagementStatus {
    if blocked_reason.is_some() {
        ManagementStatus::Blocked
    } else if position != PositionState::Current || !radio_up || !clock_trusted {
        ManagementStatus::Attention
    } else {
        ManagementStatus::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_swap::ArcSwap;
    use std::sync::Arc;

    fn fix(at: i64) -> GpsFix {
        GpsFix {
            lat: 40.1234567,
            lon: -95.7654321,
            alt_m: 300.0,
            fix_quality: 1,
            sat_count: 8,
            hdop: 0.9,
            fix_at_ms: at,
            gps_utc_ms: Some(at),
        }
    }

    #[test]
    fn observation_ages_without_coordinate_motion_and_never_falls_back() {
        let fix = fix(100_000);
        let (live, status) = position(
            Some(&fix),
            Some(SensorState::Healthy),
            true,
            100_100,
            30_000,
        );
        assert_eq!(live.as_ref().unwrap().observed_at_ms, 100_000);
        assert_eq!(live.as_ref().unwrap().latitude, fix.lat);
        assert_eq!(status.state, PositionState::Current);
        let (expired, stale) = position(
            Some(&fix),
            Some(SensorState::Healthy),
            true,
            130_001,
            30_000,
        );
        assert!(expired.is_none(), "stale coordinate must be absent");
        assert_eq!(stale.state, PositionState::Stale);
        assert_eq!(
            stale.observed_at_ms,
            Some(100_000),
            "old observation must not be restamped"
        );
        assert!(position(
            Some(&fix),
            Some(SensorState::Healthy),
            true,
            130_000,
            30_000
        )
        .0
        .is_some());
        let stationary_new_observation = GpsFix {
            fix_at_ms: 130_001,
            gps_utc_ms: Some(130_001),
            ..fix
        };
        assert!(position(
            Some(&stationary_new_observation),
            Some(SensorState::Healthy),
            true,
            130_001,
            30_000
        )
        .0
        .is_some());
    }

    #[test]
    fn malformed_future_untrusted_and_non_gnss_are_not_current() {
        let base = fix(100_000);
        for bad in [
            GpsFix {
                lat: 0.0,
                lon: 0.0,
                ..base.clone()
            },
            GpsFix {
                lat: 91.0,
                ..base.clone()
            },
            GpsFix {
                lon: f64::NAN,
                ..base.clone()
            },
            GpsFix {
                fix_quality: 0,
                ..base.clone()
            },
            GpsFix {
                fix_quality: 7,
                ..base.clone()
            },
            GpsFix {
                fix_at_ms: i64::MIN,
                ..base.clone()
            },
        ] {
            let (pos, state) = position(
                Some(&bad),
                Some(SensorState::Healthy),
                true,
                100_000,
                30_000,
            );
            assert!(pos.is_none());
            assert_eq!(state.state, PositionState::InvalidFix);
        }
        for (sample, trusted, health, expected) in [
            (
                fix(100_001),
                true,
                Some(SensorState::Healthy),
                PositionState::FutureObservation,
            ),
            (
                base.clone(),
                false,
                Some(SensorState::Healthy),
                PositionState::ClockUntrusted,
            ),
            (
                base.clone(),
                true,
                Some(SensorState::Degraded),
                PositionState::SensorUnavailable,
            ),
            (
                GpsFix {
                    gps_utc_ms: Some(1),
                    ..base.clone()
                },
                true,
                Some(SensorState::Healthy),
                PositionState::Stale,
            ),
        ] {
            let (pos, status) = position(Some(&sample), health, trusted, 100_000, 30_000);
            assert!(pos.is_none());
            assert_eq!(status.state, expected);
        }
        assert_eq!(
            position(
                None,
                Some(SensorState::Initializing),
                false,
                100_000,
                30_000
            )
            .1
            .state,
            PositionState::NoFix
        );
    }

    #[test]
    fn refusal_is_management_status_even_with_healthy_radio_and_position() {
        assert_eq!(
            management(
                Some(crate::identity::UPDATE_BLOCKED_REASON),
                PositionState::Current,
                true,
                true
            ),
            ManagementStatus::Blocked
        );
        assert_eq!(
            management(None, PositionState::Current, true, true),
            ManagementStatus::Ok
        );
        assert_eq!(
            management(None, PositionState::NoFix, true, true),
            ManagementStatus::Attention
        );
    }

    // Real heartbeat serialization, no broker, no real config/credentials. The
    // optional output directory feeds the independent JSON/schema view tests.
    #[test]
    fn heartbeat_only_position_health_and_refusal_fixtures() {
        use crate::heartbeat::{build_payload, FleetProprioception, RadioState};
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64
        };
        let trust = Arc::new(crate::clock_discipline::TimeTrust::new());
        let mut clock = crate::node_config::ClockYaml::default();
        clock.max_step_secs = 1; // Any step-worthy skew is refused before a syscall.
        for _ in 0..clock.consistent_fixes {
            crate::clock_discipline::discipline_from_gps(now(), &trust, &clock);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(trust.is_trusted());
        let latest = Arc::new(ArcSwap::from_pointee(None));
        let mut health = crate::sensor::SensorHealth::initializing("fixture-gps");
        health.state = SensorState::Healthy;
        let health = Arc::new(ArcSwap::from_pointee(health));
        let fleet = FleetProprioception {
            identity: crate::identity::RunningIdentity::unverified(),
            channel: "stable".into(),
            time_trust: trust,
            silver: Some(Context {
                configured_position: ConfiguredPosition {
                    latitude: 41.0,
                    longitude: -96.0,
                    elevation_meters: 320.0,
                    source: ConfigSource::ConfigStatic,
                },
                config_hash: Some(format!("sha256:{}", "a".repeat(64))),
                fresh_for_ms: 30_000,
            }),
        };
        for (name, sample, expected) in [
            ("live", Some(fix(now())), PositionState::Current),
            ("stale", Some(fix(now() - 3_600_000)), PositionState::Stale),
            ("no_fix", None, PositionState::NoFix),
            ("recovering", Some(fix(now())), PositionState::Current),
        ] {
            latest.store(Arc::new(sample));
            let radio = RadioState::new();
            radio.set(if name == "recovering" {
                crate::heartbeat::RadioStatus::Recovering
            } else {
                crate::heartbeat::RadioStatus::Up
            });
            let hunter = crate::hunter::HunterState::new();
            let hb = build_payload(
                "d32-fixture",
                std::time::Instant::now(),
                &radio,
                Some(&hunter),
                Some("d32-fixture-no-device"),
                None,
                Some(&health),
                Some(&latest),
                Some(&fleet),
            );
            assert_eq!(hb.position_status.as_ref().unwrap().state, expected);
            assert_eq!(hb.management_status, Some(ManagementStatus::Blocked));
            assert_eq!(
                hb.update_blocked_reason.as_deref(),
                Some(crate::identity::UPDATE_BLOCKED_REASON)
            );
            let bytes = serde_json::to_vec_pretty(&hb).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            for key in ["engine_version", "image_digest", "build_seq", "policy_ack"] {
                assert!(value.get(key).is_none(), "D26 field fabricated: {key}");
            }
            if expected != PositionState::Current {
                assert!(
                    value.get("current_position").is_none(),
                    "static or stale promoted to current"
                );
                assert!(value.get("node_position_source").is_none());
            } else {
                assert_eq!(value["current_position"]["latitude"], 40.1234567);
            }
            assert_eq!(value["configured_position"]["source"], "config_static");
            assert!(value["configured_position"].get("observed_at_ms").is_none());
            if let Some(dir) = std::env::var_os("D32_PROOF_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    std::path::Path::new(&dir).join(format!("{name}.json")),
                    bytes,
                )
                .unwrap();
            }
        }
    }
}
