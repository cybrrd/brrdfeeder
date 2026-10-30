<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Beta acceptance gaps

This public evidence-boundary review was recorded on 2026-10-30 for the 0.9.0
specifications. It describes missing instruments and measurements identified by
the acceptance-checklist review. It is not an executed receipt: no live probe,
restore, load run or physical power-cut test was performed by that review.
The registry records these behaviors as unverified, with no measurement receipt.

## Archived stream restore

True restore of archived Red/Silver streams has not yet been measured. No
reviewed instrument restores an export into an isolated disposable reader and
compares the ordered payload bytes with the original stream. Archive checksums
show integrity only. A restore drill for a different data store does not prove
stream restore or end-to-end archive custody.

## Live disk alert delivery

Live disk-alert delivery to a person has not yet been measured. The reviewed
tooling has no end-to-end instrument that crosses a live filesystem threshold
and proves warning, Red, resolution and witnessed human receipt. Offline rule
fixtures do not establish live metric ingestion, destination delivery or receipt.

## Public edge request bounds

Public-edge request-size, rate and timeout behavior has not yet been measured.
No reviewed instrument demonstrates rejection or timeout at each configured
boundary and acceptance within it. Route/authentication probes and per-browser
WebSocket guards do not establish the public edge's bounds. No numeric public-edge
limit or tested operating envelope is claimed. Acceptance needs a controlled,
authorized test environment; production flooding is not an acceptance method.

## Physical power loss

Physical power-cut behavior and the vulnerable write interval have not yet been
measured on the release hardware and storage. The bounded acceptance plan does
not include physical power cuts or native-radio memory qualification. Process
restarts, simulated outages, storage-full tests and synthetic replay do not
establish physical power-loss survival. A UPS recommendation is not a tested
zero-loss envelope.

## Overflow close reasons and counters

The reviewed queue tests establish bounds and disconnect behavior, but direct
assertions pairing the peer-visible `send_buffer_full` or `airspace_send_overflow`
close reason with its matching counter were not found. Those pairs remain
unverified. The `frame_too_large` reason/counter assertion covers that different
guard only; it cannot be generalized to overflow guards or live alert delivery.

## Viewer load beyond tested levels

Viewer load beyond the levels covered by executed receipts remains unmeasured.
The full 4,097-client load measurement is not run in the bounded acceptance plan.
A firing rule fixture at that count and tests that admit clients do not prove
throughput, latency or reliability at that load. Proposed smaller harness runs
are not results until executed and recorded. Advisory thresholds are not viewer
caps or a guaranteed supported capacity.

## Live alert human receipt

Live human receipt of operational alerts has not yet been measured. The reviewed
tooling describes a destination canary but has no executable live-canary command
that proves a person received it. Offline alert-routing and bridge checks do not
establish human receipt or acknowledgement. No notification or response deadline
is guaranteed; this gap also applies to live disk and viewer-guard alerts.
