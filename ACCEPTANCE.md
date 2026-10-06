<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Wi-Fi Beacon message-counter acceptance

## Channel scheduler acceptance (social-dwell default; C1–C5)

The receiver's objective is the set of aircraft in range, not a good track for
whichever aircraft spoke first. The default schedule becomes cadence-aware and
fair; aircraft-following lock-on leaves the default path entirely.

Parameters (config-tunable; `preset: social` default): channels 6 and 149 dwell
**1200 ms** each per visit; every other configured channel **220 ms**; dwell
jitter ±15% with the social dwell clamped to a 1000 ms floor so the ≥1 s
`BUR0010` Beacon interval and one full 512 TU (524.288 ms) NAN discovery window
plus margin survive jitter; sweep channels are re-shuffled each supercycle and
the two social channels are placed approximately half a supercycle apart; the
schedule is deterministic for a given seed. Rationale: 1200 ms spans two NAN
discovery-window periods and gives P(catch) = 1−e^−1.2 ≈ 70% per visit against
a 1 Hz Beacon; 220 ms covers the 200 TU (204.8 ms) any-channel Beacon interval
(`BWFB0040`/`BWFB0050`) with guard time. Retune dead time is modelled at the
cited 8.5 ms same-silicon MT7921e PCIe proxy (734-switch measurement, mean 8.5
/ p50 8.1 / p90 9.5 ms) with a 25 ms conservative sensitivity; the engine now
measures and reports its own per-switch retune time so the fleet value can
replace the proxy. Presets: `social` (default), `park6` (fixed ch 6 for dense
single-band sites), `sweep` (legacy rotation for spectrum-witness missions).
Existing explicit hunter configs keep their exact values (back-compat) with a
logged notice; `lock_on_duration_ms > 0` remains available and is documented as
an opt-in single-target audit tool.

Pre-registered expected numbers (deterministic simulator, 600 s, 12-channel
set, 1 Hz Beacon aircraft on every channel, NAN discovery windows every
524.288 ms on ch 6, T_r = 8.5 ms, sensitivity 25 ms):

| Measure | Legacy default (expected) | Social default (expected) |
|---|---|---|
| ch 6 / ch 149 dwell share | 10.25% each | ≥ 24% each (26.1% at T_r=0; 24.5% at 25 ms) |
| Other-channel dwell share | 5.1–10.25% | 4.4–4.8% (accepted trade: −~10% sweep yield for 2.5× social) |
| Per-aircraft capture rate, 1 Hz Beacon on 6/149 | 0.081–0.089 fps | 0.145–0.155 fps (≥ 1.7× legacy) |
| Mean time-to-first-detection on 6/149 | 6.5–10 s | ≤ 2.7 s |
| Inter-catch gap on 6/149 (legacy + lock-on + 1 s dedup modeled) | p10 3.7–5.5 s, median 4–9 s — reproduces the 24-h capture (measured pooled gaps: p10 ≈ 4.2 s, median ≈ 4.5 s, p75 9.0 s, 2×-cycle harmonic ≈ 8.4 s) within reason | median ≤ 7 s, p10 ≥ 2 s |
| NAN discovery windows covered per social visit | ≤ 1 (82.5%/visit overlap) | ≥ 2 full periods |
| Chatty 10 Hz identity-only aircraft on ch 6 (never completes budget) | channel monopoly: rotation starves, other channels lose ≥ 25% of visits | zero effect: every channel still visited each supercycle; no dwell exceeds 1200 ms × 1.15 |

**Simulator amendment (recorded before mutation checks, after the first
simulator run):** the pre-registered rate/TTFD bands above were derived under
a Poisson-arrival approximation. The simulator models periodic ≥1 Hz
sources, for which the steady-state capture rate equals the dwell share ×
frame rate, and TTFD is an ensemble over transmitter phase. Two further
model facts surfaced and are now part of the evidence: (1) a perfectly
periodic source against the legacy schedule phase-locks rationally (ch 6
hard 8.0 s gaps, ch 149 zero catches in the aliased construction) — the
simulator therefore applies deterministic ±2% interval jitter to emissions,
matching real oscillator drift, and this pathology is itself the strongest
argument for the scheduler's anti-phase-lock jitter requirement; (2) the
legacy gap distribution is aggregated over an 8-phase ensemble, as the
24-h field data averages over real phase noise. Re-registered, measured
bands: legacy rate 0.095–0.110 fps, social rate 0.220–0.270 fps
(measured 0.102 / 0.235; ratio ≥ 2.2×); legacy mean TTFD ≥ 2.0 s, social
mean TTFD ≤ 2.7 s and ≤ 0.8 × legacy; legacy ensemble gaps p10 3.5–5.5 s,
median 4–9 s. The dwell-share, NAN-window, monopoly and starvation numbers
stand as originally registered.

