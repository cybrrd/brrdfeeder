<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# ACCEPTANCE — Adafruit Ultimate GPS (#746) support (development seat, 2026-10-08)

Packet: `PACKET-GPS-ADAFRUIT-ULTIMATE-746-2026-10-06.md (internal queue; path elided)` + ADDENDUM 1.
Registered BEFORE code (design note already drafted: `docs/adafruit-gps-746-design.md`).
Repo: public `cybrrd/brrdfeeder`, branch `feat/adafruit-gps-746-2026-10-08` (design note may
ride the same PR or a docs-first commit). The verifier audits; NOT merged; no version bump (batch
identity assigned at merge).

## Acceptance criteria

- AC1 **Recognition is opt-in and explicit (the director-amended):** the CP2102N (`10c4:ea60`) maps
  to `/dev/cybrrd_gps` ONLY via (a) interactive prompt, default **No**, read through the
  installer's existing tty discipline (`BRRDFEEDER_TTY_FD`/`/dev/tty`; pipe-with-tty prompts
  on the tty; **no tty = No**; `-y` never auto-accepts) or (b) the explicit headless flag
  **`--gps-usb-id <vid:pid>`** — then a **passive NMEA confirm** (≈5 s read at 9600,
  ≥2 checksum-valid `$GP`/`$GN` sentences, zero writes to the port) must pass before the
  udev entry renders; a failed confirm records "opted in, no NMEA seen" and the node runs
  the 0.8.28 degrade path. Recorded as `sensors.gps.usb_id` in config; rendered as an
  ADDITIONAL udev entry beside the unchanged u-blox entries. **Pre-existing third-party
  rules are reported, never deleted or overwritten** (brrdg3s3's interim rule: reported with
  removal left to the operator). Existing `usb_id` on re-run: honored without prompting
  (idempotent); survives auto-update by construction (updater never touches
  `/etc/brrdfeeder` or udev).
- AC2 **UART path explicit:** `--gps-uart` asks before any host-config change (serial
  console, overlays); never silently edits cmdline.txt/config.txt. Writes
  `sensors.gps.device` and a kernel-name udev entry.
- AC3 **Engine MTK handling:** generic-NMEA reader unchanged; optional PMTK init block
  (`init.pmtk/update_rate_hz/nmea_set/sbas`, default off) sends checksummed sentences once
  per open, logs sentences + `$PMTK001` acks; **no baud-change command** (stated in
  config docs). Unknown PMTK sentences in the stream regress nothing (tests).
- AC4 **PPS optional, behind config:** `--gps-pps <gpio>` asks before overlay/chrony
  changes; status/heartbeat gain `pps_source/pps_locked/clock_offset_ms` ONLY when the
  helper is installed (absence stays absent — no invented clock data).
- AC5 **Device boundary:** the GPS device/UART is BRRDfeeder-dedicated; the installer never
  touches unrelated serial/console config except via the explicit ask; no probing of
  non-declared USB devices.
- AC6 **Tests (red-first where behavioral):** udev-rule rendering (declared entry present,
  undeclared absent, invalid usb_id refused); installer recognition flow (prompt default-No,
  `-y` does not accept, accept-path writes config + rule); gps-seed tolerates PMTK ack
  sentences; engine PMTK init (sent sentences logged, once per open, default sends nothing);
  interim-rule supersession; UART flag ask-path (mocked host files).
- AC7 **No node contact, no host changes by me;** measurement plan is a the director-run document
  (§5 of the design note) with the grounding/bias-voltage preconditions the ADDENDUM
  demands; no hardware-list/get.cybrrd.com changes.
- AC8 Identity `cyBRRD Development <dev@cybrrd.com>`; workspace
  `the seat workspace (internal path elided)`; CI green (contracts incl. role-language,
  CodeQL, analyze); local build-arm64 at final head with fuzz corpus cleared.

## Amendments folded (the director answer 2026-10-08T22:34Z, recorded in the design note §2)

1. `--gps-usb-id` headless flag (interactive prompt stays; tty discipline per the uninstall
   confirmation pattern; no-tty = No).
2. Passive NMEA confirm before mapping; never write during the probe; failure = recorded +
   0.8.28 degrade.
3. Pre-existing rules never deleted/overwritten — reported, operator removes.
4. `sensors.gps.usb_id` survives auto-update (verified: updater replaces the container image
   only; config + udev untouched).
