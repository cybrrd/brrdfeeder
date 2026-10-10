<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Changelog

## 0.8.30 — Unreleased

Release preparation only. The changes below are merged into the release
candidate; this is not a claim that a release has shipped. The final tagged
commit and GitHub release notes determine the delivered scope.

- MERGED [#39](https://github.com/cybrrd/brrdfeeder/pull/39)
  ([eb229523](https://github.com/cybrrd/brrdfeeder/commit/eb2295236d41445b61a0ba5bf4905a68e29edbba)):
  sign a same-run
  release receipt and attach `release-receipt.json` and
  `release-receipt.sigstore.json` to the draft GitHub release, bringing its asset
  set to 13. These receipts are release evidence, not update-ring promotion
  permission.
- MERGED [#40](https://github.com/cybrrd/brrdfeeder/pull/40)
  ([9f7bc02b](https://github.com/cybrrd/brrdfeeder/commit/9f7bc02bc3a733dfdb547c1b910d3145803f8898)):
  reconcile the public
  host-updater build, join, and proof documentation; add documentation regression
  checks while preserving the distinction between fixture and native proof.
- MERGED [#42](https://github.com/cybrrd/brrdfeeder/pull/42)
  ([1010d0c5](https://github.com/cybrrd/brrdfeeder/commit/1010d0c51f3626a14fa37d62dcc79b8443c61d21)):
  Adafruit Ultimate GPS support through explicit USB-device opt-in and passive
  NMEA confirmation, plus default-off PMTK initialization. No claim of PPS
  enablement, hardware qualification, or a completed field measurement is made.
