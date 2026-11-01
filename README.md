<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# BRRDfeeder

[![Scorecard workflow](https://github.com/cybrrd/brrdfeeder/actions/workflows/scorecard.yml/badge.svg)](https://github.com/cybrrd/brrdfeeder/actions/workflows/scorecard.yml)

BRRDfeeder receives Remote ID observations. This repository contains its engine,
BRRDhouse local console, installer and update tooling.

- [Install](https://get.cybrrd.com)
- [Specifications](docs/specs/README.md)
- [Release evidence](docs/specs/SBOM.md)

- [Engine](Component/aviary/README.md)
- [Installer](Component/brrdfeeder/install/README.md)
- [Console](Component/brrdhouse/README.md)
- [Release workflow and maintainer setup](.github/RELEASING.md)
- [Security reporting](SECURITY.md)
- [Contribution policy](CONTRIBUTING.md)

The Scorecard badge reports workflow status, not a numerical security rating.
Detailed results go to GitHub code scanning. Outside contributions are not
accepted. [GitHub Issues](https://github.com/cybrrd/brrdfeeder/issues/new/choose)
are open for install help and bug reports through the **Install help** and
**Bug report** forms. Discussions remain disabled. Report security vulnerabilities
only through [private vulnerability reporting](https://github.com/cybrrd/brrdfeeder/security/advisories/new).
For private install help or bug reports, email **admin@cybrrd.com**.

Licensing is described in the root [LICENSE](LICENSE). Release approval and
publishing a draft release remain maintainer actions; successful CI alone does
not authorize a release.

## Bluetooth hotplug

With the matching engine and host installer update, an absent `sensors.rid_ble`
key means automatic reception when one supported Realtek **0bda:876e** or
**0bda:a728** adapter is plugged in. Explicit `enabled: false` stays off.
Missing or unplugged adapters are retried about every five seconds; local status
and the console explain waiting, unsupported hardware, ambiguous selection and
ownership conflicts. See [behavior, limits and native checks](docs/ble-hotplug.md).

## Ownership

Copyright © 2026 Macawi LLC, Iowa, USA.
BRRDfeeder and BRRDhouse are licensed under the GNU Affero General Public License v3.0 or later (see LICENSE).
cyBRRD, BRRDfeeder and BRRDhouse are developed and operated by cyBRRD Corporation, Iowa, USA, under license from Macawi LLC.
