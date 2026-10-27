<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Known limits

<!-- BEGIN GENERATED: metadata -->
Applies to: **0.9-beta** · Product: **0.9.0** · Specification revision: **2026-10-27**.

**Next/unreleased draft; release tag pending.** Source baseline: [6eed7b81](https://github.com/cybrrd/brrdfeeder/tree/6eed7b81a15415a4f9f9564cdb85341b886520cb). Future-v5 rows are separate design targets, not beta features.
<!-- END GENERATED: metadata -->

[Back to the information sheet](README.md). This list distinguishes absent
acceptance evidence from design intent. It does not claim that every listed
mechanism is broken.

- NAN decoding has synthetic and independent-oracle evidence; real transmitter
  reception and RF timing/channel coverage remain unverified.
- Bluetooth HCI exposes advertising reports, not every over-air packet field.
  Controlled passive-air, quiet/loss/recovery and shutdown tests remain open.
  Optional full-packet sniffers are future work.
- A scheduled capture radio cannot listen continuously on every channel.
  Simulator results are not measured RF coverage; DFS capture is unsupported.
- The current wire format is not the complete future record format. Historical
  records are not upgraded by inventing missing data. Display filtering does
  not redact backend telemetry.
- The beta outage-buffer behavior is a target pending native acceptance.
  Storage wear, actual duration, the power-loss vulnerable window and behavior
  with a full distress queue need measurements on the release hardware.
- Helper feed publication is outstanding. The explicit existing-node upgrade
  needs its approved published installer chain and native acceptance. Fixture
  or cross-build success does not prove interruption/reboot recovery.
- No beta end-to-end archive retention duration or permanent-copy custody
  guarantee is established here. Heartbeat retention is distinct from telemetry.
- Broadcast serials and positions are reported claims, not authenticated proof
  of aircraft identity or independently verified location.
- No uptime, reception range, aviation-safety use, ASTM/FCC/CE certification,
  full standards conformance, vulnerability-free state, support period or
  security-remediation deadline is claimed.

See [radio evidence](REQUIREMENTS.md#what-the-radios-can-hear),
[operations](OPERATIONS.md), [service measurements](SERVICE-LEVELS.md) and the
[existing acceptance catalogue](../../ACCEPTANCE.md). Existing source claims
remain scoped to their evidence. The registry does not close open tests.

## Release placeholders and owners

<!-- BEGIN GENERATED: placeholders -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **P03 — Stream capacity alerts** | Pending (advisory thresholds) | 0.9-beta / **unverified** (source) | [source 1](evidence/design-basis.md#pending) Pending signature. Values remain unpublished until signed approval; implementation and acceptance evidence are also pending. |

| Claim | Owner / acceptance still needed |
|---|---|
| P03 | B05 stream capacity: B05: obtain release-owner signature before publishing values; measure saturation, custody, refusal visibility and recovery. |
<!-- END GENERATED: placeholders -->

Viewer thresholds and guards, enrollment policy and the support path are decided;
see [service expectations](SERVICE-LEVELS.md#capacity-and-future-sizing) and
[operations](OPERATIONS.md#registration-and-node-capacity). Their public acceptance
receipts remain pending. Stream capacity is pending signature; no values are
published here. The release tag and the power-loss vulnerable window (D09, owned
by B06 outage acceptance) also remain pending. Recheck actual release source,
deployment and evidence before tagging.
