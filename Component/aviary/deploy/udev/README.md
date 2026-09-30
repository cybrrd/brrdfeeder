<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# udev rules — substrate-stable device naming

**`99-cybrrd-brrdfeeder.rules` is harvested verbatim from a live feeder, not authored here.**

## Provenance

Captured from **test-node-1** at `/etc/udev/rules.d/99-cybrrd-brrdfeeder.rules` (386 bytes, dated
2026-06-08) on 2026-07-18, during edge-beta Increment 0 (REQ-BRRD-001). Device identity confirmed
independently with `udevadm info -q property -n /dev/cybrrd_gps`:

```
ID_VENDOR_ID=1546      ID_MODEL_ID=01a7
ID_MODEL=u-blox_7_-_GPS_GNSS_Receiver
ID_SERIAL=u-blox_AG_-_www.u-blox.com_u-blox_7_-_GPS_GNSS_Receiver
DEVNAME=/dev/ttyACM1
```

REQ-BRRD-001 explicitly requires this rule be **harvested**, not invented — inventing a
device-matching rule is how you bind the *wrong device*, which is the exact failure the
requirement exists to prevent (substrate-discovered, not invented).

## Why this matters — the "lucky success" it replaces

On test-node-1 at harvest time: `/dev/cybrrd_gps -> ttyACM1`. The engine's **compiled default was also
`/dev/ttyACM1`** — so the default worked *by luck*. Had enumeration shifted to `ttyACM0` (the
2026-05/06 "port jumpiness" incidents), the symlink would have followed the real GPS while the
hardcoded default silently bound whatever landed on ACM1 — on test-node-1, that is the **BLE adapter**
(`/dev/cybrrd_ble -> ttyACM0`). Deterministic failure with a clear device name beats lucky success.

## Install

```
sudo install -m 0644 99-cybrrd-brrdfeeder.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
ls -l /dev/cybrrd_gps        # -> ttyACM*
```

Installed by `brrdfeeder-install.sh` as part of the handbook path (one Condo-Tenancy crossing).

## Known divergences (recorded, not silently reconciled)

1. **Two naming conventions exist in the substrate.** `docs/src/substrate-device-identity.md` §2
   documents Tier-1 rules in a file named `99-cybrrd-substrate.rules` using `/dev/brrd-ble`,
   `/dev/brrd-sdr` (hyphen, `brrd-`). The **deployed** rule on test-node-1 is
   `99-cybrrd-brrdfeeder.rules` using `cybrrd_gps`, `cybrrd_ble` (underscore, `cybrrd_`). The
   deployed form is authoritative (it is what the engine and `config.yaml` bind to); the doc needs
   truth-up. Filed rather than quietly "fixed", since the doc may describe a different fleet era.
2. **`MODE="0666"` is world-writable.** The doc's Tier-1 examples use `GROUP` + `MODE="0660"`,
   which is tighter. Harvested verbatim to preserve fidelity with the running fleet; tightening to
   `0660` is a candidate hardening item, but it must be a deliberate fleet-wide change verified on
   a node, not an edit made in passing here.
3. The `cybrrd_ble` rule is retained verbatim from the harvest even though `capabilities.toml`
   declares `sensors.ble_sniffer = none`. The rule is harmless (it only creates a symlink if the
   hardware is present) and removing it would diverge this file from the deployed fleet. The
   capability claim is governed by `capabilities.toml`, not by the presence of a udev rule.
