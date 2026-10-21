<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# CI-fix acceptance — make the drill release gate pass without weakening it

Registered 2026-10-21 BEFORE implementation, per the drill-CI-fix packet
of 2026-10-21 and its same-day extension (items 8–11).
Checked items are accepted scope, not completed evidence.

- [x] R2: SPDX headers added to ACCEPTANCE.md and BAD-CANDIDATE.md (repo .md form: HTML comments).
- [x] R3: release-real-podman conflict fixed without weakening any test.
- [x] R4: engine `Cargo.toml` stays `0.8.31-drill.1`; lock untouched.
- [x] R5: build_seq floor re-checked on final heads (hard stop if it would fail).
- [x] R6: proofs below run locally; negative control included.
- [x] R7 + extension: new commits only, no `[skip ci]`, draft kept; #46 SPDX fix in scope.

## Verified identity chain (not assumed)

- `release-image.sh` does **not** stamp the console binary: the console release path runs
  `release-metadata.py context` (asserts tag == engine `Cargo.toml` version) then
  `Component/brrdhouse/build.sh`, which injects identity only as OCI build-args
  (`SOURCE_REVISION`, `BUILD_SEQ` → labels) — "Build provenance stays in the OCI label,
  never in the console page". The binary itself carries no version.
- `run-runtime-proof.py` builds the fixture `brrdhouse` with plain `go build` (no env,
  no stamp); the test wraps it in `FROM scratch` with no ENV.

## Chosen mechanism: release-identity ENV injection

The deliberate 503 stays in this branch's `main.go` handler but is enabled by the
process environment instead of unconditionally:

1. `build.sh` derives the release identity the same way the release workflow does —
   the engine product version — and passes `--build-arg BRRDHOUSE_DRILL_UNHEALTHY=1`
   **only when that version is exactly `0.8.31-drill.1`**.
2. The drill branch's `Containerfile` bakes that build-arg into a runtime `ENV`
   in the final (scratch) stage.
3. `main.go` enables `drillHealthFailure` only when `BRRDHOUSE_DRILL_UNHEALTHY=1`.

Why this satisfies every constraint:

- (a) The release image for `0.8.31-drill.1` is built by `build.sh` from this branch,
  so it carries the ENV and returns 503 through the existing handler; the drill
  overlay proof exercises the real updater `consoleHealthy`.
- (b) Fixture consoles (plain `go build`, `FROM scratch`, no ENV) are healthy, so
  `TestRealPodmanLifecycleAndPrune` exercises real lifecycle/rollback mechanics —
  unchanged test, unchanged assertions.
- BAD-CANDIDATE rule: the failure needs BOTH this branch's code AND this release's
  exact version-derived ENV; no flag or setting enables it in any other build, and
  no OCI identity is falsified.
- `prove-console.sh` needs no change: the drill overlay test starts the drill binary
  with the ENV when proving unhealthy (`DRILL_EXPECT_HEALTHY=false`) and without it
  when proving healthy.

## Predicted results (each to be demonstrated)

| Proof | Prediction |
|---|---|
| `prove-console.sh false` | PASS — /status 503, real `consoleHealthy` false |
| `prove-console.sh true` (added sanity) | PASS — /status 200, `consoleHealthy` true |
| `release-real-podman` on new head | PASS (fixtures healthy; rollback exercised) |
| Negative control: `release-real-podman` on unchanged `e1315a51` | FAIL at `podman_test.go:293` as CI showed |
| Full `run.py --group all` on new head | All PASS except the two advisory local entries (`installer-ux-full`, `naming-compat` — the local test host's bluetooth dongle, per extension item 10) |
| `reuse-lint` | PASS after SPDX headers (302/302) |
| Engine identity | `Cargo.toml` + lock still `0.8.31-drill.1` |
| build_seq floor (final heads) | #45 = 1039 (39 commits: +2, the minimum), #46 = 1040 (40 commits: +1 SPDX) → good ≥ bad for engine and console (same formula both) |

If any prediction fails, stop and report rather than iterate silently.
