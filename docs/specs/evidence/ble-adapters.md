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
are `not_yet_observed` until a structurally valid Coded-PHY report arrives during
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
