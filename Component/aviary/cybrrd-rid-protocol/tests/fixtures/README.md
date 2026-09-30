<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Synthetic BLE session

`synthetic-ble-session.jsonl` is authored test data, not a capture or a modified
capture. All identities, locally administered MACs, dates and coordinates are
invented. It represents two aircraft sharing one operator station, legacy BT4
and coded BT5 transport, location before identity, and the literal operator ID
`"0"`. UNKNOWN is absence of aircraft identity, not a serial number.

The decoder tests re-encode these rows as ASTM messages to exercise address
isolation, cross-PHY association, non-replayed positions and the 60-second TTL.
Fuzz seeding also encodes the shared station and operator ID. Never replace this
fixture with real node/customer observations.