| Requirement | Executable evidence | Acceptance |
|---|---|---|
| C1 — social long dwell + fair sweep + jitter | `hunter_defaults_give_social_channels_long_dwell_share` (node_config), `schedule::*` unit tests, simulator asserts | 6/149 ≥ 24% share each; social dwell ≥ 1000 ms and ≥ 1 NAN window + margin after jitter; every configured channel visited every supercycle; dwell order/length jittered deterministically. |
| C2 — no aircraft-following lock in default | `hunter_default_disables_aircraft_following_lock_on` (node_config), `schedule::sim_chatty_aircraft_cannot_monopolize_social_default` | Default `lock_on_duration_ms == 0`; a drone observation does not defer rotation; a chatty identity-only aircraft changes no other channel's visit. |
| C3 — presets + back-compat + docs | `schedule::preset_resolution_*`, `schedule::explicit_config_wins_over_preset`, tools/CHANNEL-SCHEDULER.md | `social` default; `park6` parks one social channel (no rotation churn); `sweep` reproduces legacy dwells; explicit values always win with a logged notice. |
| C4 — wire + heartbeat measurement | `wire_carries_rx_channel` (capture), `channel_vitals_report_dwell_and_hits` (heartbeat), `schedule::*` share tests | Additive optional `rx_channel` + `rx_channel_source` (`radiotap`, else `configured`) on every Wi-Fi observation (wire v4 unchanged); heartbeat hunter block carries per-channel `dwell_ms_total`, `rid_hits_total`, `dwell_share_pct`, and retune stats. |
| C5 — device boundary | `route_guard::*` tests, main.rs startup gate | Only the designated capture adapter is administered (channel setting only in this slice); the engine fails closed if that adapter carries the default route, before any radio administration. |
| S1 — simulator | `schedule::sim_*` tests | Table above, including the legacy reproduction of the 24-h gap distribution and the legacy lock-on monopoly demonstration. |
| S2 — retune dead time | `hunter` retune telemetry (heartbeat), ACCEPTANCE citation | Cited 8.5 ms proxy + 25 ms sensitivity in the model; per-switch `last/max/count` measured live so the fleet value replaces the proxy. |

Before implementation, this table and the compiling behavioral tests were
committed and their assertions recorded red against the pre-scheduler code.
After implementation, two mutations must each fail a test: re-enabling the
2000 ms default lock-on (fails `hunter_default_disables_aircraft_following_lock_on`)
and shrinking the social dwell to 400 ms (fails
`hunter_defaults_give_social_channels_long_dwell_share`). Engine version at
PR head: 0.8.27 (separate bump commit); no tag or release.

## Wi-Fi NAN reception acceptance

Stacked on the Beacon counter fix. Corpus references: BWF0010, BWF0020,
BWF0030, BWF0034, BWF0036, BWF0040, BWF0050, BWF0060, BWF0100.
RF timing/channel requirements BWF0032, BWF0070, BWF0080 and BWF0090 are not
established by synthetic decoder tests.

| Requirement | Executable evidence | Acceptance |
|---|---|---|
| N1 — Action admission | `nan_sdf_sets_wire_metadata`, `nan_rejects_other_actions` | Only a matching ODID NAN SDF produces telemetry. |
| N2 — bounded SDF walk | `nan_skips_unknown_attributes`, `nan_rejects_wrong_service`, `nan_rejects_bad_lengths`, `nan_rejects_sync_beacon`, `nan_truncation_and_byte_mutations_do_not_panic` | Match category/action/OUI/type/service ID; require publish + service info, check every declared length, skip unknown attributes. |
| N3 — shared strict decode | `nan_all_counters_and_pack_counts`, `nan_rejects_bad_pack` | All 256 counters × counts 1–9 decode; malformed Message Packs fail. JSON carries wifi_nan and exact counter. |
| N4 — identity and provenance | `nan_sdf_sets_wire_metadata`, `nan_rotating_mac_deduplicates_by_identity` | Addr2 remains the received source; optional BSSID is provenance only. NAN dedup ignores MAC, correlates usable pack identity, and never merges identity-less observations under UNKNOWN. |
| N5 — obsolete parser removal | repository reference check and migrated NAN/legacy-OUI negative tests | Remove both unused alternate entry points and their unverified legacy branch. |
| N6 — preserved transports | existing eight Beacon regressions, complete BLE suite and legacy differential oracle | Existing Beacon fixture bytes/output remain identical; no BLE, BPF, hunter, or radio-administration changes. |

