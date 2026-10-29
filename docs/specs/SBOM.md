<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Release evidence and software bills of materials

<!-- BEGIN GENERATED: metadata -->
Applies to: **0.9-beta** · Product: **0.9.0** · Specification revision: **2026-10-28**.

**Next/unreleased draft; release tag pending.** Source baseline: [6eed7b81](https://github.com/cybrrd/brrdfeeder/tree/6eed7b81a15415a4f9f9564cdb85341b886520cb). Future-v5 rows are separate design targets, not beta features.
<!-- END GENERATED: metadata -->

[Back to the information sheet](README.md). Select the exact installed tag on
[GitHub Releases](https://github.com/cybrrd/brrdfeeder/releases). Do not use a
mutable latest-release link as evidence for an older installation. This beta's
release tag and assets are pending.

<!-- BEGIN GENERATED: evidence -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **B01 — Release evidence coverage** | Per-image CycloneDX SBOMs, metadata, digests, provenance, checksums and signed receipt | 0.9-beta / **target** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/.github/RELEASING.md) Existing published baseline assets are linked separately. Beta assets and dependency coverage must be checked for the actual release. |
| **B02 — Security analysis** | CodeQL and Scorecard workflow links; no numeric security rating | 0.9-beta / **unverified** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/.github/workflows/codeql.yml) [source 2](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/.github/workflows/scorecard.yml) [source 3](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/SECURITY.md) A successful workflow is not proof of no vulnerabilities or certification. No remediation deadline is promised. |
<!-- END GENERATED: evidence -->

## Assets to inspect

| Asset | Purpose |
|---|---|
| `engine-sbom.cdx.json`, `console-sbom.cdx.json` | CycloneDX component inventories for the corresponding image builds |
| `engine-metadata.json`, `console-metadata.json` | Version, source revision, build and workflow identity |
| `engine-image.txt`, `console-image.txt`, `digests.txt` | Immutable image references and digests |
| `attestations.json` | Links to GitHub build provenance for the images |
| `SHA256SUMS` | Integrity checks for downloaded release assets |
| `release-receipt.json`, `release-receipt.sigstore.json` | Same-run source/tag/image/asset binding and its signature bundle |
| `brrdfeeder-release-arm64`, `GO-LICENSE` | Installer-pinned host helper and license |

For a previously published example, see the
[v0.8.31 release](https://github.com/cybrrd/brrdfeeder/releases/tag/v0.8.31), its
[engine SBOM](https://github.com/cybrrd/brrdfeeder/releases/download/v0.8.31/engine-sbom.cdx.json),
[console SBOM](https://github.com/cybrrd/brrdfeeder/releases/download/v0.8.31/console-sbom.cdx.json),
and [signed receipt assets](https://github.com/cybrrd/brrdfeeder/releases/tag/v0.8.31).
Those are baseline artifacts, not evidence that the beta has been released.
SBOMs and receipts remain release assets; this directory copies none of them.

## Verification

Download the selected release's assets into a new directory. With the full asset
set present, check file integrity:

```sh
sha256sum --check SHA256SUMS
```

Checksums alone do not establish publisher identity. Compare metadata, SBOM
image identity, receipt, exact source revision, tag and immutable image digests.
Verify the receipt signature against the exact release workflow identity and
source revision using the [release verification procedure](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/.github/RELEASING.md).
For each image, GitHub provenance can be verified with the documented template
below; replace `IMAGE` and `DIGEST` with the selected immutable reference:

```sh
gh attestation verify oci://IMAGE@DIGEST --repo cybrrd/brrdfeeder
```

Do not treat a release receipt as authority to promote an update ring. The
release workflow and signed update feed have separate approval boundaries.

## Coverage and security boundaries

An image SBOM is an inventory of that image build. It is not an inventory of the
whole host OS, radio firmware, every attached device or the entire hosted
platform. Check the actual asset's coverage before drawing conclusions about a
dependency. No current registration in an external dependency service is claimed.
Provenance is not an independently assessed SLSA level or proof of safe behavior.

[CodeQL](https://github.com/cybrrd/brrdfeeder/actions/workflows/codeql.yml) and
[Scorecard](https://github.com/cybrrd/brrdfeeder/actions/workflows/scorecard.yml)
show workflow activity. A badge is not a numerical security rating or a claim
of no vulnerabilities. Use [private security reporting](../../SECURITY.md) for
suspected vulnerabilities; no acknowledgement or remediation deadline is promised.
