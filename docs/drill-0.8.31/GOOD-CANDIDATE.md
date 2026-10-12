<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Healthy v0.8.31 drill candidate

Independent from the deliberately unhealthy branch: no failure implementation
or failure commit is included. The only console behavior change is the public
`X-BRRDhouse-Release: 0.8.31` response header. This changes the embedded binary
and therefore console image, as do the truthful source/build OCI labels.

Acceptance: `ACCEPTANCE.md`, committed before implementation. The identical
production-process proof used for bad is run here with:

```sh
bash docs/drill-0.8.31/prove-console.sh true
```

It supplies synthetic fresh engine status to the real compiled console and
calls the real updater `consoleHealthy` through a test overlay. The overlay
never changes or ships in the pinned host-helper binary. Native execution,
receiver telemetry, real release CI and keyless signing are separate gates.

## Release ordering (proposal, not execution authority)

1. Review both candidate PRs. Never merge the bad PR. The release workflow
   accepts version tags independent of main ancestry (`release.yml:21,50,70`),
   and checks out the event SHA (`:32,57,81`). An ordinary branch dispatch
   cannot publish; only a main dispatch has the build-only dry-run path.
2. The earlier stable-release Guard A/B procedure required final-main push CI
   and a main-only dry run. For bad, explicitly approve an adapted procedure:
   freeze the reviewed branch SHA, validate version/lock and prerequisite
   ancestry, run later-authorized CI on that exact branch SHA, inspect current
   tag/environment protections, and establish tag/release absence. Repeat
   immediately before an immutable annotated tag is created. Do not report a
   main rehearsal as evidence for this branch. Bad's first actual ARM64 release
   build is a real gated tag run, not a dry run.
3. Commits currently carry `[skip ci]` under the preparation restriction. Future
   authorized `workflow_dispatch` of test/CodeQL at the branch can check them.
   After separately authorized tagging, dispatch `release.yml` at the existing
   bad tag if its push run was skipped. Never launch a duplicate release run;
   record its event, SHA, ID and attempt. A tag dispatch is a real release path.
4. The protected `release` environment and `RELEASE_APPROVAL_CONFIGURED=true`
   remain mandatory (`release.yml:68–78`). Review the successful test gate,
   both ARM64 images/SBOMs and helper before approval. The publisher downloads
   this run's artifacts (`:82–90`), verifies exact source/build/archive metadata,
   signs both images and creates a signed receipt and draft (`:96–125`).
   `draft-release.py:52–65` binds run/attempt, source/tag/workflow, both image
   tuples and the ten base asset hashes. Require all 13 uploaded assets.
5. `draft-release.py:68–70` sets `--draft`, not `--prerelease`. Explicitly set
   the bad draft's prerelease flag true and latest false, review those values,
   then publish after separate approval. API `prerelease` corresponds to CLI
   JSON `isPrerelease`; a hyphenated version does not set it in this script.
6. Sign and publish only the exact bad pair to dev after operational approval.
   Ordinary timer discovery, two failed health attempts, restoration of N and
   quarantine must be observed before introducing good. Never promote bad.
7. Merge only the good PR, preserving its commits with a normal merge commit
   (no squash). Compute `1000 + git rev-list --count <commit>` on full history
   for the final main release commit and compare to the actual bad receipt's
   independent engine and console builds. Require good strictly greater here.
   Tagging later by itself does not ensure this; a squash can regress the count.
   Recheck after any new commits, rebase or merge-strategy change.
8. For good on final main, retain the original Guard A → approved main dry run
   with `publish-sign` SKIPPED → Guard B sequence. Freeze source/CI/protection/
   artifact evidence, then separately authorize immutable `v0.8.31`, protected
   signing and draft publication with prerelease=false. Revalidate the actual
   receipt builds, both new digests and all signatures before signing a higher
   dev-ring sequence. No floor reset, tag movement or old artifact reuse.

P05 observes bad by timer; P06 restores both N pins/images and unchanged floors
with received Red; P07 observes the second failure and persistent pair quarantine.
P08 introduces the distinct higher-sequence good pair, downloads/verifies before
switching, and commits it. P09 requires two distinct fresh engine status writes,
console 200 with correct engine identity and stable restart identities for both
workloads. P10 requires independently received Silver and ordinary downstream
traffic, correlated with bad's Red. All six native cases remain NOT_RUN here.
