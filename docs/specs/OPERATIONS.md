<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Operations and data handling

<!-- BEGIN GENERATED: metadata -->
Applies to: **0.9-beta** · Product: **0.9.0** · Specification revision: **2026-10-28**.

**Next/unreleased draft; release tag pending.** Source baseline: [6eed7b81](https://github.com/cybrrd/brrdfeeder/tree/6eed7b81a15415a4f9f9564cdb85341b886520cb). Future-v5 rows are separate design targets, not beta features.
<!-- END GENERATED: metadata -->

[Back to the information sheet](README.md). Read the scope and status beside
each expectation. Policy targets do not establish that every end-to-end path
has passed acceptance.

## Data and display

<!-- BEGIN GENERATED: operations -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **D01 — Serial display** | Complete reported serial numbers are displayed | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#display) Serials are not masked. A broadcast identifier is a reported claim, not authenticated physical identity. |
| **D02 — Operator/station position** | Shown on globe.cybrrd.com as broadcast in Remote ID; telemetry kept in full | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#display) Position is reported by the drone, not independently verified. Serials and operator IDs are not hidden. Display acceptance remains pending; this is not a new claim of complete beta fidelity. |
| **D03 — Telemetry generation** | Current v4 is not a full-fidelity v5 record; v5 is design-only | future-v5 / **unverified** (source) | [source 1](evidence/design-basis.md#generations) Do not invent missing fields in older records or treat a future design as deployed behavior. |
| **D04 — Heartbeat retention** | Heartbeats excluded from permanent lake storage by design | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#retention) This does not specify a beta retention duration for observations or settle operational/distress retention. |
| **D05 — Delayed observations** | Archive with capture and delivery times; never replay as live Globe positions | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#backlog) A recent delivery does not make an old capture fresh. |
<!-- END GENERATED: operations -->

Globe shows the operator/station position that the drone broadcasts in Remote ID.
Reported serial numbers are shown in full, and operator IDs are not hidden.
Telemetry, including operator/station position, is kept in full in backend
storage, browser network payloads and state. These are broadcast reports, not
independently verified identities or locations.

Complete retention is the design policy; it must not be confused with complete
capture or a measured current archive guarantee. Radio hardware may not expose
fields, older records may lack fields, and outages can cause explicit gaps.
The [known limits](KNOWN-LIMITS.md) record those boundaries. Delayed records keep
their original capture times; their later arrival does not make them live.

## Registration and node capacity

<!-- BEGIN GENERATED: enrollment -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **P04 — Sign-up and quota posture** | 1 node per new registrant by default | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#enrollment) More nodes require an audited exception during beta. No manual approval of registrants. Capacity earned through reputation is post-beta work. New nodes' own drones are never hidden because the node is new or provisional. Deployment acceptance remains pending. |
<!-- END GENERATED: enrollment -->

Registration does not require manual approval. The beta node allowance is a
separate owner policy: additional nodes require an audited exception. A capacity
refusal must explain the reason and support route. Reputation-earned capacity
comes after beta. A new node's own drones remain visible; being new or
provisional must never hide its detections.

## Outages and power loss

<!-- BEGIN GENERATED: outage -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **D07 — Outage buffer default** | 2 GiB | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#outage) Beta on-disk spool design, not measured endurance. Duration depends on encoded record sizes and traffic. |
| **D08 — Full-buffer behavior** | Keep stored records; stop adding; persist the exact gap; mark coverage impaired; raise Red | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#outage) Never discard the only stored copy to make room. Physical storage exhaustion is not an advisory user-admission threshold. |
| **D09 — Power-loss vulnerable window** | Pending (duration) | 0.9-beta / **unverified** (source) | [source 1](evidence/design-basis.md#power) Not yet measured. No unconditional zero-loss claim, including before the durable marker. |
<!-- END GENERATED: outage -->

At full storage, keep what is already stored and stop accepting additional
records into that buffer. Persist the exact gap, mark coverage impaired, and
raise Red distress. Do not shed the oldest stored records to make the display
look healthy. Acceptance must also cover a full distress queue, so a missing
alarm cannot conceal missing observations. Alert delivery itself may be delayed
by the outage or prevented by a stopped engine.

The physical buffer is finite. Advisory capacity alerts should give operators
time to add capacity; they do not remove that storage failure condition.
The vulnerable interval before a durable write remains unmeasured. Power-loss
protection is strongly recommended, but no unconditional zero-loss promise is
made. Record storage model, power arrangement and failure timing in acceptance.

## Installation, updates, recovery and removal

<!-- BEGIN GENERATED: updates -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **U01 — Engine and console updates** | Signed metadata, exact image digests and configured dev/staging/general ring | 0.9-beta / **unverified** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/release-system/README.md) Existing source/fixture evidence is linked; complete beta deployment coverage is not established. |
| **U02 — Existing-node helper update** | 1 explicit command per existing node | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#helper) [source 2](https://github.com/cybrrd/brrdfeeder/blob/0b6a132180d03812f0bf1e80c23c4f65206c311a/Component/brrdfeeder/install/README.md#explicit-helper-only-upgrade-090) Signed helper feed is not published for the beta. Use the documented helper-only command after the approved installer/helper chain is published. |
| **U03 — Local update recovery** | Retained images and journal-based health/rollback checks | 0.9-beta / **unverified** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/release-system/README.md) Local health is not proof of downstream ingestion. Broken storage or a missing retained image can prevent recovery. |
| **U04 — Removal** | Uninstall removes local package data/configuration/images; server deregistration is separate | 0.9-beta / **unverified** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/brrdfeeder/install/README.md) Follow the installer removal guide and its prompts. Do not uninstall to bypass an update or enrollment refusal. |
<!-- END GENERATED: updates -->

