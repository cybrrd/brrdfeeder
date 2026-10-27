<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Product Technical and Operational Specifications

<!-- BEGIN GENERATED: metadata -->
Applies to: **0.9-beta** · Product: **0.9.0** · Specification revision: **2026-10-27**.

**Next/unreleased draft; release tag pending.** Source baseline: [6eed7b81](https://github.com/cybrrd/brrdfeeder/tree/6eed7b81a15415a4f9f9564cdb85341b886520cb). Future-v5 rows are separate design targets, not beta features.
<!-- END GENERATED: metadata -->

BRRDfeeder receives broadcast Remote ID on your own hardware. Start with
[Install](https://get.cybrrd.com) and [hardware requirements](REQUIREMENTS.md).
This sheet explains what to expect, what is still being tested, and where to
find evidence for a release. It is a draft for the next beta, not a claim that
all described changes have shipped.

## Evidence legend

- **Measured:** an executed test or probe has a public receipt with source
  revision, date, environment, denominator and result.
- **Target:** accepted desired behavior; the required acceptance evidence is
  absent or incomplete.
- **Unverified:** evidence is missing, stale or insufficient for this scope.

Evidence kind is separate: **source**, **synthetic**, **native**, or **field**.
Source code and design decisions explain a claim; they do not prove performance.
Synthetic checks do not establish radio reception or power-loss behavior.
Each table also separates **beta** expectations from **future-v5** design.
No claim in this draft has been promoted to measured from a private summary.

## At a glance

<!-- BEGIN GENERATED: summary -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **H07 — Power-loss protection** | Strongly recommended, not required | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#power) An inexpensive UPS or battery/DC supply can help. Protection alone does not establish a tested zero-loss envelope. |
| **D07 — Outage buffer default** | 2 GiB | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#outage) Beta on-disk spool design, not measured endurance. Duration depends on encoded record sizes and traffic. |
| **U02 — Existing-node helper update** | 1 explicit command per existing node | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#helper) [source 2](https://github.com/cybrrd/brrdfeeder/blob/0b6a132180d03812f0bf1e80c23c4f65206c311a/Component/brrdfeeder/install/README.md#explicit-helper-only-upgrade-090) Signed helper feed is not published for the beta. Use the documented helper-only command after the approved installer/helper chain is published. |
| **S01 — First contact on wired/Wi-Fi/LTE** | ≤ 2 seconds (target) | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#service) Send as soon as possible on every link. This is a beta acceptance objective adopted from the future service design; measurement is outstanding. |
| **S02 — First contact on narrow links** | ≤ 15 seconds (target) | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#service) Best effort as soon as possible. This target does not declare any particular narrow-link radio supported. |
| **P01 — Viewer advisory threshold** | 100 concurrent viewers | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#capacity) Chosen alert level; deployed behavior and public acceptance receipt remain pending. Metrics are internal only. This is an advisory monitoring threshold, not a user limit. |
| **P02 — Per-IP viewer advisory threshold** | 32 concurrent viewers per IP | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#capacity) Shared networks must not hit a fixed count-based wall. Threshold crossing raises an alert. Metrics are internal only. This is an advisory monitoring threshold, not a user limit. |
<!-- END GENERATED: summary -->

The declared setup uses an ARM board, a dedicated monitor-mode Wi-Fi adapter,
a recognized Bluetooth receiver, GPS, durable storage and Internet access.
Power-loss protection is strongly recommended, not required. Hardware variants
and field performance need their own acceptance evidence.

Reported serial numbers are displayed. Only operator position is hidden on
**globe.cybrrd.com's display**; that is not backend deletion. All other telemetry
is kept in full under the data policy. The current wire format still has known
fidelity gaps; the complete future record design is not a shipped beta feature.

Engine/console updates use signed metadata and immutable image identities.
The host helper has a separate beta update path. Outage handling preserves
stored data and makes missing coverage explicit; no unconditional zero-loss
claim is made.

Viewer counts are **advisory monitoring thresholds that raise alerts**. Crossing
them does not refuse users. The [retained protective guards](SERVICE-LEVELS.md#capacity-and-future-sizing)
show a reason, count the event and alert internal monitoring; metrics are internal
only. [Registration and node capacity](OPERATIONS.md#registration-and-node-capacity)
have a separate approved policy. Physical
storage can still fill; [outage behavior](OPERATIONS.md#outages-and-power-loss)
describes that separate condition.

For install help and bugs, use [GitHub Issues](https://github.com/cybrrd/brrdfeeder/issues),
enabled at launch, with [admin@cybrrd.com](mailto:admin@cybrrd.com) as the private
fallback. Vulnerabilities follow [private security reporting](../../SECURITY.md).

## Read more

- [Requirements and radio boundaries](REQUIREMENTS.md)
- [Data, installation, updates, outages and removal](OPERATIONS.md)
- [Beta service objectives and measurement methods](SERVICE-LEVELS.md)
- [Known limits and decisions still pending](KNOWN-LIMITS.md)
- [Release evidence, SBOMs and verification](SBOM.md)
- [Security reporting](../../SECURITY.md) and [contribution policy](../../CONTRIBUTING.md)

These are beta objectives, not a contractual SLA. There is no uptime guarantee,
service credit, aviation-safety warranty or certification claim. Do not use a
missing observation as proof that no aircraft was present.

## Version and evidence maintenance

Use the specification at the exact release tag when judging that release.
`main` means next/unreleased; this draft's source baseline is identified above.
The beta tag and release evidence remain pending. A source baseline is not
native acceptance of the beta or proof of the currently deployed platform.

[claims.yaml](claims.yaml) is the source for values, scopes, evidence labels,
verification scenarios and owners. Its JSON-compatible YAML syntax lets the
small generator run with the Python standard library. Tables and page metadata
are generated; edit claims, then run `python3 .github/scripts/specs.py` from the
repository root. CI runs the same command with `--check` and exercises rejection
cases. It checks documentation consistency, not runtime behavior.

Before release, bind the tag and source metadata, inspect release assets, confirm
time-sensitive update/support statements, and obtain the listed acceptance
receipts. A measured receipt must report failures and its real denominator.
Change a target's status only when evidence covers its actual scope. Publish
corrections in a later revision; do not move tags or silently rewrite history.
Public interoperability standards stay in their own publication; this suite
does not reproduce licensed standards or internal operational records.
