<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Known limits

<!-- BEGIN GENERATED: metadata -->
Applies to: **0.9-beta** · Product: **0.9.0** · Specification revision: **2026-10-30**.

**0.9.0 beta release specifications; release tag pending.** Source baseline: [d6130a20](https://github.com/cybrrd/brrdfeeder/tree/d6130a208f92a275db3fe11a52a4f21dcdbeaf63). Future-v5 rows are separate design targets, not beta features.
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
  records are not upgraded by inventing missing data. Telemetry is kept in full
  under the data policy.
- The beta outage-buffer behavior is a target pending native acceptance.
  Storage wear, actual duration, the power-loss vulnerable window and behavior
  with a full distress queue need measurements on the release hardware.
- Helper feed publication is outstanding. The explicit existing-node upgrade
  needs its approved published installer chain and native acceptance. Fixture
  or cross-build success does not prove interruption/reboot recovery.
- In 0.9.0, reinstalling with the same account and hostname keeps the same node
  ID and issues a new credential, but does not revoke the previous credential.
  To revoke the previous credential, contact support through
  [GitHub Issues](https://github.com/cybrrd/brrdfeeder/issues) (Install help) or
  [admin@cybrrd.com](mailto:admin@cybrrd.com);
  [local uninstall does not deregister the node](../../Component/brrdfeeder/install/README.md#status-support-and-uninstall).
  Automatic revocation is planned for 0.9.1.
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

## Unmeasured beta behavior

These limits ship with the 0.9.0 specifications. Each remains **unverified**:
the reviewed evidence lacks the stated instrument or measurement. They do not
establish a safe load, successful restore, delivered alert, or zero-loss envelope.
The [acceptance-gap review](evidence/acceptance-gaps.md) explains what the existing
checks do and do not establish. Proposed verification scenarios are not test results.

<!-- BEGIN GENERATED: beta-limits -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **Archived stream restore** | Not yet measured; no reviewed end-to-end stream-restore instrument | 0.9-beta / **unverified** (source) | [source 1](evidence/acceptance-gaps.md#archived-stream-restore) Archive checksums prove integrity only; they do not establish successful restore or end-to-end custody. A different data store restore is not a substitute. |
| **Live disk alert delivery** | Not yet measured; no reviewed live threshold-to-person instrument | 0.9-beta / **unverified** (source) | [source 1](evidence/acceptance-gaps.md#live-disk-alert-delivery) Offline rule fixtures do not prove live metric ingestion, destination delivery or human receipt. No alert delivery deadline is guaranteed. |
| **Public edge request bounds** | Request size, rate and timeout behavior not yet measured; no reviewed boundary instrument | 0.9-beta / **unverified** (source) | [source 1](evidence/acceptance-gaps.md#public-edge-request-bounds) Route/authentication probes and per-browser WebSocket guards do not establish public-edge bounds. No numeric limit or tested operating envelope is claimed. |
| **Physical power loss** | Release-hardware power-cut behavior and vulnerable write interval not yet measured | 0.9-beta / **unverified** (source) | [source 1](evidence/acceptance-gaps.md#physical-power-loss) Physical power cuts and native-radio memory qualification are outside the bounded acceptance plan. Restarts, simulated outages and storage-full tests do not establish a zero-loss envelope. |
| **Overflow close reasons and counters** | Unverified; direct overflow close-reason plus matching-counter assertions are missing from reviewed tests | 0.9-beta / **unverified** (source) | [source 1](evidence/acceptance-gaps.md#overflow-close-reasons-and-counters) Queue bounds and disconnects do not prove the send_buffer_full or airspace_send_overflow reason/counter pair. The frame_too_large assertion covers a different guard; live alert delivery remains unmeasured. |
| **Viewer load beyond tested levels** | Not yet measured; the full 4,097-client load run is not executed in the bounded acceptance plan | 0.9-beta / **unverified** (source) | [source 1](evidence/acceptance-gaps.md#viewer-load-beyond-tested-levels) Rule-firing fixtures and admission tests do not establish capacity at that load. Proposed harness runs are not measured results. Advisory thresholds are not viewer caps or a supported-capacity guarantee. |
| **Live alert human receipt** | Not yet measured; no reviewed executable live destination-canary instrument | 0.9-beta / **unverified** (source) | [source 1](evidence/acceptance-gaps.md#live-alert-human-receipt) Offline routing and bridge checks do not establish human receipt or acknowledgement. This includes disk and viewer-guard alerts; no notification or response deadline is guaranteed. |
<!-- END GENERATED: beta-limits -->

## Release acceptance and open values

<!-- BEGIN GENERATED: placeholders -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **P03 — Stream capacity alerts** | Stream bytes: warning at 50%, Red at 80%; per-node message count: Red at 80%; each sustained for 5 minutes | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#stream-capacity) These are advisory monitoring thresholds, not collection or viewer-admission caps. They do not prevent broker storage exhaustion or prove loss-free retention. Alert activation and public acceptance receipts remain pending. |

| Claim | Owner / acceptance still needed |
|---|---|
| P03 | Stream capacity: Verify signed capacity configuration, a full post-change observation hour without growing gaps, archive custody and routed alerts at each sustained threshold; measure saturation, visible refusal and recovery. |
<!-- END GENERATED: placeholders -->

Viewer thresholds and guards, enrollment policy and the support path are decided;
see [service expectations](SERVICE-LEVELS.md#capacity-and-future-sizing) and
[operations](OPERATIONS.md#registration-and-node-capacity). Their public acceptance
receipts remain pending. The approved stream storage capacities and advisory
alert thresholds are recorded in the service expectations. The release tag and
the power-loss vulnerable window (D09, owned by outage acceptance) remain open.
No hardware-independent interval, buffer duration, media lifetime or end-to-end
archive-retention period has been established. Recheck actual release source,
deployment and evidence before tagging.
