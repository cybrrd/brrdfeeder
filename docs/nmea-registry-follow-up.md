<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# DEP-091-NMEA-REGISTRY: return nmea to crates.io

Status: OPEN. Owner: BRRDfeeder dependency maintainer (Codex owns PR #88).
Review at every dependency-update/release preparation until closed. This is an
explicit tracked follow-up, not a permanent exception for Git dependencies.

Trigger: a crates.io nmea release newer than 0.8.0 that contains upstream commit
`b2e7e4f6c1d6b21a0aae787adac35b6baee7c018` (or an independently reviewed equivalent
fix), permitting patched serde_with and keeping it optional without `serde`.
Do not switch merely because a higher version number appears.

Close only after a dedicated PR:

1. Verifies upstream release provenance and the packaged source's fix; replaces
   the full Git rev with the reviewed crates.io version and regenerates the
   workspace lock without unrelated dependency changes.
2. Confirms nmea has a registry checksum, no nmea Git source remains, serde_with
   is absent from the active engine tree (including all targets), and any locked
   serde_with is at least 3.21.0. Runs advisory checks without new suppressions.
3. Runs GPS fixtures, the engine suite and a cold dependency fetch followed by
   offline metadata/build or tests. Updates the public-ci guard from the temporary
   Git pin to the chosen registry provenance and retains the advisory floor.
4. Keeps semver-major update review policy, obtains review and green hosted CI,
   and records the closing PR/version here. No release or merge is authorized by
   this checklist.

Build preparation: see `Component/aviary/tools/BUILD-ARM64.md`. Both the registry
cache and Git cache (or complete vendor tree) must be available before offline
steps; a developer's warm cache is not evidence for a cold builder.
