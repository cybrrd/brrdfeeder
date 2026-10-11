<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->

# R1 handoff acceptance — before the corrective implementation

Scope: repair PR #44 without weakening the broker failure matrix, preserve
Red boot grace, and verify the complete offline contracts on current main.
Baseline PR head: `30560d6512594d88e72868d338632fcdeb6c678c`.

## Diagnosis to prove

The reported `upward_tests.rs:361` assertion compares **fault status**, not
the number of Reds. The preceding wait already establishes one real PubAck
from the Red worker with its disk broken. This test does not run `monitor()`;
the grace deadline cannot cause this failure. `Status::delivered(false)` in
the PR clears the still-broken storage status, whereas main retains fault=1.

The R1 design's blanket in-grace suppression remains valid. Its companion
fault-clear premise needs correction: main's post-PubAck store already clears
the preceding transport fault when durable=true. A live Red PubAck does not
repair local disk durability. Preserve the original no-change-to-Red-durability
acceptance requirement over the mistaken unconditional-clear test.

## Acceptance

1. Reproduce the unchanged local-broker failure on the inherited head. Keep
   its `acknowledged == 1`, `fault == 1`, and `failures > 0` assertions intact.
2. Add red-first unit coverage for connect/publish failure followed by PubAck:
   durable=true clears fault; durable=false replaces transport failure with
   disk fault=1. Assert acknowledgment count and subsequent failure re-arming.
   Include an already-existing disk fault and repeated non-durable delivery.
3. Correct only post-ack fault bookkeeping; do not change monitor timing,
   queue retirement, spooling, permissions, stream checks, retries or grace.
4. Existing R1 transient/persistent/late-fault/clear-return/Silver/config tests
   remain green. A regression to unconditional zero must fail the new unit
   test and unchanged broker matrix; a permanently latched transport fault
   must fail the durable-success case.
5. Rebase onto current main if behind, retaining original acceptance-first
   history. State old/new heads before an exact-lease force push.
6. Run the full Rust workspace, both local broker tests and the complete
   offline contracts job; report host/CI distinctions and any skips honestly.
   Require exact-head PR CI green before handoff completion.
7. Supply operator configuration documentation (default 300, zero immediate,
   clamp 3600, blanket Red-only suppression, GPS grace floor +60 seconds).
   No production/node contact; development identity for new commits; Kimi
   verifies, release approver merges/deploys separately.