Review follow-up: `standard_beacon_skips_bad_odid_ie` must decode a later valid
ODID IE after an earlier well-bounded IE contains malformed RID data. Commit
and record its behavioral failure first. An outer IE whose declared length
overruns the frame still stops walking because the next boundary is unknown.

Before implementation, commit this table and compiling behavioral tests, then
record admission/256-counter red assertions on the parent branch's code.
After implementation, prove service-ID and counter-peel mutations fail their
own assertions; exercise negative controls with paired valid inputs. Generate
independent C-oracle fixtures with pinned commit/license provenance. Run Rust
1.88.0 workspace, hosted contracts/CodeQL, and the actual ARM64 release helper.
Capability status may state implemented with synthetic/oracle evidence, never
field-proven. Engine version is 0.8.26; no tag or release is created.

## Parent Beacon acceptance

This change is complete only when the following requirements are demonstrated
by executable tests on Rust 1.88.0.

| Requirement | Test evidence | Acceptance |
|---|---|---|
| R1 — standard Beacon/Probe Response deframing removes the counter and strictly validates Message Pack bytes | `router::tests::standard_beacon_rejects_malformed_framing` and `router::tests::standard_beacon_deframer_is_panic_free_for_arbitrary_bytes` | Only IE 221 / `FA:0B:BC` / `0x0D` is accepted; truncated input, size other than 25, count 0, count above 9, and inconsistent length are rejected without panic. |
| R2 — all counter values decode | `router::tests::standard_beacon_accepts_every_message_counter_value` | The rejected-counter list is empty for `0x00..=0xFF`. |
| R3 — transport and counter reach the wire | `router::tests::standard_beacon_sets_transport_and_counter_on_wire` | The decoded model and serialized JSON contain `wifi_beacon` and the exact counter. Wire format version remains unchanged because both fields already exist as additive optional wire-v4 fields. |
| R4 — non-standard behavior is explicit | `router::tests::counterless_vendor_payload_is_rejected` and real DJI regression tests | A payload that omits the required counter is rejected. No Wi-Fi dialect fallback is retained without captured evidence. Real captured DJI fields remain identical except for the newly populated transport/counter metadata. |
| R5 — BLE is unchanged | Existing `ble::tests`, including `verify_bb40030_*`, `verify_bb50030_*`, `verify_bb50110_*`, `verify_bb50120_*`, and `verify_bur0050_*` | The existing BLE suite remains green with no BLE production-code changes. |

Additional gates:

- the existing real DJI fixture and an independently captured Beacon frame are
  decoded byte-exactly through the standard counter boundary;
- deliberately restoring the old counter boundary makes the 256-value sweep
  fail at its own assertion;
- the full workspace test suite, hosted CI, and the local ARM64 release dry-run
  pass;
- engine version is `0.8.25`; no tag or release is created.

## Channel-scheduler hygiene follow-up (2026-10-06)

Post-merge verification of the scheduler work passed with no MUST findings and
three SHOULDs; this follow-up closes the two that land in this repository.
The field-evidence correction above (measured pooled inter-observation gaps
from the 24-h capture: p10 ≈ 4.2 s, median ≈ 4.5 s, p75 9.0 s, with the 2×
cycle harmonic near 8.4 s — replacing the earlier "median 5–8 s" wording)
is recorded first: an acceptance document must not misquote the field
evidence it cites. The acceptance bands themselves are unchanged and still
hold.

Acceptance for the channel-set hygiene work, registered before
implementation. Tests below fail on the current scheduler at their
assertions:

| Requirement | Test | Accepted behavior |
|---|---|---|
| H1 — duplicates never double a channel's share | `channel_set_duplicates_are_deduped` | `channel_set: [6, 11, 11, 149]` resolves to each channel exactly once (first-occurrence order); the startup log names the dropped duplicates. Dedupe with a notice, not refusal: the duplicated config functions today, and boot-failing it would break a working setup over an operator typo. |
| H2 — DFS channels are refused at plan level | `dfs_channels_are_refused_at_plan_level` | UNII-2 (52–144) never appears in a resolved plan; startup fails closed with a per-channel reason before any radio administration (replacing per-cycle runtime refusals). |
| H3 — invalid channels are refused at plan level | `invalid_channels_are_refused_at_plan_level` | Channel numbers outside the supported regulatory range never appear in a resolved plan; same fail-closed refusal. |
| H4 — defaults unchanged | existing preset/back-compat suite | The default 12-channel set produces no dedupe notice and no refusals; every existing scheduler test stays green. |

Mutation gates (after green): removing the dedupe step fails H1; removing the
DFS branch fails H2; removing the regulatory-range branch fails H3. No engine
version bump in this change — it batches into the next release.
