<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Bluetooth adapter qualification — 0.9.1

The runtime admission set is exactly **0bda:876e, 0bda:8771, 0bda:a728**.
Every other USB identity is refused, including when selected by address.
Admission permits reception; it is not proof of long-range reception.

| USB identity | Admission | Reception evidence boundary |
| --- | --- | --- |
| 0bda:876e | Allowed | Qualify the selected adapter, kernel and firmware by observed reports. |
| 0bda:8771 | Allowed; native proof pending | Obtain source-attributed native reception evidence by observation at the remote site. |
| 0bda:a728 | Allowed | Reported reception evidence supersedes the prior refusal; repeat on the candidate build. |
| Every other ID | Refused | No wildcard/family/feature-bit admission. |

Status `inventory.rid_ble.long_range` and operational `message.bluetooth.long_range`
are `not_yet_observed` until a structurally and semantically valid Coded-PHY report arrives during
the current controller lifetime. An extended report on 1M does not prove long range.
`observed` reports that evidence, not a chipset claim or a guarantee of RID delivery.
Controller replacement/loss resets it. A controller refusing extended scan setup
continues passive legacy scanning and reports `unavailable`; later observed Coded-PHY evidence
can supersede it. Feature bits only select setup commands. Scan timings are unchanged.

The operational `message.bluetooth.rejects` map has the seven decoder rejection
reasons (`truncated_ad`, `no_signature`, `bad_payload`, `pack_count`, `core_decode`,
`no_location`, `unsupported_transport`). `unassociated_dropped` counts Location
arrivals without a fresh same-address cached identity. All counters are cumulative
since process start, survive adapter restarts, and are independent of drained audit
intervals. `no_signature` includes ordinary non-RID advertising, not just bad RID.

Explicit OFF, adapter pins, exclusive ownership, conflict handling and ownership
receipts remain in force. Bench qualification must record source-attributed frames;
aggregate counters, feature probes and Healthy state alone are not RID proof.

## Reception evidence and rejection boundaries

Any valid Coded report proves controller reception, including non-RID advertising,
anonymous advertising, unavailable RSSI, and incomplete advertising data. Those
reports need not qualify for RID decoding. Invalid metadata contributes no Coded
evidence and cannot enter RID decoding; other valid reports in the event survive.
The all-report health count remains independent of this capability evidence.

Metadata validation follows [Bluetooth Core v5.4, Vol 4, Part E,
§7.7.65.13 and §7.8.53](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface/host-controller-interface-functional-specification.html),
and [Vol 6, Part B, §2.3.1.5–6 and §4.4.3.5](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/low-energy-controller/link-layer-specification.html).
Address type `0xff` means anonymous; it is not intrinsically malformed. Resolved
identity address types also qualify as evidence without widening RID admission.
PHY value 4 requires host coding-selection negotiation, which this scanner does
not enable. Enabling that feature would require extending this validation.

`no_location` means neither Location nor usable aircraft identity, despite its
historical name. BT4 final no-observation rejection and BT5 core NoLocation both
increment it once. A usable Basic ID alone remains accepted. A Location without
usable association still increments only `unassociated_dropped`.

Production-decoder fixtures exercise six of seven reject categories through
`BleDecoder::ingest` → `ingest_at` and assert exact deltas in queued operational
records. `core_decode` is currently unreachable: BT4 checks size before its core
call, and BT5 checks the pack's structural preconditions before its core call;
the remaining core error is mapped to `no_location`. The key stays for wire
compatibility and future core errors. `pack_count` is BT5-only.
`unsupported_transport` is reachable at the decoder API with inconsistent PHY /
extended metadata; the HCI filter does not emit those combinations, so this is a
defensive API control, not a claimed radio-origin rejection. The other five
categories are reachable from eligible HCI reports.
