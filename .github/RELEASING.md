<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Maintainer release setup

These files do not configure GitHub account/repository settings. The development team must
complete the checklist below before setting the acknowledgement variable.
The release approver approves every release; publishing the GitHub Release draft is a separate
manual action. Never run this workflow from the private-history repository.

## Required settings (the development team, with the release approver)

1. The repository owner creates the **release** environment with the release approver
   as the required reviewer. Disable administrator bypass. Restrict deployment
   refs to version tags `v*`, and protect those tags against unauthorized
   creation, update and deletion. Decide the self-review setting explicitly:
   if self-review is prevented, the development team must trigger the release so the release approver can approve;
   do not remove the required reviewer to get a run through.
2. Protect main with PR review and the stable **Public offline contracts /
   contracts** check (select the observed check in GitHub after its first run).
   Require CODEOWNERS review and protect workflow/policy changes. Keep default
   Actions token permissions read-only and disallow Actions from approving PRs.
   Only the `publish-sign` job has package/content/attestation/OIDC write access.
3. Enable Dependency graph, Dependabot alerts and Dependabot security updates.
   Weekly grouped version updates are configured in dependabot.yml; supported
   security updates are event-driven. Never auto-merge dependency PRs. Container
   base updates and manually checksum-pinned tool updates require review.
4. Keep public Issues and Discussions **disabled**. Keep **private vulnerability
   reporting enabled**. Use private team coordination for ordinary bugs and
   ecosystem practice. No CLA agreement is provided until counsel writes one.
5. Configure CodeQL advanced setup from the supplied workflow (Go, Python,
   Rust; pinned CLI 2.27.1 supports Rust). Do not also enable a conflicting
   default-setup workflow. Scorecard uploads SARIF without OIDC. Its README
   badge is workflow status, not a numerical rating: Scorecard API publication
   would require OIDC and is intentionally disabled.
6. Inspect and test the environment protection, then set repository variable
   **RELEASE_APPROVAL_CONFIGURED=true**. Leave it absent/false until protection
   is in place. This acknowledgement is NOT the approval mechanism; GitHub's
   protected environment is. Removing protection while leaving it true is unsafe.
   Recheck this checklist after repository ownership or organization conversion;
   update CODEOWNERS and reviewer identity together.

## Workflow

Push a version tag such as `v1.2.3`, or manually dispatch that existing tag with
`gh workflow run release.yml --ref v1.2.3`. A manual dispatch on `main` runs the
same gate, both image/SBOM builds and host-updater build as a release, but the
tag-only `publish-sign` job cannot run and no release environment is entered.
For that build-only path, `release-image.sh` invokes release metadata with
`--dry-run`: it requires `workflow_dispatch` on `refs/heads/main`, validates the
engine manifest's version as canonical SemVer, and records a null tag.
The `engine` package version in `Component/aviary/engine/Cargo.toml` is the
authoritative product version; the release metadata gate refuses a tag unless
`vX.Y.Z` exactly matches engine version `X.Y.Z`. Never move a release tag. Tests and the
two native ARM64 image/SBOM builds run without write/OIDC permissions; the pinned
ARM64 host helper is reproduced separately. Build artifacts expire in seven
days; an expired approval needs a fresh build and fresh approval.

The release approver reviews the source SHA, full test gate, image metadata/SBOMs and build logs
before approving environment release. The publisher downloads artifacts from
THIS workflow run only, verifies both archives and their source/sequence labels,
then publishes with digest preservation and signs only those exact digests.
It does not rebuild. Both image digests receive GitHub provenance attestations
inside the approved job. The attestations are stored in GitHub's attestation API,
and the draft records their GitHub attestation URLs. They are deliberately not
attached to GHCR: the pinned attestation action only discovers registry credentials
at `~/.docker/config.json`, while the publisher keeps its short-lived image/cosign
credential isolated under `$RUNNER_TEMP`. Avoiding a second credential copy and
cleanup path is safer. The images and their cosign signatures remain in GHCR;
verify provenance with
`gh attestation verify oci://IMAGE@DIGEST --repo cybrrd/brrdfeeder`.
Provenance describes this workflow run; it is not a claim of an independently
isolated SLSA build level.

The publisher creates a **draft** GitHub Release with generated changelog,
source SHA, build sequence, image digests, per-image SBOMs, attestation URLs,
checksums and the installer-pinned host helper/license. The release approver reviews and publishes
that draft. CI never publishes the draft or signs S5 update manifests.

