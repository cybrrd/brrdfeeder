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
`gh workflow run release.yml --ref v1.2.3`. Dispatch on main does not publish.
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
inside the approved job, also pushed to GHCR. Provenance describes this workflow
run; it is not a claim of an independently isolated SLSA build level.

The publisher creates a **draft** GitHub Release with generated changelog,
source SHA, build sequence, image digests, per-image SBOMs, attestation URLs,
checksums and the installer-pinned host helper/license. The release approver reviews and publishes
that draft. CI never publishes the draft or signs S5 update manifests.

Every history-derived component build sequence remains `1000 + commit count`.
Never lower the offset/floors, rewrite the public lineage or publish a build from
the larger private history. The bootstrap host helper's separate fixed build 1
and its installer pin are unchanged.

## Failure handling

No registry operation occurs until both image archives pass validation. Publishing
two images/signatures is not transactional: a later failure can leave the first
image/signature present. Never treat that as an approved complete release. Inspect
the run, exact digests and attestation records; do not change a tag to different
bytes. Rerunning the gated job requires environment approval again. If a draft
already exists, the script refuses rather than clobbering or publishing it.
Registry credentials are removed in an `always()` cleanup step.

Action commits and tool checksums are pinned. The upstream Scorecard action
itself references its versioned v2.4.4 container; review upstream transitive
dependencies when changing any action pin. Hosted runner OS images still receive
GitHub maintenance updates. First hosted executions, environment enforcement and
account settings require operator validation; offline tests cannot prove them.
