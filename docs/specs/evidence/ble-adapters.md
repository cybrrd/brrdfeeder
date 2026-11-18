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
call, and BT5 checks the structure of packed messages before its core call;
the remaining core error is mapped to `no_location`. The key stays for wire
compatibility and future core errors. `pack_count` is BT5-only.
`unsupported_transport` is reachable at the decoder API with inconsistent PHY /
extended metadata; the HCI filter does not emit those combinations, so this is a
defensive API control, not a claimed radio-origin rejection. The other five
categories are reachable from eligible HCI reports.

## HCI field-combination matrix

This matrix governs `valid_extended_metadata`; it does not widen RID admission.
`H` denotes [Core v5.4, Vol 4 Part E](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface/host-controller-interface-functional-specification.html);
`L` denotes [Core v5.4, Vol 6 Part B](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/low-energy-controller/link-layer-specification.html).
Values below are hexadecimal except lengths and statuses. Statuses are 0
(complete), 1 (continuing), 2 (truncated); 3 and event bits 7–15 are rejected.

| Field | Domain / condition | Source |
| --- | --- | --- |
| Address_Type | 00–03 or FF (anonymous); row constraints below | H §7.7.65.13, Address_Type |
| Primary_PHY / Secondary_PHY | 01/03; 00–03 respectively; legacy primary 01 | H §7.7.65.13, PHY fields and legacy paragraph |
| Advertising_SID | 00–0F or FF (absent); row constraints below | H §7.7.65.13, Advertising_SID |
| Periodic_Advertising_Interval | 0 or 0006–FFFF; periodic rows require non-anonymous addressing | H §7.7.65.13; L §4.4.2.12 |
| Direct_Address_Type | Directed: 00–03/FE; undirected: ignored | H §7.7.65.13, directed-address fields |
| TX_Power / RSSI | −127…20 dBm or unavailable (7F) | H §7.7.65.13, power fields |
| Event properties / data status | Only combinations below | H §7.7.65.13, Event_Type / Table 7.1 |

PHY 04 is rejected because this scanner does not enable host coding selection.
Properties below are event bits 0–4; statuses are checked separately. “Periodic”
means a nonzero interval is permitted, and “anonymous” means Address_Type FF is
permitted. The implementation expands these grouped rows into 16 exact rows.

| Report class | Properties | Secondary | SID | Anonymous | Periodic | Statuses | Data length | Sources |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Legacy, ordinary/response | 10,12,13,1A,1B | 00 | absent | no | no | 0 | ≤31 | H §7.7.65.13 Table 7.1, §7.7.65.2; L §2.3.1.1–4, §2.3.2.2 |
| Legacy directed | 15 | 00 | absent | no | no | 0 | 0 | H Table 7.1; L §2.3.1.2 |
| Extended primary-only | 00,04 | 00 | absent | no | no | 0 | 0 | L §2.3.1.5 Table 2.4; §4.4.3.5 |
| Auxiliary nonconnectable/nonscannable | 00,04 | 01–03 | present | yes | yes | 0,1,2 | ≤229 | L §2.3.1.6 Table 2.5; §4.4.2.12; H §7.8.61 |
| Auxiliary connectable | 01,05 | 01–03 | present | no | no | 0,1 | ≤229 | L Table 2.5; H §7.7.65.13 |
| Auxiliary scannable advertisement | 02,06 | 01–03 | present | no | no | 0 | 0 | L Table 2.5; H §7.7.65.13 |
| Auxiliary scan response | 0A,0E | 01–03 | optional | no | no | 0,1,2 | ≤229 | L §2.3.2.3 Table 2.10; H §7.7.65.13 |

The primary-only prohibitions are retained: Table 2.4 requires AdvA and excludes
ADI, SyncInfo and AdvData in both no-auxiliary rows. No data exists to fragment.
Connectable auxiliary reports may be split by HCI, but cannot report AUX_CHAIN
loss because Table 2.5 excludes AuxPtr. Scannable advertisements have no AdvData
to split. Scan responses may omit ADI; HCI recommends using the original SID,
which is not treated as a mandatory field-presence guarantee here.

**Directed announcements of periodic advertising are valid.** Table 2.5 permits
SyncInfo in its directed nonconnectable/nonscannable row. H §7.8.61 excludes
connectable, scannable, legacy and anonymous sets, not directed sets. This event
reports the announcement, not an HCI LE Periodic Advertising Report (§7.7.65.15).
A missing-SID periodic announcement is invalid under L §4.4.2.12 and Tables 2.4–5.

The matrix tests enumerate every meaningful Event_Type bit pattern and every SID
byte for periodic and nonperiodic auxiliary reports, plus reserved event bits,
scalar domains, primary-only presence and legacy combinations. They independently
check metadata validity, parser evidence, mixed-event survival and framing errors.
