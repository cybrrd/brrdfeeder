#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Late CP2102N discovery with real PTYs and fake sysfs/proc; no host effects."""
import contextlib
import errno
import importlib.util
import io
import os
from pathlib import Path
import pty
import re
import select
import tempfile
import termios
import threading
import time
import unittest
from unittest.mock import patch

INSTALL = Path(__file__).resolve().parents[2]
ROOT = INSTALL.parents[2]
HELPER = Path(os.environ.get('GPS_HOTPLUG_RUNTIME', INSTALL/'gps-runtime.py'))
spec = importlib.util.spec_from_file_location('gps_hotplug_runtime', HELPER)
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)

GOOD = (b'$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n'
        b'$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A\r\n')


class Hotplug(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.sys = self.root/'sys/class/tty'; self.sys.mkdir(parents=True)
        self.dev = self.root/'dev'; self.dev.mkdir()
        self.proc = self.root/'proc'; self.proc.mkdir()
        self.state = {}
        self.patches = [patch.object(runtime, 'SYS_TTY', self.sys),
                        patch.object(runtime, 'DEV_ROOT', self.dev),
                        patch.object(runtime, 'PROC', self.proc),
                        patch.object(runtime, 'DEVICE', self.dev/'cybrrd_gps'),
                        patch.object(runtime, 'PROBE_SECONDS', .25)]
        for item in self.patches:
            item.start(); self.addCleanup(item.stop)
        self.fds = []

    def tearDown(self):
        for fd in self.fds:
            with contextlib.suppress(OSError): os.close(fd)

    def tty(self, name='ttyUSB0', usb='10c4:ea60', port='1-1.3'):
        master, slave = pty.openpty(); self.fds.extend((master, slave))
        physical = self.root/'sys/devices/usb1'/port
        endpoint = physical/(port+':1.0')/name; endpoint.mkdir(parents=True)
        (physical/'idVendor').write_text(usb.split(':')[0]+'\n')
        (physical/'idProduct').write_text(usb.split(':')[1]+'\n')
        klass = self.sys/name; klass.mkdir()
        (klass/'device').symlink_to(endpoint)
        (self.dev/name).symlink_to(os.ttyname(slave))
        return master, slave

    def feed(self, master, data=GOOD):
        def writer():
            time.sleep(.05)
            with contextlib.suppress(OSError): os.write(master, data)
        threading.Thread(target=writer, daemon=True).start()

    def test_absent_install_then_late_valid_nmea_is_qualified(self):
        master, slave = self.tty(); before = termios.tcgetattr(slave); self.feed(master)
        identity = runtime.discover(self.state, 100)
        self.assertEqual(identity, runtime.device_identity(self.dev/'ttyUSB0'))
        self.assertEqual(self.state['qualified']['usb_id'], '10c4:ea60')
        self.assertEqual(termios.tcgetattr(slave), before)
        if select.select([master], [], [], 0)[0]:
            try:
                self.assertEqual(os.read(master, 4096), b'', 'runtime probe wrote or echoed bytes')
            except OSError as error:
                self.assertEqual(error.errno, errno.EIO)

    def test_no_nmea_is_not_adopted_and_same_identity_is_rate_limited(self):
        self.tty()
        self.assertIsNone(runtime.discover(self.state, 100))
        self.assertEqual(self.state['probe']['outcome'], 'no-nmea')
        with patch.object(runtime, 'confirm', side_effect=AssertionError('backoff must not reopen')):
            self.assertIsNone(runtime.discover(self.state, 101))

    def test_busy_alias_is_never_opened(self):
        _, slave = self.tty()
        (self.proc/'777777').symlink_to(Path('/proc')/str(os.getpid()))
        with patch.object(runtime.os, 'open', side_effect=AssertionError('busy tty opened')):
            self.assertIsNone(runtime.discover(self.state, 100))
        self.assertEqual(self.state['probe']['outcome'], 'busy')
        self.assertIsNotNone(slave)

    def test_unknown_ownership_is_never_opened(self):
        self.tty()
        self.proc.rmdir(); self.proc.write_text('not a visible process tree')
        with patch.object(runtime.os, 'open', side_effect=AssertionError('unknown-owner tty opened')):
            self.assertIsNone(runtime.discover(self.state, 100))
        self.assertEqual(self.state['probe']['outcome'], 'ownership-unknown')

    def test_restore_failure_overrides_valid_nmea_and_blocks_identity(self):
        master, slave = self.tty(); before = termios.tcgetattr(slave); self.feed(master)
        real = runtime.termios.tcsetattr; calls = []
        def fail_restore(*args):
            calls.append(args)
            if len(calls) == 2: raise termios.error('injected restore failure')
            return real(*args)
        with patch.object(runtime.termios, 'tcsetattr', side_effect=fail_restore):
            self.assertIsNone(runtime.discover(self.state, 100))
        self.assertEqual(self.state['probe']['outcome'], 'restore-failed')
        with patch.object(runtime, 'confirm', side_effect=AssertionError('failed identity must remain blocked')):
            self.assertIsNone(runtime.discover(self.state, 1000))
        real(slave, termios.TCSANOW, before)

    def test_qualified_identity_is_idempotent_without_reopening(self):
        master, _ = self.tty(); self.feed(master)
        first = runtime.discover(self.state, 100)
        with patch.object(runtime, 'confirm', side_effect=AssertionError('qualified tty reopened')):
            self.assertEqual(runtime.discover(self.state, 101), first)

    def test_replacement_identity_requires_fresh_confirmation(self):
        master, _ = self.tty(); self.feed(master)
        first = runtime.discover(self.state, 100)
        (self.dev/'ttyUSB0').unlink()
        master2, slave2 = pty.openpty(); self.fds.extend((master2, slave2))
        (self.dev/'ttyUSB0').symlink_to(os.ttyname(slave2)); self.feed(master2)
        second = runtime.discover(self.state, 200)
        self.assertNotEqual(first, second)
        self.assertEqual(self.state['qualified']['identity'], second)

    def test_native_ublox_mapping_wins_without_bridge_probe(self):
        _, slave = self.tty()
        (self.dev/'cybrrd_gps').symlink_to(os.ttyname(slave))
        with patch.object(runtime, 'confirm', side_effect=AssertionError('native mapping probed')):
            self.assertEqual(runtime.discover(self.state, 100),
                             runtime.device_identity(self.dev/'cybrrd_gps'))

    def test_missing_custom_device_never_adopts_bridge(self):
        self.tty()
        with patch.object(runtime, 'DEVICE', self.dev/'operator-selected-gps'), \
             patch.object(runtime, 'confirm', side_effect=AssertionError('custom binding replaced')):
            self.assertIsNone(runtime.discover(self.state, 100))

    def test_ublox_sysfs_waits_for_native_udev_mapping(self):
        self.tty(); self.tty('ttyACM0', '1546:01a7', '1-1.4')
        with patch.object(runtime, 'confirm', side_effect=AssertionError('bridge probed beside u-blox')):
            self.assertIsNone(runtime.discover(self.state, 100))

    def test_multiple_bridges_refuse_without_opening(self):
        self.tty(); self.tty('ttyUSB1', port='1-1.4')
        with patch.object(runtime, 'confirm', side_effect=AssertionError('ambiguous bridge probed')):
            self.assertIsNone(runtime.discover(self.state, 100))
        self.assertEqual(self.state['probe']['outcome'], 'ambiguous')


class Wiring(unittest.TestCase):
    def test_installed_runtime_files_retain_uninstall_ownership_markers(self):
        uninstall = (INSTALL/'uninstall.sh').read_text()
        installer = (INSTALL/'brrdfeeder-install.sh').read_text()
        files = {'/usr/local/libexec/brrdfeeder-gps-runtime': INSTALL/'gps-runtime.py'}
        for ext in ('service', 'timer'):
            files[f'/etc/systemd/system/brrdfeeder-gps-runtime.{ext}'] = (
                ROOT/f'Component/aviary/deploy/quadlet/brrdfeeder-gps-runtime.{ext}')
        for installed, source in files.items():
            with self.subTest(installed=installed):
                marker = re.search(re.escape(installed) + r"\|([^']+)'", uninstall)
                self.assertIsNotNone(marker, 'installed runtime file lacks uninstall ownership rule')
                content = source.read_text()
                self.assertIn(marker[1], content)
                self.assertIn(content, installer)

    def test_helper_is_embedded_and_never_rewrites_gps_required(self):
        helper = (INSTALL/'gps-runtime.py').read_text()
        installer = (INSTALL/'brrdfeeder-install.sh').read_text()
        self.assertIn(helper, installer)
        self.assertNotRegex(helper, r"required['\"]?\s*[]\)]?\s*=")
        self.assertNotIn('safe_dump', helper)

    def test_add_trigger_and_timer_mirror_bluetooth_shape_without_claiming_bridge(self):
        installer = (INSTALL/'brrdfeeder-install.sh').read_text()
        deployed = (ROOT/'Component/aviary/deploy/udev/99-cybrrd-brrdfeeder.rules').read_text()
        for source in (installer, deployed):
            lines = [line for line in source.splitlines()
                     if '10c4' in line and 'brrdfeeder-gps-runtime.service' in line]
            self.assertEqual(len(lines), 1)
            self.assertIn('ACTION=="add"', lines[0]); self.assertIn('TAG+="systemd"', lines[0])
            self.assertNotIn('SYMLINK', lines[0]); self.assertNotIn('RUN', lines[0])
        for ext in ('service', 'timer'):
            unit = (ROOT/f'Component/aviary/deploy/quadlet/brrdfeeder-gps-runtime.{ext}').read_text()
            self.assertIn(unit, installer)
        self.assertIn('OnUnitInactiveSec=5s', installer)


if __name__ == '__main__': unittest.main(verbosity=2)
