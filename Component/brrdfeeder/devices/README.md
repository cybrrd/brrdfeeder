<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Device inventory scaffold

`brrdfeeder devices scan [--json]` enumerates USB metadata only. It never opens
serial/HCI endpoints, invokes a qualifier, changes monitor mode or writes state.
Recognized ID plus driver means candidate, not available, supported on every
platform, healthy or selected. Multiple role candidates are reported as ambiguous.
Add/remove/replace deliberately refuse until transactional consent is implemented.

The standalone development command is `python3 Component/brrdfeeder/devices/scan.py scan --json`.
The installed self-contained CLI embeds identical source and registry bytes;
`generate.py --check` enforces parity. No second helper download or installed
file is introduced. Future packaging may install vendor data under
`/usr/share/brrdfeeder/`; it must remain release-bound and verified atomically.
This scaffold does not yet generate or replace existing runtime allowlists/rules.
Tests assert the initial GPS/BLE ID parity; consolidation needs golden tests.

Qualification identifiers refer to existing implementations, never executable
catalog commands: GPS `gps-runtime.py` (late qualification from GPS hotplug draft),
Bluetooth `bluetooth-state.py` and `rid_ble.rs` hotplug supervisor, and Wi-Fi
`capture.rs`/`watchdog.rs`. Scan does not call those effectors. Existing automatic
runtime policy continues independently; this command creates no new consent.

The u-blox family, CP210x bridge and Realtek HCI pair mirror runtime candidates.
Nordic serial is a separate unqualified legacy role. No bridge ID is labeled as
an Adafruit product. WLAN MT7921 USB is a candidate only: Linux lists
`0e8d:7961` in its `mt7921u` driver table; the ALFA AWUS036AXML product page names
MT7921AUN. These do not establish retail-revision or monitor-mode qualification:

- https://github.com/torvalds/linux/blob/master/drivers/net/wireless/mediatek/mt76/mt7921/usb.c
- https://www.alfa.com.tw/products/awus036axml

No profile currently carries a recommended tier or qualified-platform receipt.
No active capability or owner probe occurs; ownership is explicitly unverified.
The timestamped snapshot can immediately become stale and is not an apply lease.
USB disappearance can yield partial inventory; missing USB metadata refuses scan.
PCI/platform WLAN discovery and local health projection are deferred. Kernel
metadata enumeration alone cannot identify all service/USBFS owners or prove RF.
