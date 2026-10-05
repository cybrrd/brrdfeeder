<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Wi-Fi Beacon message-counter acceptance

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
