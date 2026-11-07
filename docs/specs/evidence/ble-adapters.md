<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Bluetooth adapter qualification — 0.9.1

The runtime admission set is exactly **0bda:876e, 0bda:8771**. Admission permits
qualification; it is not proof of actual Coded-PHY RID reception.

| USB identity | Admission | Reception evidence boundary |
| --- | --- | --- |
| 0bda:876e | Allowed | Existing receiver path; validate kernel/firmware and actual reception for the node under test. |
| 0bda:8771 | **allowed-pending-native-Coded-PHY-proof** | Not listed-supported. Requires actual Coded-PHY RID capture on brrdg2s2, with USB identity, kernel, firmware, exact candidate and source attribution retained. |
| 0bda:a728 | Refused | Broken extended scan: `BTUSB_BROKEN_EXT_SCAN` suppresses extended scanning, so no Coded-PHY RID reception on the target kernel. |
| Every other ID | Refused | No wildcard/family/feature-bit admission; separate qualification required. |

Linux [v6.18.50 btusb.c](https://github.com/gregkh/linux/blob/v6.18.50/drivers/bluetooth/btusb.c)
flags a728 with `BTUSB_BROKEN_EXT_SCAN`; 8771 has an ordinary Realtek entry.
The [upstream narrowing change](https://github.com/torvalds/linux/commit/ca0583c24661749508a0979189c254388a685559)
moves the RTL8761B quirk to a728 specifically. Earlier kernels can apply the
quirk more broadly; do not infer Coded reception from family name, matching
`rtl8761bu` firmware, controller feature bits or the absence of this quirk alone.

An address pin cannot bypass the finite admission set. Unsupported devices do
not authorize takeover of bluetoothd. Explicit OFF, exclusive ownership,
operator conflict handling and rollback protections remain in force.

The native B16 procedure must retain source-attributed Coded-PHY RID evidence;
ordinary advertisements, selected-controller status and combined Wi-Fi/BLE
counters cannot promote the pending 8771 row. No native proof is claimed here.
