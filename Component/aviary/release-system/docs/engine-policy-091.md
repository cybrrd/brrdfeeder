<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Engine restart-policy migration (draft)

This change is stacked on helper-5 draft PR #80 at
`15b835400eb5732f21ca663a244ad845bae7fda1`. It implements the reviewed
`engine-restart-policy-v1` design (SHA256
`f6e41b3f4b5d55124f22a945d9eb3e53d7757a10ef0d6e84d9d8a53ac05085cc`).
It is not a signed release or native qualification.

## Two separately buildable steps

| Commit/artifact | Identity | Initiates migration | Recovers policy journals |
|---|---:|---|---|
| Existing helper-5 base | 5 | No | No; strict reader rejects new active fields |
| First commit: bridge | 6 | No | Yes |
| Second commit: enabled | 7 | Yes, after all gates | Yes |

Build each artifact from its own commit with `bash build-host.sh OUTPUT_DIR` and
Go 1.27.3 on PATH. The script reproduces the ARM64 binary from two source paths
and verifies the commit's installer pin. Do not manufacture another build-6
binary from the enabled tree with linker overrides. The commits also update
the installer/bootstrap hash chain; these are draft pins, not published bytes.
`self-test` remains v1. Both new helpers expose exactly:

```
brrdfeeder-release recovery engine-restart-policy-v1 schema=1
```

Install/deliver 6, verify its provenance and soak, then deliver 7, so 6 is the
actual retained `.previous`. Current and previous bytes must match their
root-owned installed receipts, the executing helper must match current, and
both executable probes must advertise the exact recovery schema. A claimed
version alone is not enough. The launcher contract is unchanged.

A node that skips 6 remains `policy_pending_recovery_helper`; it does not stage
new policy-required work. Do not overwrite `.previous`, lower a floor, or ship
replacement bytes under the same build/hash. If already at 7 with an old
fallback, a later forward recovery-capable helper must make its actual previous
compatible, or an independently authorized full-installer repair is required.
No forward catch-up build is allocated or released by this draft.

### Reproduced draft identities

Bridge commit: `2ec281109e71e65069a1330ccc072a57d4547f5a` (direct child of
the helper-5 base above). The enabled artifact is built from this PR's second
commit. Both passed two-path reproduction and QEMU build/self-test/capability
probes; QEMU is not native qualification.

| Artifact | ARM64 SHA256 |
|---|---|
| Bridge 6 | `fca9304dfe16d86b31ba7cce3aa5b8795879207b2a5c5cf3e7fcd3e5df740025` |
| Enabled 7 | `4f47619a029518c652601d4a87e52e99f4ca608c91c97a3f260d310abbd5f473` |

## Authority, effects and recovery

Helper-only upgrade and independent signed host update never migrate policy.
A subsequent normal workload `poll` or staged `apply` uses the genuine eligible,
unexpired signed workload manifest. Policy reconciliation precedes the same
image/sequence no-op. Independent planner bits preserve image-change semantics:
policy-only work pulls no images and restarts only the engine once. Combined
workload changes use one all-or-rollback transaction and one engine restart.

Only the packaged root-owned, regular, single-link, mode-0644 engine Quadlet at
the canonical path is eligible. Its exact legacy `300`/`3` and `always`/`5`
directives, active healthy baseline, unit origin, no drop-ins, resource checks,
config integrity and both recovery executables must pass. The narrow transform
changes only `StartLimitIntervalSec=300` to `StartLimitIntervalSec=0`, retaining
burst 3 and unrelated bytes. Already-current file/effective policy is a no-op.
Custom/ambiguous directives, aliases, unsafe paths, inactive/unhealthy baseline,
unknown overrides and third-party changes are refused, with original bytes kept.
Cooperating installer/admin writers must take the shared lock; a root actor
ignoring the lock cannot be made race-free by userspace comparisons.

A versioned object exists only in the active private transaction. It records
exact before/after hashes and metadata, original effective policy, image and
invocation baselines, config hash, owned holds and bounded recovery state.
The original Quadlet remains the sole original-byte backup. The complete
prepared journal and public marker are durable before the file mutation.
Reload/readback, a distinct engine invocation, exact image, fresh radio/GPS
health, console freshness and restart stability precede confirmation.

Unconfirmed recovery restores exact bytes/metadata, reloads and verifies the
legacy effective policy even when image IDs match, and confirms original
readiness. Policy-only rollback never writes/restarts the console. Unknown
third-party bytes/config/overrides leave an operator conflict and retain holds.
Confirmed recovery finishes commit rather than undoing the migration. Recovery
is local: it needs neither live manifest service nor an unexpired feed.

