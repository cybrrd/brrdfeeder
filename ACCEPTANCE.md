# Wi-Fi Beacon message-counter acceptance

This change is complete only when the following requirements are demonstrated
by executable tests on Rust 1.88.0.

| Requirement | Test evidence | Acceptance |
|---|---|---|
| R1 — standard Beacon/Probe Response deframing removes the counter and strictly validates the pack | `router::tests::standard_beacon_rejects_malformed_framing` and `router::tests::standard_beacon_deframer_is_panic_free_for_arbitrary_bytes` | Only IE 221 / `FA:0B:BC` / `0x0D` is accepted; truncated input, size other than 25, count 0, count above 9, and inconsistent length are rejected without panic. |
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
