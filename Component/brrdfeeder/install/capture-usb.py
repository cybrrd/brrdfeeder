#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Read-only fresh-install USB Wi-Fi selection. Existing config bypasses this."""
from pathlib import Path
import re
import subprocess
import sys


def event(path):
    return dict(line.split('=', 1) for line in path.read_text().splitlines() if '=' in line)


def monitor(phy):
    result = subprocess.run(['iw', 'phy', phy, 'info'], check=True, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
    modes = result.stdout.split('Supported interface modes:', 1)
    if len(modes) != 2:
        raise ValueError('wireless driver did not report supported interface modes')
    return bool(re.search(r'^\s*\* monitor\s*$', modes[1].split('Band ', 1)[0], re.M))


def choose(root=Path('/sys'), capability=monitor):
    devices = (root/'devices').resolve(strict=True)
    candidates, reasons = [], []
    for net in sorted((root/'class/net').iterdir()):
        if not (net/'phy80211').exists():
            continue
        if not re.fullmatch(r'[A-Za-z0-9_][A-Za-z0-9_.:-]{0,14}', net.name):
            raise ValueError('invalid Wi-Fi interface name')
        device = (net/'device').resolve(strict=True)
        if not device.is_relative_to(devices):
            raise ValueError('Wi-Fi device escapes sysfs devices')
        usb = next((p for p in (device, *device.parents)
                    if p.is_relative_to(devices) and ((p/'idVendor').exists() or (p/'idProduct').exists())), None)
        if usb is None:
            reasons.append(net.name+': not a USB adapter')
            continue
        vid, pid = (usb/'idVendor').read_text().strip(), (usb/'idProduct').read_text().strip()
        if not re.fullmatch(r'[0-9a-fA-F]{4}:[0-9a-fA-F]{4}', vid+':'+pid):
            raise ValueError('invalid USB identity')
        ue = event(usb/'uevent')
        product = ue.get('PRODUCT', '').split('/')
        if (ue.get('DEVTYPE') != 'usb_device' or len(product) != 3
                or [int(x, 16) for x in product[:2]] != [int(vid, 16), int(pid, 16)]):
            raise ValueError('USB identity and kernel uevent disagree')
        driver = (device/'driver').resolve(strict=True)
        if driver.parent != (root/'bus/usb/drivers').resolve(strict=True):
            raise ValueError('Wi-Fi USB driver is not bound')
        ue = event(device/'uevent')
        if ue.get('DEVTYPE') != 'usb_interface' or ue.get('DRIVER') != driver.name:
            raise ValueError('USB driver and kernel uevent disagree')
        phy = (net/'phy80211').resolve(strict=True).name
        if not re.fullmatch(r'phy[0-9]+', phy):
            raise ValueError('invalid wireless phy name')
        if capability(phy):
            candidates.append(net.name)
        else:
            reasons.append(net.name+': USB '+vid+':'+pid+' driver '+driver.name+' has no monitor-mode support')
    if len(candidates) > 1:
        raise ValueError('more than one monitor-capable USB Wi-Fi interface found ('+', '.join(candidates)+'); refusing to guess. Supply --interface explicitly')
    if not candidates:
        raise ValueError('no USB Wi-Fi adapter with a bound driver and monitor-mode support found. Plug in the external capture adapter (ALFA AWUS036AXM/AXML). '+ '; '.join(reasons))
    return candidates[0]


if __name__ == '__main__':
    try:
        print(choose())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit('USB Wi-Fi capture selection refused: '+str(error))
