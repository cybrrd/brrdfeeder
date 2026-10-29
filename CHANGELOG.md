<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Changelog

## 0.9.0 — Unreleased public beta candidate

- Automatic engine-side HTTPS credential renewal with local replacement keys,
  independent JWT verification, private atomic credential rotation and explicit
  supervisor reconnect preserving the queue.
- Owner recovery through `sudo brrdfeeder renew`, without uninstalling or changing
  node identity; dedicated writable credential directory and read-only trust.
- Silver renewal status and Red expiry/recovery signals. Companion server renewal
  routes, identity-pinned owner recovery and a short-TTL canary must precede rollout.
- Go 1.27.3 across the console, host helper, container builds and CI.
- Patched Rust TLS dependencies: rustls 0.23.46 and quinn-proto 0.11.15.
- Durable, bounded telemetry custody during broker outages, with ordered replay
  of stored records and explicit loss diagnostics when capacity is exhausted.
- Explicit host helper build 2-or-3-to-4 upgrade through `--upgrade-helper`, preserving
  node identity, configuration and credentials, with transactional recovery and
  an idempotent repeat path. Existing 0.8.31 system-only credential layouts
  are accepted without creating or migrating credentials; present renewal
  credentials retain their full ownership, mode and trust checks.
- An unpublished host-updater ring manifest (HTTP 404) is a successful no-op
  with an explicit journal message. Executable 404 and all other HTTP failures
  still fail, with the download stage and numeric status in the error.
- Safe uninstall of known updater attempts, credential-renewal state and
  interrupted package writes. Private credentials are removed before their
  service account; pending-file hardlinks are accepted only as the validated
  two-name pair inside the private updater directory.
- Hermetic Bluetooth installer fixtures and job-wide Python bytecode suppression
  keep offline CI independent of host adapters and generated cache files.
- The v4 wire format is unchanged. Native helper/reboot/rollback qualification,
  deployed-server renewal and broker confirmation remain rollout gates. This
  candidate does not claim those checks passed or that a release was published.
- Existing broad uninstall rules for status files, engine-state `*.tmp` and
  quarantined pending entries remain; external temporary-file cleanup and tighter
  provenance checks are deferred. See `docs/uninstall-state-inventory.md`.

### Known limits

- Reinstalling with the same account and hostname issues a new credential
  without revoking the old credential. Contact support at admin@cybrrd.com to
  revoke the old credential; automatic revocation is planned for 0.9.1.
- Units on older releases need a one-time uninstall and reinstall to get
  self-update. There is no silent migration.
- Bluetooth: the adapter must be present at install; hotplug is planned for 0.9.1.

<!-- RELEASE-EDITOR CHOICE: keep exactly one Bluetooth line before publication.
The line above is for a release without Bluetooth hotplug. If reviewed Bluetooth
hotplug is included, replace that line with the alternative below.
BLE-WITH-HOTPLUG:
- Bluetooth: supported adapters can be connected or reconnected after installation,
  while preserving explicit disablement and adapter selection.
-->

## 0.8.31 — Published 2026-10-25 (build 1045)

- Prepare the healthy follow-up to the separately reviewed, deliberately
  unhealthy `0.8.31-drill.1` prerelease. The healthy console returns its normal
  status response and an `X-BRRDhouse-Release: 0.8.31` header; it contains no
  deliberate failure code. Native rollback and convergence are not yet proved.
- Includes the already merged [#44](https://github.com/cybrrd/brrdfeeder/pull/44)
  Red boot-grace change. No credential renewer is included.

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
