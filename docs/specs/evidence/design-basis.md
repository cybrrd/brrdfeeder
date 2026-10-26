<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Public design basis

This is a sanitized record of accepted product decisions, not a test receipt.
Decision dates identify provenance; claim values live only in
[claims.yaml](../claims.yaml). Private meeting records, infrastructure details,
identities and licensed standards text are not reproduced here. The source
review date is 2026-10-26. The hardware projection was checked against
installation-page template revision `7fb1a3b060e66ef075de9b9496b1faf592946019`
and settings revision `67fc869f135805eba5d669a94a7f6094c70f9e10`. Future-wire
policy and sizing were checked against design revision
`27b45e0c5aa691bb2ba3e1d32cc1f19dffdc91c9`. These are source-review identifiers,
not public release tags or performance receipts. These decisions do not establish deployed behavior.

## Hardware

Installation-page source reviewed for this draft declares the board, OS, radio,
power and high-endurance media baseline recorded in the hardware claims. The
public installer and capability sources linked in those claims describe checks
and supported mappings. This is selection guidance, not qualification of every
listed hardware combination. Network suitability requires measurement.

## Power

Accepted hardware/outage decision (Decision 5b), 2026-10-26: power-loss protection is strongly
recommended, not mandatory. Measure and disclose the vulnerable write interval.
A zero-loss claim is permitted only inside a tested protected envelope. Sustained
writes call for high-endurance storage; media wear remains device-specific.

## Radio tiers

Accepted hardware decision (Decision 5a), 2026-10-26: whole-frame monitor-mode Wi-Fi is the
capture boundary. Standard HCI Bluetooth is accepted at the base tier; mark
unexposed packet fields as not exposed. A full-packet Bluetooth sniffer is an
optional higher tier in the future design, requiring its own implementation and
acceptance. Do not describe HCI reports as whole over-air packets.

## Display

Serial-display decision, 2026-10-25, and adopted display design, 2026-10-26:
show complete reported serials. Hide only operator position on Globe's display.
Retain that position in backend/network/state and keep all other telemetry in
full; other consumers may display operator position. This policy is not a claim
that every complete-record or display acceptance case has passed.

## Generations

The adopted future wire design, 2026-10-26, is design-only. The current record
format has known fidelity gaps. Preserve historical semantics; do not invent
future fields in older records. Implementation and deployment require their own
evidence. This suite does not republish the interoperability standards corpus.

## Retention

Accepted heartbeat decision recorded 2026-10-25: heartbeats are excluded from
permanent lake retention. This is separate from observation retention and from
operational/distress retention. No measured beta archive duration is supplied.

## Backlog

Accepted outage clarification, 2026-10-26: delayed observations go through ingest
to the archive, with capture and delivery times preserved. They must not be
played as live Globe positions. Recent receipt does not change capture age.

## Outage

Accepted beta outage ruling (Ruling 2), 2026-10-26: use a bounded on-disk outage buffer. On physical
exhaustion, keep stored records, stop adding, persist the exact gap, mark coverage
impaired and raise Red distress. Do not delete the only stored copy. Native
acceptance must cover restart, durable markers and a full distress queue.

## Future outage

Accepted future hardware/outage design (Decision 5d–5f) and sizing revision, 2026-10-26: size for
complete records at the declared busy-peak observation workload, reserve
headroom, and drain the backlog while capture continues. The illustrative
payload rate excludes additional overhead that needs measurement. Grow capacity
if needed; do not shrink fields or thin observations to satisfy the budget.
These estimates are future design, not the beta buffer or a bandwidth guarantee.

## Service

Accepted service decision (Decision 7, including amended first-contact clause 7c), 2026-10-26: send first contact as soon as possible on
every link. A measured miss of the narrow-link bound makes that link unsuitable
for live first contact; archive transfer continues. First-contact bounds are
included in beta acceptance, with no claim that the rest of the future design
has shipped. Future liveness, Red delivery and server revocation objectives are
separately scoped in the registry. Mobile offline alone is operational status;
prolonged fixed-node offline is distress. Human notification timing is separate.
Disconnected nodes have no local shut-off timer and keep capturing.

## Helper

Beta update decision, 2026-10-26: signed helper manifests are not published.
Existing nodes use the explicit helper-only installer path after its approved
publication. Signed engine/console updates are a separate mechanism. Candidate
source and native acceptance are not interchangeable.

## Capacity

Latest capacity ruling, 2026-10-26: viewer and per-IP values are advisory
thresholds that raise alerts, never hard count-based user limits. Apply the same
principle to capacity planning: monitor and manage load without arbitrary caps
that refuse users or thin observations. Future occupancy levels raise warnings;
physical exhaustion retains the explicit outage policy. Security enforcement
is not a capacity quota. Selected viewer values still need deployment evidence.

## Pending

Release work ownership, 2026-10-26: B07 owns viewer alert implementation and
acceptance; B05 owns stream capacity alerts and evidence; B04 owns sign-up and
quota policy and acceptance. The release owner has not decided whether public
Issues open for ordinary beta support. Leave undecided values unset. The
existing installation-page support alias is an ordinary help route, not a
response-time guarantee or a decision to open Issues.
