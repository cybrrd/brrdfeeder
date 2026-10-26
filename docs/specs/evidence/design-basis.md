<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Public design basis

These are accepted design choices, not execution receipts.

## Outage

Decision dated 2026-10-26: preserve stored observations when the outage buffer fills, stop adding, persist the exact gap, mark coverage impaired and raise Red. The beta size target is recorded in claims.yaml.

## Capacity

Decision dated 2026-10-26: capacity numbers are advisory thresholds that raise alerts. They must not refuse users or observations at an arbitrary count.
