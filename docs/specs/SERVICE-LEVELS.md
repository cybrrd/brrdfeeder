<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Beta service objectives

<!-- BEGIN GENERATED: metadata -->
Applies to: **0.9-beta** · Product: **0.9.0** · Specification revision: **2026-10-26**.

**Next/unreleased draft; release tag pending.** Source baseline: [6eed7b81](https://github.com/cybrrd/brrdfeeder/tree/6eed7b81a15415a4f9f9564cdb85341b886520cb). Future-v5 rows are separate design targets, not beta features.
<!-- END GENERATED: metadata -->

[Back to the information sheet](README.md). These objectives describe desired
behavior, not a contractual SLA. There is no uptime guarantee, service credit,
guaranteed compensation, aviation-safety warranty or certification claim.
Actual results depend on the declared hardware, RF environment, storage, link
and platform conditions.

## Targets and scope

The first-contact objectives are included for beta acceptance. The remaining
service-design rows are explicitly future-v5; they are not claims about beta
timers or deployed server behavior. All remain targets until scoped acceptance
receipts exist. A link type in a test scenario is not a supported-modem list.

<!-- BEGIN GENERATED: service -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **S01 — First contact on wired/Wi-Fi/LTE** | ≤ 2 seconds (target) | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#service) Send as soon as possible on every link. This is a beta acceptance objective adopted from the future service design; measurement is outstanding. |
| **S02 — First contact on narrow links** | ≤ 15 seconds (target) | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#service) Best effort as soon as possible. This target does not declare any particular narrow-link radio supported. |
| **S03 — Heartbeat interval** | 60 seconds | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#service) Future service design, not a measured beta timer. |
| **S04 — Node offline classification** | 4 minutes (approximately) | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#service) Offline classification is separate from aircraft freshness. |
| **S05 — Fixed-node offline distress** | > 15 minutes | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#service) Fixed-node outage raises Red; mobile offline alone is an operational (Silver) event. |
| **S06 — Red node-to-ingest on wired/Wi-Fi/LTE** | ≤ 10 seconds (target) | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#service) Human notification after ingest is a separate, uncommitted service. |
| **S07 — Red node-to-ingest on narrow links** | 60 seconds best-effort target | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#service) Urgent traffic precedes routine bulk; a disconnected link cannot deliver an alert immediately. |
| **S08 — Server-side revocation enforcement** | ≤ 10 seconds (target) | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#service) No local shut-off timer: a disconnected node keeps capturing. This is a security control, not a capacity threshold. |
| **S09 — Future backlog drain** | 24 hours for the declared backlog workload while capture continues | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#future-outage) Effective uplink must cover live capture plus drain and measured overhead. |
<!-- END GENERATED: service -->

## Measurement, failure and visibility

For each scenario record the exact build, hardware/profile, link conditions,
clock uncertainty, observation rate and encoded sizes. Report the sample count,
losses, retries and worst observed latency; missing deliveries are failures,
not samples to discard. Separate synthetic software checks from physical radio
and native storage results. Human notification after ingest is measured
separately from node-to-ingest delivery.

<!-- BEGIN GENERATED: service-methods -->
| Claim | Measurement scenario | On a miss | Failure signal |
|---|---|---|---|
| S01 | B03 first-contact matrix: receiver capture to first network send; separately record ingest acceptance and live display latency, clock error, missing sends and maximum by link/profile. | Publish the failed profile and retain delayed records for archive; do not thin collection. | Capture-to-send timing and missing-send results in the acceptance report; end-to-end live latency display is unverified. |
| S02 | B03 first-contact matrix: same capture-to-send measurement on each declared narrow-link scenario, including outage and retry; report ingest/display latency separately. | Classify a link that misses the bound as unsuitable for live first contact; delayed records still go to archive. | Late or absent live first contact; acceptance report identifies the unsuitable link. |
| S03 | Future liveness acceptance: timestamp heartbeat receipts and correlate event traffic and clock state. | Expose missed heartbeat evidence; do not infer missing aircraft from node liveness alone. | Liveness evidence and operational status; integrated beta signal unverified. |
| S04 | Future liveness acceptance: suppress heartbeats with controlled clocks and measure server classification. | Record late or incorrect classification and investigate heartbeat/event/clock handling. | Node offline status; integrated beta signal unverified. |
| S05 | Future liveness acceptance: distinguish fixed/mobile node profiles across prolonged disconnect and reconnect. | Record missed or incorrect distress; preserve the outage interval. | Red distress for fixed nodes; mobile offline operational status. Delivery itself needs a working path. |
| S06 | Future Red acceptance: event creation to durable ingest receipt under concurrent bulk traffic, loss and retry. | Retry until stored and report delay; preserve the local event. | Event and receipt timestamps; human notification timing is not guaranteed. |
| S07 | Future Red acceptance: same durable-receipt measurement on each declared narrow-link profile. | Keep retrying and report the delayed event when a path returns. | Queued/delayed distress and receipt evidence; local visibility depends on a running node. |
| S08 | Future revocation acceptance: policy effective time to broker/ingest refusal, including disconnected and reconnecting nodes. | Record enforcement failure and quarantine/rejection evidence for investigation. | Server enforcement audit; end-user revocation status integration unverified. |
| S09 | Future drain acceptance: fill the declared workload, restore provisioned uplink, measure clearance time and new-capture survival. | Show remaining backlog and time-to-clear; provision more capacity without dropping fields. | Backlog bytes, oldest age and time-to-clear in operational reporting (target). |
<!-- END GENERATED: service-methods -->

A service miss must lead to a known-limit report, recovery, rollback or a revised
future target. It must not silently remove collected fields or replay old
observations as live. Mark unobservable results as such. A dead or disconnected
node may be unable to send distress immediately; preserve available local
receipts for later reconciliation.

## Capacity and future sizing

Capacity values are advisory alert thresholds. Crossing them must not refuse
users, impose a fixed aircraft count, or silently thin collection. Monitoring
and operational capacity management handle load. Security enforcement, malformed
input checks and actual storage exhaustion are separate conditions.

<!-- BEGIN GENERATED: future -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **D10 — Future outage-buffer sizing** | 32 GB (approximate) | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#future-outage) Future design envelope. It does not replace the beta buffer or promise a fixed duration. |
| **D11 — Future backlog workload** | 24 hours at 25 observations/s | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#future-outage) Workload is observations, not a maximum aircraft count. Multiple transports and repeated reports contribute. |
| **D12 — Future storage reserve** | 10 % | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#future-outage) Planning headroom; adjust the buffer when measurements require, without thinning records. |
| **D13 — Future occupancy alert levels** | 50, 80 % | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#capacity) Advisory alerts, never count-based refusal or telemetry thinning. |
| **D14 — Future impaired-coverage warning** | 95 % | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#capacity) Advisory warning. Actual storage exhaustion follows the explicit keep-stored and exact-gap policy. |
| **S10 — Illustrative future drain payload rate** | 5.15 Mbit/s (approximately, before additional measured overhead) | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#future-outage) Planning estimate for the declared workload, not a carrier specification or guaranteed bandwidth. |
<!-- END GENERATED: future -->

The future buffer, workload and drain figures belong together and are estimates
pending full-record measurements. They are not the beta buffer, a bandwidth
purchase recommendation, or a promise of a particular outage duration.

## Decisions still pending

<!-- BEGIN GENERATED: placeholders -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **P01 — Viewer advisory threshold** | 100 concurrent viewers | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#capacity) Chosen alert level; deployed behavior and public acceptance receipt remain pending. |
| **P02 — Per-IP viewer advisory threshold** | 32 concurrent viewers per IP | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#capacity) Shared networks must not hit a fixed count-based wall. Threshold crossing raises an alert. |
| **P03 — Stream capacity alerts** | Pending (advisory thresholds) | 0.9-beta / **unverified** (source) | [source 1](evidence/design-basis.md#pending) Pending public thresholds and implementation evidence; no internal broker limits published as user allowances. |
| **P04 — Sign-up and quota posture** | Pending (advisory thresholds and enrollment policy) | 0.9-beta / **unverified** (source) | [source 1](evidence/design-basis.md#pending) Pending owner decision and acceptance. Authentication and security checks still apply. |
| **P05 — Ordinary beta support path** | Pending (support channel) | 0.9-beta / **unverified** (source) | [source 1](evidence/design-basis.md#pending) Public Issues and Discussions are currently closed. Beta choice is pending; no response deadline. |

| Claim | Owner / acceptance still needed |
|---|---|
| P01 | B07 viewer capacity: B07: cross global threshold and verify alerts/metrics without count-based refusal. |
| P02 | B07 viewer capacity: B07: cross per-IP threshold and verify alerts/metrics without count-based refusal. |
| P03 | B05 stream capacity: B05: publish approved alert thresholds and measure saturation, custody, refusal visibility and recovery. |
| P04 | B04 enrollment: B04: publish approved enrollment policy and prove capacity alerts do not impose arbitrary count-based refusals. |
| P05 | Release owner (support policy): Release owner: decide whether public Issues open; verify published support route and redacted-bundle handling. |
<!-- END GENERATED: placeholders -->

Threshold selection is not proof of deployed alert behavior. The viewer values
have been selected, but their implementation and acceptance evidence remain
pending. Stream and enrollment values stay unset until their owners publish
reviewed policy and receipts. Support has no invented response-time objective.