The draft also carries `release-receipt.json` and
`release-receipt.sigstore.json`. Cosign 2.6.5 signs the exact receipt bytes with
GitHub OIDC and the new Sigstore bundle format, after both image/provenance
steps. The receipt binds source/tag, workflow, event, run ID **and attempt**,
both image tuples and SHA-256 of the ten release evidence/helper assets. The
bundle and receipt themselves are included in SHA256SUMS (not recursively
inside the receipt). The private publisher verifies the bundle against the
exact tag workflow identity, GitHub issuer and source SHA, then independently
checks GitHub run completion, published release, assets and image identities.
No receipt or GitHub release grants permission to promote a ring: that needs a
separate protected operator approval for the exact manifest. Public jobs hold
neither S5 credentials nor its signing key and never contact S5.

Every history-derived component build sequence remains `1000 + commit count`.
Never lower the offset/floors, rewrite the public lineage or publish a build from
the larger private history. The 0.9.0 bootstrap host helper has separate fixed
build 4, rebuilt with Go 1.27.3. Reproduce its bytes, update the installer helper
pin, then hash that installer and update all three bootstrap installer pins.
Neither an image update nor an ordinary installer rerun replaces an existing
valid helper; fleet delivery needs its separately approved host-update path.

**Landing method — linear history (recorded 2026-10-28).** Repository
enforcement: main requires linear history and disables merge commits; squash
and rebase-merge are both permitted by the settings. Our policy is stricter:
land a release as **one squash** of the single combined candidate PR onto an
unchanged main — never rebase-merge, never an admin bypass, no individually
merged component PRs, no intermediate landing commits carrying retired pins.
Immediately verify the resulting main commit M **before any publication
step**: exactly one parent (the previous main), the exact expected tree, and the
complete installer → helper-pin → three-bootstrap pin chain (installer SHA, its
sole `RELEASE_HELPER_SHA256`, every `bootstrap*.sh` installer pin); reproduce the
helper bytes from M with the pinned Go 1.27.3 official script and require the
pinned hash. Then wait for M's own hosted gate — `contracts`, all three
Go/Python/Rust CodeQL `analyze` jobs green, a code-scanning CodeQL analysis per
language for M's exact SHA with no analysis error, and the main-only Scorecard.
Retain the job and analysis receipts before final release approval. The `CodeQL`
aggregate check is PR-only; it is not emitted on pushes to main and is not a
main gate. A green pre-merge PR head is not a substitute for M's own evidence.
Record the open code-scanning alert count on main at M and have the release
approver explicitly review the alerts and disposition before the cut; successful
analysis does not mean there are no open findings. A later change to installer
bytes requires a new reviewed pin chain before anything is published.

After approved publication of the complete 0.9.0 hash chain, the explicit path is:

```sh
curl -fsSL https://get.cybrrd.com | bash -s -- --upgrade-helper
```

The installer flag works with the old installed command and requires no staging
step. It downloads only the pinned build-4 SHA-addressed artifact (or accepts
`--helper-file PATH` with the same SHA256), preserves identity, credentials and
all workload pins/floors, and never enrolls. An absent runtime credential is
accepted only with the configured 0.8.31 system credential path; present runtime
state retains its strict ownership, mode and trust checks. Incomplete local identity, invalid
old-helper receipts, downgrades and equal-build/different-hash replacements are
refused. Do not present this command as live before publication or describe an
ordinary rerun as equivalent.

The reviewed local operation installs a backward-compatible stable launcher
before writing its extended host journal. Under launcher/updater locks,
`.previous`/checksum and journal retain helper bytes and previous independent
host-floor bytes. Self-test/exact-build checks bracket atomic replacement;
receipt/floor writes precede durable journal removal. Failure/crash restores
helper, receipt and floor; the new launcher remains to finish recovery. This
does not publish or enable signed host metadata. Require all six refusal
mutation proofs plus the old/renewal-layout acceptance and refusal mutations,
byte-preservation/rollback/crash tests, full offline gate, two
helper reproductions and native ARM64 interruption/service proof before release.
The drill versions and their existing build floors remain unchanged.

## Failure handling

No registry operation occurs until both image archives pass validation. Publishing
two images/signatures is not transactional: a later failure can leave the first
image/signature present. Never treat that as an approved complete release. Inspect
the run, exact digests and attestation records; do not change a tag to different
bytes. Rerun **all jobs**, not only failed jobs: inputs from a previous attempt
are deliberately rejected even when the run ID is unchanged. Rerunning the
gated job requires environment approval again. If a draft
already exists, the script refuses rather than clobbering or publishing it.
Registry credentials exist only under `$RUNNER_TEMP/registry-auth` during the
publisher and are removed in the final `always()` cleanup step. The attestation
steps use the job's OIDC token plus `attestations: write`; draft creation uses the
job token's `contents: write`. The receipt upload is retained for seven days and
fails if draft preparation produced no files.

Action commits and tool checksums are pinned. The upstream Scorecard action
itself references its versioned v2.4.4 container; review upstream transitive
dependencies when changing any action pin. Hosted runner OS images still receive
GitHub maintenance updates. First hosted executions, environment enforcement and
account settings require operator validation; offline tests cannot prove them.
