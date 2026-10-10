<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Joining and proving signed convergence

This is a preregistration guide, **not executed native proof** and not permission
to contact or change a participant's node. Code review, a CI build, a published
GitHub release, signer deployment, feed publication and participant promotion
are separate events. The operator records an explicit approval for each.

## Before a one-time join

Classify the installed helper, recovery launcher, configured ring, namespace,
key, image identities, release/build floors and enabled timers. A compatible
installation can receive a signed image update without reinstall; a legacy
baseline or newer required helper may need an approved one-time join. Workload
updates do not replace the independent host helper. Standalone host-update
metadata remains unpublished until its separate signing path is validated.

Before uninstalling anything, have all of these available:

- Exact approved source revision, engine/console digests and builds; release
  receipt/signatures, helper SHA/build, installer SHA and matching bootstrap.
- Owner consent, physical/recovery access where needed, preserved nonsecret
  configuration/diagnostic receipts and a known rollback/support route.
- A coordinated old-credential revocation/re-enrollment plan and verified server
  grants/streams for Silver, operational/Red and ordinary receiver traffic.
- Adequate local rollback storage and the correct rootful engine/rootless
  console stores. No global prune, unrelated image removal or host upgrade.

Do not record credentials, signing seeds, raw operator coordinates or complete
secret-bearing config in review artifacts. Keep raw sensitive evidence private;
review only redacted receipts with timestamps and content hashes. Do not reset
floors, edit journals or falsify revisions to force eligibility.

## Exact candidate registration

Record N, a deliberately unhealthy **distinct** candidate and a good N+1 before
the drill. Each record includes both full image digests/revisions/builds,
keyless signature/OCI identity receipts, exact signed manifest bytes/hash,
ring/sequence/rollout/salt/validity, approval and publication/readback receipts.
Include a real console digest change. A wrong revision or unsigned image is a
verification-rejection test, not the validly identified bad-health candidate.

Run bad-first: the bad candidate rolls back to N without committing higher
component floors. Repeating it can demonstrate quarantine after two failures;
new sequence/salt must not re-enable that digest pair. Then the distinct good
N+1 has a higher ring sequence and nondecreasing independent component builds.
If good-first is chosen instead, the later bad trial must be a higher-build
N+2 rolling back to good N+1. Never request a remote downgrade.

## Native observations

Let the ordinary timer discover the feed. Do not send a Blue/NATS trigger,
invoke `apply`, write the mailbox or change ring configuration to manufacture
discovery. Retain timestamps for last poll/floor, manifest fetch, downloads,
staging, switching, health, commit/rollback and receiver delivery.

For the bad candidate, verify restoration of both prior Quadlet pins and actual
running image digests, restored health, outcome/journal, unchanged committed
floors and independently received Red. Observe the second failed application
and quarantine separately. A missing Red receipt is a failed observation, not
proof merely because a host JSON says rollback.

For good N+1, verify both actual images and OCI identities, two distinct fresh
engine status writes, console HTTP 200 displaying the expected healthy engine,
stable unit/container restart counters and identities, committed floors and
running sequence. Confirm receiver-observed Silver and resumed downstream
traffic separately from local health. After a separately approved reboot,
verify persistence of the committed pair and functioning recovery/timers.

Distinguish expected operation at the old version during unavailable/slow
downloads from rollback of an unhealthy candidate. Prove rollback without
registry access using only approved lab fault injection; do not alter a
participant's host routing, firewall, APN, DNS or clock. A native process-kill
drill and a physical power-cut are separate cases and approvals. Fixture
SIGKILL, emulated ARM64 and local Podman tests are labeled as such.

Expected default timing: initial timer 2–7 minutes, later ticks roughly
15–20 minutes after completion plus scheduling delay; durable polling floor
also survives failures. HTTP has a 30-second timeout, commands five minutes,
engine and console health up to 180 seconds each, package service 20 minutes.
Use installed values and observed timestamps, not an invented overall SLA.

Stop on signature/floor mismatch, inability to restore the baseline, missing
required Red/Silver/downstream evidence, unexplained restart, actual identity
disagreement, owner-consent gap or mutation outside BRRDfeeder-owned files,
units/stores. Preserve evidence and remain in dev; do not loosen safety gates.

## Promotion and evidence accounting

Retain exact source heads, approvals, candidate identities and artifact hashes,
all attempted cases and their PASS/FAIL/NOT_RUN/BLOCKED dispositions. Missing
evidence is NOT_RUN/BLOCKED, never an implied PASS. Independent review checks
the actual records; a checklist or fixture summary cannot certify field health.

After approved dev proof and review, promote the **same proved good image
digests**, without rebuild or mutable-tag resolution, under a new signature and
sequence for staging. Repeat its approved observation/soak, then obtain explicit
general approval and owner coordination. Only general supports partial rollout;
the operator chooses percentage and soak, keeping the cohort salt fixed when
raising percentage. Public bootstrap pins switch last after approval. Never
publish the deliberate bad pair to staging/general.

Telemetry-exclusion thresholds and 30-day no-check-in revocation are separate
policy/enforcement work. Software currency is not proof of credential validity,
and receiver-observed check-in time is not the device's clock or poll timestamp.
