// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
// Wave 7.1 — nl80211 substrate primitives for the Kittler Substrate Defense.
//
// Native Generic Netlink channel control + per-channel survey dump +
// monitor-mode establishment. No shell-out to `iw`. The engine binary
// (with cap_net_admin granted via setcap) drives the kernel directly:
//
//   • set_channel            — nl80211-ng NtSocket wrapper for
//                              NL80211_CMD_SET_CHANNEL (Increment 2).
//   • get_survey             — direct neli for NL80211_CMD_GET_SURVEY
//                              (Increment 4).
//   • establish_monitor_mode — Increment 8: down → set type monitor →
//                              up. Retires the operator's manual
//                              `iw dev set type monitor` dance and
//                              gives the Capture Liveness Watchdog
//                              the ability to auto-heal a managed-
//                              mode interface back to monitor.
//   • interface_is_monitor   — Increment 8: query current iftype so
//                              the watchdog can detect when a USB
//                              re-enumeration silently reverted the
//                              interface to managed mode.
//
// Lineage: this is the *active observation* layer of the Kittler
// Substrate Defense. The Hunter rotates the horse (Alfa) to where
// the trail is; get_survey reports back what the trail looks like;
// Inc 8's monitor-mode self-establishment means the horse re-saddles
// itself when the substrate jostles it loose.

use neli::attr::Attribute;
use neli::consts::nl::NlmF;
use neli::consts::socket::NlFamily;
use neli::genl::{AttrTypeBuilder, Genlmsghdr, GenlmsghdrBuilder, NlattrBuilder};
use neli::nl::NlPayload;
use neli::router::synchronous::NlRouter;
use neli::types::GenlBuffer;
use neli::utils::Groups;
use nl80211_ng::attr::{
    Nl80211Attr, Nl80211ChanWidth, Nl80211ChannelType, Nl80211Iftype, Nl80211SurveyInfo,
};
use nl80211_ng::cmd::Nl80211Cmd;
use nl80211_ng::ntsocket::NtSocket;
use nl80211_ng::rtsocket::RtSocket;
use nl80211_ng::{NL_80211_GENL_NAME, NL_80211_GENL_VERSION};

/// Convert a Wi-Fi channel number to its center frequency in MHz.
///
/// Returns Err for channels that are not part of the US/EU regulatory
/// domain we currently scan (2.4 GHz channels 1–14, 5 GHz UNII-1
/// 36–48, UNII-2 52–144, UNII-3 149–165). Future expansion can add
/// 6 GHz UNII-5+ once we get into Wi-Fi 6E drone control links.
pub fn channel_to_freq_mhz(channel: u32) -> Result<u32, String> {
    match channel {
        // 2.4 GHz: special-case channel 14 (Japan-only), formula otherwise
        1..=13 => Ok(2407 + 5 * channel),
        14 => Ok(2484),
        // 5 GHz: uniform formula across UNII-1/2/3
        36..=64 | 100..=144 | 149..=165 => Ok(5000 + 5 * channel),
        _ => Err(format!(
            "channel {} is not in the supported regulatory range",
            channel
        )),
    }
}

/// Convert a center frequency in MHz back to a Wi-Fi channel number.
/// Used when interpreting per-channel survey results that the kernel
/// returns in MHz units.
pub fn freq_mhz_to_channel(freq_mhz: u32) -> Option<u32> {
    if (2412..=2472).contains(&freq_mhz) && (freq_mhz - 2407) % 5 == 0 {
        Some((freq_mhz - 2407) / 5)
    } else if freq_mhz == 2484 {
        Some(14)
    } else if (5180..=5825).contains(&freq_mhz) && (freq_mhz - 5000) % 5 == 0 {
        Some((freq_mhz - 5000) / 5)
    } else {
        None
    }
}

