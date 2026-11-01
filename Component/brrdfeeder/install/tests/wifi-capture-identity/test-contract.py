#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Only fake sysfs, mocked iw and temporary config files; never host radios."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import types
import unittest
from unittest.mock import patch

INSTALL = Path(__file__).resolve().parents[2]
ROOT = INSTALL.parents[2]
spec = importlib.util.spec_from_file_location('capture_usb', INSTALL/'capture-usb.py')
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class CaptureIdentity(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='wifi-capture-test-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.sys = self.root/'sys'
        for part in ['devices', 'class/net', 'bus/usb/drivers']:
            (self.sys/part).mkdir(parents=True)

    def add(self, name='wlan1', port='1-1', phy=0, ident='0e8d:7961', driver='mt7921u'):
        usb = self.sys/'devices/usb1'/port
        device = usb/(port+':1.0')
        net = device/'net'/name
        radio = device/'ieee80211'/('phy'+str(phy))
        driver_dir = self.sys/'bus/usb/drivers'/driver
        for path in [net, radio, driver_dir]:
            path.mkdir(parents=True, exist_ok=True)
        (usb/'idVendor').write_text(ident[:4])
        (usb/'idProduct').write_text(ident[5:])
        (usb/'uevent').write_text('DEVTYPE=usb_device\nPRODUCT='+ident.replace(':', '/')+'/100\n')
        (device/'uevent').write_text('DEVTYPE=usb_interface\nDRIVER='+driver+'\n')
        if not (device/'driver').exists():
            (device/'driver').symlink_to(driver_dir)
        (net/'device').symlink_to(device)
        (net/'phy80211').symlink_to(radio)
        (self.sys/'class/net'/name).symlink_to(net)
        return device

    def test_absent_and_unknown_capability_refuse(self):
        with self.assertRaisesRegex(ValueError, 'no USB Wi-Fi adapter'):
            helper.choose(self.sys, lambda _: True)
        self.add()
        with self.assertRaisesRegex(ValueError, 'no monitor-mode support'):
            helper.choose(self.sys, lambda _: False)
        with self.assertRaises(OSError):
            helper.choose(self.sys, lambda _: (_ for _ in ()).throw(OSError('netlink/iw unavailable')))

    def test_single_usb_and_renamed_replacement(self):
        self.add()
        self.assertEqual(helper.choose(self.sys, lambda _: True), 'wlan1')
        (self.sys/'class/net/wlan1').unlink()
        self.add('wlxnew', '1-2', 1, '0bda:8812', 'rtw88_8812au')
        self.assertEqual(helper.choose(self.sys, lambda _: True), 'wlxnew')

    def test_two_capable_adapters_and_virtual_interfaces_refuse(self):
        self.add()
        self.add('wlan2', '1-2', 1)
        with self.assertRaisesRegex(ValueError, 'refusing to guess'):
            helper.choose(self.sys, lambda _: True)
        (self.sys/'class/net/wlan2').unlink()
        self.add('mon0', '1-1', 0)
        with self.assertRaisesRegex(ValueError, 'refusing to guess'):
            helper.choose(self.sys, lambda _: True)

    def test_missing_or_inconsistent_driver_and_usb_uevent_refuse(self):
        device = self.add()
        (device/'uevent').write_text('DEVTYPE=usb_interface\nDRIVER=other\n')
        with self.assertRaisesRegex(ValueError, 'driver and kernel uevent disagree'):
            helper.choose(self.sys, lambda _: True)
        (device/'uevent').write_text('DEVTYPE=usb_interface\nDRIVER=mt7921u\n')
        (device.parent/'uevent').write_text('DEVTYPE=usb_device\nPRODUCT=0bda/8812/100\n')
        with self.assertRaisesRegex(ValueError, 'identity and kernel uevent disagree'):
            helper.choose(self.sys, lambda _: True)
        (device/'driver').unlink()
        with self.assertRaises((OSError, ValueError)):
            helper.choose(self.sys, lambda _: True)

    def test_builtin_is_not_an_automatic_usb_candidate(self):
        device = self.sys/'devices/platform/wifi'
        device.mkdir(parents=True)
        net = self.sys/'class/net/wlan0'
        net.mkdir()
        (net/'device').symlink_to(device)
        (net/'phy80211').mkdir()
        with self.assertRaisesRegex(ValueError, 'not a USB adapter'):
            helper.choose(self.sys, lambda _: True)

    def test_iw_probe_is_read_only_and_requires_explicit_monitor_support(self):
        with patch.object(helper.subprocess, 'run', return_value=types.SimpleNamespace(stdout='Supported interface modes:\n * managed\n * monitor\nBand 1:\n')) as run:
            self.assertTrue(helper.monitor('phy1'))
            self.assertEqual(run.call_args.args[0], ['iw', 'phy', 'phy1', 'info'])
        with patch.object(helper.subprocess, 'run', return_value=types.SimpleNamespace(stdout='Supported interface modes:\n * managed\nBand 1:\n')):
            self.assertFalse(helper.monitor('phy1'))

    def test_embedded_helpers_match_canonical_files_and_all_bootstrap_pins_match(self):
        import hashlib
        installer = (INSTALL/'brrdfeeder-install.sh').read_text()
        prepare = (INSTALL/'bootstrap-prepare.sh').read_text()
        self.assertEqual(prepare.split("<<'CAPTURE_USB_EOF'\n", 1)[1].split('\nCAPTURE_USB_EOF', 1)[0]+'\n', (INSTALL/'capture-usb.py').read_text())
        self.assertEqual(installer.split("<<'BOOTSTRAP_PREPARE_EOF'\n", 1)[1].split('\nBOOTSTRAP_PREPARE_EOF', 1)[0]+'\n', prepare)
        self.assertEqual(installer.split("<<'INSTALL_LOG_PY'\n", 1)[1].split('\nINSTALL_LOG_PY', 1)[0]+'\n', (INSTALL/'install-log.py').read_text())
        pin = hashlib.sha256((INSTALL/'brrdfeeder-install.sh').read_bytes()).hexdigest()
        for name in ['bootstrap.sh', 'bootstrap-dev.sh', 'bootstrap-d44.sh']:
            self.assertIn('INSTALLER_SHA256="'+pin+'"', (INSTALL/name).read_text())

    def config(self, auto, explicit='wlan1', existing=None):
        config = self.root/'config.yaml'
        if existing is not None:
            config.write_text(existing)
        elif config.exists():
            config.unlink()
        installer = (INSTALL/'brrdfeeder-install.sh').read_text()
        block = installer.split('# --- config.yaml: generate baseline template if absent', 1)[1].split('# Gate on placeholder VALUES', 1)[0]
        block = block[block.index('\n')+1:]
        prelude = '''set -euo pipefail
say() { :; }; ok() { :; }; warn() { :; }; run() { "$@"; }
atomic_install() {
  local dest=$4; shift 4
  if [[ $# == 0 ]]; then cat > "$dest.tmp";
  else local source=$1; shift; if [[ $# == 0 ]]; then cat "$source" > "$dest.tmp";
  else "$@" "$source" > "$dest.tmp"; fi; fi
  mv "$dest.tmp" "$dest"
}
CONFIG_PATH=$1; capture_auto_selected=$2; INSTALL_INTERFACE=$3
DRY_RUN=0; INSTALL_LATITUDE=''; INSTALL_LONGITUDE=''
'''
        subprocess.run(['bash', '-c', prelude+block, 'fixture', str(config), str(auto), explicit], check=True, capture_output=True)
        return config.read_text()

    def test_fresh_automatic_config_uses_usb_and_explicit_keeps_name(self):
        automatic = self.config(1)
        self.assertIn('  adapter_usb_id: auto\n', automatic)
        self.assertNotIn('  interface:', automatic)
        named = self.config(0, 'wlxchosen')
        self.assertIn('  interface: "wlxchosen"', named)
        self.assertNotIn('adapter_usb_id', named)

    def test_existing_config_is_byte_preserved_even_if_auto_flag_is_set(self):
        existing = '# owner choice\ncapture:\n  interface: wlan1\n  hunter: {enabled: false}\n'
        self.assertEqual(self.config(1, existing=existing), existing)

    def test_bootstrap_rerun_and_explicit_choice_do_not_probe_or_convert(self):
        source = (INSTALL/'bootstrap-prepare.sh').read_text().split('# ── Capture interface', 1)[1].split('# ── Sensor position', 1)[0]
        source = source[source.index('\n')+1:]
        prefix = "set -euo pipefail\nbootstrap_note() { :; }; bootstrap_die() { exit 1; }; bootstrap_say() { :; }; run_step() { exit 99; }; install_mode=$1; shift\n"
        for args in [('rerun',), ('fresh', '--interface', 'owner0')]:
            result = subprocess.run(['bash', '-c', prefix+source+'\ntest "$capture_auto_selected" = 0\n', 'fixture', *args], capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_fresh_bootstrap_connects_read_only_usb_probe_to_auto_intent(self):
        self.add('wlxnew')
        binary = self.root/'bin'
        binary.mkdir()
        iw = binary/'iw'
        iw.write_text('#!/bin/sh\n[ "$1" = phy ] && [ "$3" = info ] || exit 99\nprintf "Supported interface modes:\\n * managed\\n * monitor\\nBand 1:\\n"\n')
        iw.chmod(0o700)
        source = (INSTALL/'bootstrap-prepare.sh').read_text().split('# ── Capture interface', 1)[1].split('# ── Sensor position', 1)[0]
        source = source[source.index('\n')+1:].replace("root=Path('/sys')", 'root=Path('+repr(str(self.sys))+')')
        prefix = "set -euo pipefail\nbootstrap_note() { :; }; bootstrap_die() { exit 1; }; bootstrap_say() { :; }; run_step() { exit 99; }; install_mode=fresh\n"
        command = prefix+source+'\ntest "$capture_auto_selected" = 1\ntest "$1" = --interface\ntest "$2" = wlxnew\n'
        result = subprocess.run(['bash', '-c', command, 'fixture'], capture_output=True,
                                env=dict(os.environ, PATH=str(binary)+os.pathsep+os.defpath))
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn('  adapter_usb_id: auto\n', self.config(1,'wlxnew'))


if __name__ == '__main__':
    unittest.main(verbosity=2)
