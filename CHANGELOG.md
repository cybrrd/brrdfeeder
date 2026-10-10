<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Changelog

## 0.8.30 — Unreleased

Release preparation only. The items below are pending inclusion, not claims that
their PRs have merged or that a release has shipped. The final tagged commit and
GitHub release notes determine the delivered scope.

- Pending [#39](https://github.com/cybrrd/brrdfeeder/pull/39): sign a same-run
  release receipt and attach `release-receipt.json` and
  `release-receipt.sigstore.json` to the draft GitHub release, bringing its asset
  set to 13. These receipts are release evidence, not update-ring promotion
  permission. **Do not merge this version preparation before #39.**
- Pending [#40](https://github.com/cybrrd/brrdfeeder/pull/40): reconcile the public
  host-updater build, join, and proof documentation; add documentation regression
  checks while preserving the distinction between fixture and native proof.
- Optional, only if Cy merges
  [#42](https://github.com/cybrrd/brrdfeeder/pull/42) before the release tag:
  Adafruit Ultimate GPS support through explicit USB-device opt-in and passive
  NMEA confirmation, plus default-off PMTK initialization. No claim of PPS
  enablement, hardware qualification, or a completed field measurement is made.
  If #42 is not merged before tagging, GPS support is not part of 0.8.30.