/// Set the operating channel of a wireless interface to the given
/// channel number, using NL80211_CMD_SET_CHANNEL with a 20 MHz no-HT
/// width (substrate-honest minimum: capture works for any wider
/// emission, no need to commit to HT40/VHT80 mode at this layer).
///
/// This is a synchronous netlink call. The Hunter task should invoke
/// it via `tokio::task::spawn_blocking` to avoid stalling the async
/// runtime; the call typically completes in <10 ms but the netlink
/// socket is sync.
///
/// Returns Ok on driver acceptance of the channel-set. Note that
/// "driver accepted" is not the same as "channel actually changed in
/// hardware" — DFS channels (52–144) may be silently refused or
/// require radar-clear before the radio actually tunes. Verification
/// is the Hunter task's responsibility, not this primitive's.
pub fn set_channel(ifindex: u32, channel: u32) -> Result<(), String> {
    let freq_mhz = channel_to_freq_mhz(channel)?;
    let mut sock = NtSocket::connect()
        .map_err(|e| format!("nl80211 socket connect failed: {}", e))?;
    sock.set_frequency(
        ifindex,
        freq_mhz,
        Nl80211ChanWidth::ChanWidth20Noht,
        Nl80211ChannelType::ChanNoHt,
    )
    .map_err(|e| format!("set_channel(ch={}, freq={}MHz): {}", channel, freq_mhz, e))
}

/// One entry in the per-channel survey response. Each represents the
/// kernel-driver's accumulated counters and most-recent noise reading
/// for a single frequency at the moment of the dump.
///
/// Substrate-truth caveat: `time_active_ms`, `time_busy_ms`, etc. are
/// monotonic counters since the radio started, NOT instantaneous
/// percentages. To compute "channel busy %", take two samples
/// separated in time and diff them. The Hunter task does this at
/// end-of-dwell to produce per-cycle deltas that flow into heartbeat
/// (Increment 5).
///
/// `noise_dbm` IS instantaneous (the kernel reports last-measured),
/// and the unit is dBm (signed; typical quiet floor in 5GHz UNII-3 is
/// around -97 dBm; jamming saturates toward -60 dBm).
#[derive(Clone, Debug, Default)]
pub struct SurveyEntry {
    pub frequency_mhz: u32,
    pub channel: Option<u32>,
    pub in_use: bool,
    pub noise_dbm: Option<i8>,
    pub time_active_ms: Option<u64>,
    pub time_busy_ms: Option<u64>,
    pub time_rx_ms: Option<u64>,
    pub time_tx_ms: Option<u64>,
}