Start at [the installer](https://get.cybrrd.com) and read its script and
[instructions](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/brrdfeeder/install/README.md). Use the configured update ring and inspect local
console/update status. Image identity, signature verification, local health and
downstream delivery are separate checks.

For an existing node, after the approved beta bootstrap/installer/helper chain
is published, the candidate helper-only path is:

```sh
curl -fsSL https://get.cybrrd.com | bash -s -- --upgrade-helper
```

The [candidate instructions](https://github.com/cybrrd/brrdfeeder/blob/0b6a132180d03812f0bf1e80c23c4f65206c311a/Component/brrdfeeder/install/README.md#explicit-helper-only-upgrade-090)
require intact existing state. An ordinary installer rerun does not perform
this upgrade. The signed helper feed is not yet published; helper updates in
the beta are an explicit action on each existing node. Native interruption and
recovery acceptance is still required before publication of that chain.

If an update fails, retain the result and inspect the console and update
receipts. Follow the [release recovery guide](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/release-system/README.md) rather than deleting
state to force another attempt. Local rollback depends on intact retained images
and storage. Follow the installer removal instructions if you choose to
uninstall; local removal does not deregister a server-side node.

## Support and recourse

The installer can produce a redacted support bundle with `--support-bundle`.
Nothing is uploaded automatically. Review it before sharing; include the release,
run ID and symptom, and remove credentials, raw identifiers and location data.
Do not send an unfiltered terminal recording.

For install help and bugs, use [GitHub Issues](https://github.com/cybrrd/brrdfeeder/issues),
enabled at launch. Use [admin@cybrrd.com](mailto:admin@cybrrd.com) as the private
fallback. No support response deadline or support period is promised. For
vulnerabilities, use [private security reporting](../../SECURITY.md), whose
no-deadline policy applies.

<!-- BEGIN GENERATED: support -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **P05 — Ordinary beta support path** | GitHub Issues for install help and bugs; admin@cybrrd.com as the private fallback | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#support) Issues enabled at launch. No response deadline or support period is promised; vulnerability reports remain private under SECURITY.md. |
<!-- END GENERATED: support -->
