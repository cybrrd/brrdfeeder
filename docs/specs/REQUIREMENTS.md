<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Hardware and system requirements

<!-- BEGIN GENERATED: metadata -->
Applies to: **0.9-beta** · Product: **0.9.0** · Specification revision: **2026-10-28**.

**Next/unreleased draft; release tag pending.** Source baseline: [6eed7b81](https://github.com/cybrrd/brrdfeeder/tree/6eed7b81a15415a4f9f9564cdb85341b886520cb). Future-v5 rows are separate design targets, not beta features.
<!-- END GENERATED: metadata -->

[Back to the information sheet](README.md). These are declared installation
profiles and recommendations. See each status before buying hardware; this is
not qualification of every board, receiver, firmware or storage combination.

## Setup

<!-- BEGIN GENERATED: requirements -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **H01 — Board and operating system** | Raspberry Pi 4/5 or Compute Module 4/5; ARM64; Raspberry Pi OS Lite 64-bit or Debian 13 | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#hardware) [source 2](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/brrdfeeder/install/README.md) Declared hardware baseline. It is not qualification of every board or firmware combination. |
| **H02 — Power supply class** | 27 W | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#hardware) Installation guidance, not a measured power draw or universal supply qualification. |
| **H03 — Capture Wi-Fi** | ALFA AWUS036AXML or AWUS036AXM; external monitor-mode adapter | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#hardware) [source 2](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/capabilities.toml) Capture radio is separate from the network uplink. Other chipsets need their own evidence. |
| **H04 — Standard Bluetooth receiver** | RTL8761B family; USB 0bda:876e or 0bda:a728; LE Coded PHY for Coded reception | 0.9-beta / **target** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/brrdfeeder/install/README.md) [source 2](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/capabilities.toml) Recognized USB IDs are not proof that every clone or firmware supports the required receive behavior. |
| **H05 — GPS receiver** | 1 receiver | 0.9-beta / **target** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/brrdfeeder/install/README.md) Use a supported u-blox receiver. Adafruit Ultimate GPS requires explicit opt-in and passive NMEA confirmation; no blanket PPS qualification. |
| **H06 — Storage planning baseline** | 64 GB | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#hardware) High-endurance media; USB SSD or high-endurance storage recommended for sustained writes. Guidance, not a signup or observation cap. Media life is unverified. |
| **H07 — Power-loss protection** | Strongly recommended, not required | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#power) An inexpensive UPS or battery/DC supply can help. Protection alone does not establish a tested zero-loss envelope. |
| **H08 — Network** | Internet over Ethernet or Wi-Fi; measure the actual link | 0.9-beta / **target** (source) | [source 1](evidence/design-basis.md#hardware) Capture Wi-Fi and backhaul have different roles. No carrier, coverage, or narrow-link hardware support guarantee. |
<!-- END GENERATED: requirements -->

Use a dedicated capture adapter. The built-in network adapter can provide the
uplink; it is not a substitute for monitor-mode capture. The engine rejects a
capture interface that carries the default route. The installer checks for a
measured GPS fix before engine startup and reports missing GPS or waiting for
fix. An unsupported receiver is not automatically a supported GPS device.

Bluetooth capture takes exclusive control of its selected receiver and disables
the host Bluetooth service while it owns capture. Bluetooth keyboards and audio
on that host will be affected. Follow the [installer guide](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/brrdfeeder/install/README.md) for device
selection, GPS opt-in and restore behavior. Installer instructions and source
checks are not a claim of native qualification.

## What the radios can hear

<!-- BEGIN GENERATED: reception -->
| Claim | Expectation | Scope / status | Source and boundary |
|---|---|---|---|
| **R01 — Wi-Fi Beacon and NAN** | Implemented receive paths; NAN evidence is software/fixture only | 0.9-beta / **unverified** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/capabilities.toml) [source 2](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/tools/NAN-RECEPTION.md) Existing software evidence does not qualify NAN over the air. See linked source evidence. |
| **R02 — Bluetooth reception** | BT4 legacy and BT5 extended/Coded via HCI advertising reports | 0.9-beta / **unverified** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/capabilities.toml) [source 2](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/requirements/REQ-BRRD-014.yaml) Field history and software tests do not close the remaining controlled RF cases. |
| **R03 — Bluetooth capture tiers** | Standard HCI dongle at base tier; optional full-packet sniffer at higher tier | future-v5 / **target** (source) | [source 1](evidence/design-basis.md#radio-tiers) HCI reports do not expose complete over-air packets, bad-CRC packets or IQ. Full-packet sniffer integration is future work, not a beta feature. |
| **R04 — Passive reception** | Listen without pairing, connecting or advertising to aircraft | 0.9-beta / **target** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/requirements/REQ-BRRD-014.yaml) Software configuration supports this policy; controlled passive-air qualification remains open. |
| **R05 — Channel coverage** | Scheduled channel visits; no continuous all-channel reception | 0.9-beta / **unverified** (source) | [source 1](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/capabilities.toml) [source 2](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/tools/CHANNEL-SCHEDULER.md) DFS capture is unsupported. A channel schedule or simulator cannot prove continuous coverage. |
<!-- END GENERATED: reception -->

The standard Bluetooth dongle exposes HCI advertising reports. It cannot be
called a full-packet sniffer. Fields unavailable at that boundary must be
identified as not exposed in the future full-record format. The optional higher
tier needs separate hardware and implementation evidence.

Reception depends on RF conditions, antenna placement, transmitter behavior,
channel visits, driver and controller firmware. No reception range, continuous
all-channel coverage, RF timing conformance or complete standards coverage is
promised. The [NAN evidence](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/tools/NAN-RECEPTION.md) and
[passive Bluetooth requirement](https://github.com/cybrrd/brrdfeeder/blob/6eed7b81a15415a4f9f9564cdb85341b886520cb/Component/aviary/requirements/REQ-BRRD-014.yaml)
state their open physical tests.