/// Issue NL80211_CMD_GET_SURVEY (dump mode) for the given interface
/// index and collect all per-channel survey entries the kernel
/// returns.
///
/// This is a synchronous netlink call. Hunter task wraps it in
/// `tokio::task::spawn_blocking`. Typical end-to-end latency is
/// 5–15 ms on a CM4-class brick; the response payload size scales
/// with the number of supported channels (24+ entries on a tri-band
/// Alfa).
///
/// Returns Ok with the list of entries on success. Entries with
/// `frequency_mhz == 0` are filtered out (kernel sometimes sends
/// padding entries during driver init; substrate-truth filter).
pub fn get_survey(ifindex: u32) -> Result<Vec<SurveyEntry>, String> {
    let (sock, _) = NlRouter::connect(NlFamily::Generic, None, Groups::empty())
        .map_err(|e| format!("nl80211 socket connect: {}", e))?;
    let family_id = sock
        .resolve_genl_family(NL_80211_GENL_NAME)
        .map_err(|e| format!("resolve nl80211 family: {}", e))?;
    // The router adds REQUEST and validates sequence/PID, dump completion and
    // kernel errors. No ACK is requested, matching the existing dump protocol.
    let responses = sock
        .send::<_, _, u16, Genlmsghdr<u8, u16>>(
            family_id,
            NlmF::DUMP,
            NlPayload::Payload(survey_request(ifindex)?),
        )
        .map_err(|e| format!("CMD_GET_SURVEY send: {}", e))?;
    let mut entries = Vec::new();
    for response in responses {
        let response = response.map_err(|e| format!("CMD_GET_SURVEY recv: {}", e))?;
        if let NlPayload::Payload(payload) = response.nl_payload() {
            if let Some(entry) = parse_survey(payload) {
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

// nl80211-ng still uses neli 0.6. Cross the version boundary using the kernel
// u8/u16 wire IDs, not that dependency's version-specific serialization traits.
fn survey_request(ifindex: u32) -> Result<Genlmsghdr<u8, u16>, String> {
    let attr_type = AttrTypeBuilder::default()
        .nla_type(u16::from(Nl80211Attr::AttrIfindex))
        .build()
        .map_err(|e| format!("build AttrIfindex type: {}", e))?;
    let attr = NlattrBuilder::default()
        .nla_type(attr_type)
        .nla_payload(ifindex)
        .build()
        .map_err(|e| format!("build AttrIfindex: {}", e))?;
    GenlmsghdrBuilder::default()
        .cmd(u8::from(Nl80211Cmd::CmdGetSurvey))
        .version(NL_80211_GENL_VERSION)
        .attrs(std::iter::once(attr).collect::<GenlBuffer<_, _>>())
        .build()
        .map_err(|e| format!("build CMD_GET_SURVEY: {}", e))
}

fn parse_survey(payload: &Genlmsghdr<u8, u16>) -> Option<SurveyEntry> {
    if *payload.cmd() != u8::from(Nl80211Cmd::CmdNewSurveyResults) {
        return None;
    }
    let handle = payload.attrs().get_attr_handle();
    let survey_attr = handle.get_attribute(u16::from(Nl80211Attr::AttrSurveyInfo))?;
    let nested = survey_attr.get_attr_handle::<u16>().ok()?;
    let mut entry = SurveyEntry::default();
    for inner in nested.get_attrs() {
        match Nl80211SurveyInfo::from(*inner.nla_type().nla_type()) {
            Nl80211SurveyInfo::SurveyInfoFrequency => {
                if let Ok(v) = inner.get_payload_as::<u32>() {
                    entry.frequency_mhz = v;
                    entry.channel = freq_mhz_to_channel(v);
                }
            }
            Nl80211SurveyInfo::SurveyInfoNoise => {
                if let Ok(v) = inner.get_payload_as::<i8>() {
                    entry.noise_dbm = Some(v);
                }
            }
            Nl80211SurveyInfo::SurveyInfoInUse => {
                // Flag attribute (zero-length); presence == true.
                entry.in_use = true;
            }
            Nl80211SurveyInfo::SurveyInfoTime => {
                if let Ok(v) = inner.get_payload_as::<u64>() {
                    entry.time_active_ms = Some(v);
                }
            }
            Nl80211SurveyInfo::SurveyInfoTimeBusy => {
                if let Ok(v) = inner.get_payload_as::<u64>() {
                    entry.time_busy_ms = Some(v);
                }
            }
            Nl80211SurveyInfo::SurveyInfoTimeRx => {
                if let Ok(v) = inner.get_payload_as::<u64>() {
                    entry.time_rx_ms = Some(v);
                }
            }
            Nl80211SurveyInfo::SurveyInfoTimeTx => {
                if let Ok(v) = inner.get_payload_as::<u64>() {
                    entry.time_tx_ms = Some(v);
                }
            }
            _ => {}
        }
    }
    (entry.frequency_mhz != 0).then_some(entry)
}

/// Wave 7.1 Inc 8 — query whether an interface is currently in
/// monitor mode. The Capture Liveness Watchdog uses this to detect
/// the substrate-truth condition where a USB re-enumeration silently
/// reverted the interface to `managed` mode (the 2026-05-13 Alfa
/// cable-bump lesson).
///
/// Returns Ok(true) if the interface is in IftypeMonitor, Ok(false)
/// if it exists but is in any other mode, Err if the interface
/// can't be found or the netlink query fails.
pub fn interface_is_monitor(ifindex: u32) -> Result<bool, String> {
    let mut sock = NtSocket::connect()
        .map_err(|e| format!("nl80211 socket connect: {}", e))?;
    let interfaces = sock
        .cmd_get_interfaces()
        .map_err(|e| format!("CMD_GET_INTERFACE: {}", e))?;
    // cmd_get_interfaces is keyed by wiphy; find ours by ifindex.
    let iface = interfaces
        .values()
        .find(|i| i.index == Some(ifindex))
        .ok_or_else(|| format!("ifindex {} not found in nl80211 interface list", ifindex))?;
    Ok(iface.current_iftype == Some(Nl80211Iftype::IftypeMonitor))
}

/// Wave 7.1 Inc 8 — establish monitor mode on an interface.
///
/// Canonical sequence (the substrate-honest equivalent of the
/// operator's old `iw dev <iface> set type monitor` dance):
///   1. RtSocket: bring the interface DOWN
///   2. NtSocket: NL80211_CMD_SET_INTERFACE → IftypeMonitor
///   3. RtSocket: bring the interface UP
///   4. VERIFY the end-state, and retry the whole sequence if it
///      did not stick.
///
/// ─────────────────────────────────────────────────────────────────
///   The 2026-05-14 First-Breath lesson (substrate-truth, recursive):
///
///   The first cut of this function trusted nl80211-ng's return
///   values. It does not deserve that trust:
///
///   (a) `set_type_vec(.., active=true)` builds an NL80211_ATTR_
///       MNTR_FLAGS nested attribute (active-monitor mode). Some
///       driver/kernel combos — including the rtw88_8812au on
///       test-node-2 — reject the whole SET_INTERFACE command when that
///       attribute is present. We don't NEED active monitor: the
///       sensor only listens, it never ACKs frames. So we pass
///       `active=false`, which makes the wire-format identical to a
///       plain `iw dev <iface> set type monitor`.
///
///   (b) nl80211-ng's `set_type_vec` drains its response with
///       `iter.flatten()`, which silently DROPS kernel-rejection
///       messages — so it returns `Ok(())` even when the command
///       failed. And `RtSocket::set_interface_down` can return
///       before the kernel has finished processing the down (the
///       kernel then rejects the iftype change because the link is
///       still up).
///
///   The only substrate-truth here is the *observed end-state*. So
///   this function settles between steps, then re-reads the actual
///   iftype via `interface_is_monitor`, and retries the sequence up
///   to MAX_ATTEMPTS times. If it still cannot establish monitor
///   mode, it returns Err — and the engine reports radio_status:
///   error honestly, rather than the silent 18-hour lie.
/// ─────────────────────────────────────────────────────────────────
///
/// Requires CAP_NET_ADMIN (verified at engine startup by
/// capabilities::check_required). Used both at engine startup
/// (retiring the manual setup dance) and by the Capture Liveness
/// Watchdog when it detects a managed-mode reversion.
pub fn establish_monitor_mode(ifindex: u32) -> Result<(), String> {
    use std::thread::sleep;
    use std::time::Duration;

    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err = String::from("(no attempt made)");

    for attempt in 1..=MAX_ATTEMPTS {
        if attempt > 1 {
            eprintln!(
                "[nl80211] establish_monitor_mode(ifindex={}) retry {}/{} — prior: {}",
                ifindex, attempt, MAX_ATTEMPTS, last_err
            );
        }

        // ── Step 1: bring the link DOWN ──
        // The kernel rejects SET_INTERFACE→monitor on an up interface
        // (this is why `iw` itself requires `ip link set down` first).
        // nl80211-ng's RtSocket can return before the down has fully
        // landed, so we settle afterward.
        match RtSocket::connect() {
            Ok(mut rt) => {
                if let Err(e) = rt.set_interface_down(ifindex) {
                    last_err = format!("set_interface_down(ifindex={}): {}", ifindex, e);
                    sleep(Duration::from_millis(250));
                    continue;
                }
            }
            Err(e) => {
                last_err = format!("rtnetlink connect (down): {}", e);
                sleep(Duration::from_millis(250));
                continue;
            }
        }
        sleep(Duration::from_millis(300));

        // ── Step 2: SET_INTERFACE → monitor (plain, no active flag) ──
        // active=false: wire-identical to `iw dev <iface> set type
        // monitor`. We deliberately do NOT trust this call's return
        // value (see the function doc) — Step 4's verification is the
        // real gate. But a hard Err is still worth recording.
        match NtSocket::connect() {
            Ok(mut nt) => {
                if let Err(e) =
                    nt.set_type_vec(ifindex, Nl80211Iftype::IftypeMonitor, false)
                {
                    last_err = format!("set_type monitor(ifindex={}): {}", ifindex, e);
                    // Do not `continue` — still bring the link back up
                    // below so we never leave it stranded down.
                }
            }
            Err(e) => {
                last_err = format!("nl80211 connect (set_type): {}", e);
            }
        }
        sleep(Duration::from_millis(100));

        // ── Step 3: bring the link back UP ──
        match RtSocket::connect() {
            Ok(mut rt_up) => {
                if let Err(e) = rt_up.set_interface_up(ifindex) {
                    last_err = format!("set_interface_up(ifindex={}): {}", ifindex, e);
                }
            }
            Err(e) => {
                last_err = format!("rtnetlink connect (up): {}", e);
            }
        }
        sleep(Duration::from_millis(250));

        // ── Step 4: VERIFY the observed end-state ──
        match interface_is_monitor(ifindex) {
            Ok(true) => return Ok(()),
            Ok(false) => {
                last_err = format!(
                    "interface still in non-monitor mode after attempt {}",
                    attempt
                );
            }
            Err(e) => {
                last_err = format!("post-set monitor-mode verification failed: {}", e);
            }
        }
        sleep(Duration::from_millis(200));
    }

    Err(format!(
        "establish_monitor_mode(ifindex={}) failed after {} attempts: {}",
        ifindex, MAX_ATTEMPTS, last_err
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use neli::{FromBytesWithInput, ToBytes};
    use std::io::Cursor;

    // Hand-encoded Linux nl80211 ABI fixtures, independent of neli's builders.
    fn wire_attr(id: u16, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((payload.len() + 4) as u16).to_ne_bytes());
        bytes.extend_from_slice(&id.to_ne_bytes());
        bytes.extend_from_slice(payload);
        bytes.resize((bytes.len() + 3) & !3, 0);
        bytes
    }

    fn wire_survey(cmd: u8, attrs: &[u8]) -> Genlmsghdr<u8, u16> {
        let mut bytes = vec![cmd, 1, 0, 0];
        bytes.extend_from_slice(attrs);
        Genlmsghdr::from_bytes_with_input(&mut Cursor::new(&bytes), bytes.len()).unwrap()
    }

    fn survey_fields(fields: &[Vec<u8>]) -> Genlmsghdr<u8, u16> {
        wire_survey(51, &wire_attr(84 | 0x8000, &fields.concat()))
    }

    #[test]
    fn survey_request_preserves_kernel_wire_abi() {
        let mut wire = Cursor::new(Vec::new());
        survey_request(0x01020304).unwrap().to_bytes(&mut wire).unwrap();
        let mut expected = vec![50, 1, 0, 0];
        expected.extend(wire_attr(3, &0x01020304_u32.to_ne_bytes()));
        assert_eq!(wire.into_inner(), expected);
    }

    #[test]
    fn survey_decodes_signed_noise_flag_and_64_bit_counters() {
        let payload = survey_fields(&[
            wire_attr(1, &5180_u32.to_ne_bytes()),
            wire_attr(2, &(-97_i8).to_ne_bytes()),
            wire_attr(3, &[]),
            wire_attr(4, &(u32::MAX as u64 + 100).to_ne_bytes()),
            wire_attr(5, &1234_u64.to_ne_bytes()),
            wire_attr(7, &567_u64.to_ne_bytes()),
            wire_attr(8, &89_u64.to_ne_bytes()),
            wire_attr(999, &[1, 2, 3]),
        ]);
        let entry = parse_survey(&payload).unwrap();
        assert_eq!(entry.frequency_mhz, 5180);
        assert_eq!(entry.channel, Some(36));
        assert_eq!(entry.noise_dbm, Some(-97));
        assert!(entry.in_use);
        assert_eq!(entry.time_active_ms, Some(u32::MAX as u64 + 100));
        assert_eq!(entry.time_busy_ms, Some(1234));
        assert_eq!(entry.time_rx_ms, Some(567));
        assert_eq!(entry.time_tx_ms, Some(89));
    }

    #[test]
    fn survey_ignores_missing_zero_or_malformed_frequency() {
        for fields in [vec![], vec![wire_attr(1, &0_u32.to_ne_bytes())],
                       vec![wire_attr(1, &[1, 2])]] {
            assert!(parse_survey(&survey_fields(&fields)).is_none());
        }
    }

    #[test]
    fn survey_ignores_other_commands_missing_and_malformed_nesting() {
        let attrs = wire_attr(84 | 0x8000, &wire_attr(1, &2412_u32.to_ne_bytes()));
        assert!(parse_survey(&wire_survey(50, &attrs)).is_none());
        assert!(parse_survey(&wire_survey(51, &[])).is_none());
        assert!(parse_survey(&wire_survey(51, &wire_attr(84 | 0x8000, &[8, 0, 1]))).is_none());
    }

    #[test]
    fn survey_optional_malformed_values_stay_unknown() {
        let payload = survey_fields(&[
            wire_attr(1, &2412_u32.to_ne_bytes()), wire_attr(2, &[]),
            wire_attr(4, &[1, 2]), wire_attr(5, &[]),
            wire_attr(7, &[1]), wire_attr(8, &[1, 2, 3]),
        ]);
        let entry = parse_survey(&payload).unwrap();
        assert_eq!(entry.channel, Some(1));
        assert!(!entry.in_use);
        assert_eq!(entry.noise_dbm, None);
        assert_eq!(entry.time_active_ms, None);
        assert_eq!(entry.time_busy_ms, None);
        assert_eq!(entry.time_rx_ms, None);
        assert_eq!(entry.time_tx_ms, None);
    }

    #[test]
    fn survey_malformed_duplicate_does_not_erase_valid_counter() {
        let payload = survey_fields(&[
            wire_attr(1, &2412_u32.to_ne_bytes()),
            wire_attr(5, &42_u64.to_ne_bytes()), wire_attr(5, &[1]),
        ]);
        assert_eq!(parse_survey(&payload).unwrap().time_busy_ms, Some(42));
    }

    #[test]
    fn channel_to_freq_us_24ghz() {
        assert_eq!(channel_to_freq_mhz(1).unwrap(), 2412);
        assert_eq!(channel_to_freq_mhz(6).unwrap(), 2437);
        assert_eq!(channel_to_freq_mhz(11).unwrap(), 2462);
    }

    #[test]
    fn channel_to_freq_us_5ghz_unii1() {
        assert_eq!(channel_to_freq_mhz(36).unwrap(), 5180);
        assert_eq!(channel_to_freq_mhz(48).unwrap(), 5240);
    }

    #[test]
    fn channel_to_freq_us_5ghz_unii3() {
        assert_eq!(channel_to_freq_mhz(149).unwrap(), 5745);
        assert_eq!(channel_to_freq_mhz(165).unwrap(), 5825);
    }

    #[test]
    fn channel_14_japan_only() {
        assert_eq!(channel_to_freq_mhz(14).unwrap(), 2484);
    }

    #[test]
    fn invalid_channels_rejected() {
        assert!(channel_to_freq_mhz(0).is_err());
        assert!(channel_to_freq_mhz(15).is_err());   // gap between 2.4 and 5
        assert!(channel_to_freq_mhz(35).is_err());   // gap below UNII-1
        assert!(channel_to_freq_mhz(166).is_err());  // above UNII-3
        assert!(channel_to_freq_mhz(200).is_err());
    }

    #[test]
    fn freq_to_channel_round_trip() {
        for ch in [1, 6, 11, 36, 40, 44, 48, 149, 153, 157, 161, 165] {
            let freq = channel_to_freq_mhz(ch).unwrap();
            assert_eq!(freq_mhz_to_channel(freq), Some(ch));
        }
    }
}