One automatic attempt per rule survives image and release changes. A private
`engine-policy-budget.json` tracks it and transaction-owned hold cleanup. Policy
failures do not quarantine an unchanged image pair or erase prior pair attempt
history. Recovery permits at most two automatic starts with one-minute backoff.
`authorize-policy-retry` is a root-only, lock-protected explicit authorization;
it replenishes a budget only, without restarting workloads, bypassing checks or
clearing unrelated counters. A planned migration/recovery restart may reset
only the engine start counter once at that operation boundary. There is no
counter-clearing timer. PID, invocation, logs and counter history are not
promised to roll back.

The public `engine_policy_outcome.json` contains only bounded rule/outcome/time
and, when readable, effective interval/burst/restart/backoff. It never includes
config bytes, credentials or executable paths. Receipts do not authorize drift.
Temporary holds carry transaction labels and are checked before removal; a
policy-only hold never replaces the historical package rollback anchor.

## Coverage decision still required from Cy

Accept staged convergence with visible pending cohorts, or require automatic
one-step coverage for every node that misses the bridge? This draft implements
staged convergence. The stronger guarantee needs a separately reviewed,
authenticated recovery-bundle protocol. This choice is not made by this PR.

| Cohort | Result |
|---|---|
| Same pair/sequence, healthy, eligible signed request, compatible fallback | Migrate on normal workload poll without invented metadata or pulls |
| Same engine, new console | Single paired migration/update transaction |
| Already-current file and effective policy | Verified no-op |
| Missed bridge | Pending recovery helper; forward chain or authorized repair |
| Polling disabled | Pending authorized normal poll; no timer enabled implicitly |
| Manifest absent, expired or ineligible | No unsigned policy operation |
| Stopped/failed/unhealthy engine or missing required hardware | Operator recovery prerequisite |
| Custom policy, unsafe metadata or conflicting edits | Refused/conflict, preserve bytes |
| Existing pending workload blocks helper-only upgrade | Preserve it; independent signed host path can deliver forward helper only without an active transaction |

Count migrated, already-current and each pending/refused cohort separately.
Helper installation is not evidence of engine-policy migration.

## Evidence and remaining release gate

Offline tests cover the planner matrix, same-image signed poll/staged apply,
combined updates, authority/prerequisite refusals, one-attempt budget, exact
rollback, source/config conflicts, bounded recovery, injected write failures,
strict old readers and independent assertion-failing mutations. The process
suite uses real SIGKILL at 15 durable boundaries, a broken current executable,
the unmodified Python launcher's fallback logic and an actual compiled Go test
executable. Test-only adapters supply owner/path boundaries and persisted fake
systemd/Podman/loopback health. This verifies process/state-machine behavior,
not a native ARM64 host or physical power loss.

Run from the module with a Go 1.27.3 toolchain on PATH:

```
go test -race -count=1 ./...
go vet ./...
POLICY_EVIDENCE_DIR=/absolute/evidence python3 tests/run-policy-proofs.py
D44_EVIDENCE_DIR=/absolute/evidence/rework python3 tests/run-rework-proofs.py
D44_EVIDENCE_DIR=/absolute/evidence/io python3 tests/run-helper5-io-proofs.py
```

**Native qualification: NOT RUN.** No host, production, signing or release
publication is authorized by this draft. Before release, use separately
approved disposable ARM64/systemd/Podman nodes and capture:

1. Provenance for exact helper-5 → bridge-6 → enabled-7 bytes, pins and floors;
   actual `.previous` at every step. Test both explicit helper-only and signed
   host-update paths, skipped bridge and forward catch-up; no floor reset.
2. The supported systemd `show` property spelling/value units, generated
   SourcePath/FragmentPath, absence of drop-ins and root:root 0644 source metadata.
   Validate same-image/current-sequence, engine-only, console-only and pair cases.
3. File/metadata/hash, config integrity, invocation IDs, exact images, counters,
   health and owned holds before/after. Exercise stopped/failed/custom/unsafe
   inputs and third-party edits. Credentials are not exported.
4. At least four distinct healthy engine invocations within 300 seconds after
   migration, with zero interval/always/5s, preserving backoff/resource controls
   and **no intermediate reset-failed or other counter clearing**. A crash loop
   is not a passing health result.
5. Kill/reboot across reservation, journal, write, reload, restart, health,
   confirmation and cleanup; corrupt current helper and prove the actual bridge
   fallback recovers offline. Include same-image `recovery_wait`, two-start cap,
   explicit retry and exact legacy bytes/metadata/effective rollback.
6. A failed/read-only/full filesystem and unavailable hold/Podman cases, retained
   pending requests, no policy-only console restart, package anchors preserved,
   and idempotent cleanup. Retain raw bounded evidence and explicit PASS/FAIL/
   NOT_RUN per case; stop on drift, third-party conflict or missing provenance.

Bridge review/soak, native evidence, coverage choice and the signing/publication
chain are separate release gates; this implementation must not be merged or
advertised as fleet-converged on offline evidence alone.
