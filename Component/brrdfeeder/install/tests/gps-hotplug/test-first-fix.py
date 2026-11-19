#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Runtime qualification -> first measured fix -> exact transport, with real PTYs."""
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import threading
import time
import unittest
from unittest.mock import patch
import yaml

INSTALL = Path(__file__).resolve().parents[2]
def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec); spec.loader.exec_module(result)
    return result

prior = module('hotplug_fixture', Path(__file__).with_name('test-contract.py'))
gps = module('seed_fixture', Path(os.environ.get('GPS_SEED_HELPER', INSTALL/'gps-seed.py')))
runtime = prior.runtime

class FirstFix(unittest.TestCase):
    def setUp(self):
        self.world = prior.Hotplug(); self.world.setUp()
        self.addCleanup(self.world.doCleanups); self.addCleanup(self.world.tearDown)
        self.root = self.world.root
        self.state = self.root/'state.json'
        self.config = self.root/'config.yaml'
        self.original = {'node': {'id': 'fixture'}, 'sensors': {'gps': {'required': True}}}
        self.config.write_text(yaml.safe_dump(self.original))
        for item in (patch.object(gps, 'CONFIG', self.config),
                     patch.object(gps, 'RUNTIME_STATE', self.state, create=True),
                     patch.object(gps, 'STARTUP', self.root/'startup.json'),
                     patch.object(gps, 'prestart_only'), patch.object(gps, 'report'),
                     patch.object(gps, 'held_elsewhere', return_value=False),
                     patch.object(gps, 'load_config', return_value=(self.original, self.config.read_bytes(), self.config.stat()))):
            item.start(); self.addCleanup(item.stop)

    def qualify(self):
        master, slave = self.world.tty('ttyUSB7')  # deliberately not ttyUSB0
        self.world.feed(master)
        current = runtime.discover(self.world.state, 100)
        self.assertIsNotNone(current)
        # Runtime's production path is /dev; the PTY itself is under /dev too.
        self.world.state['qualified']['path'] = os.ttyname(slave)
        self.state.write_text(json.dumps(self.world.state))
        return master, slave, current

    def test_late_attach_first_fix_releases_port_and_prepares_exact_transport(self):
        master, slave, current = self.qualify()
        before = self.config.read_bytes()
        # Stop a regressed endless missing-device wait deterministically.
        def no_missing_wait(seconds):
            raise AssertionError('qualified late GPS did not reach first-fix reader')
        stop = threading.Event()
        def feed():
            while not stop.wait(.03):
                with contextlib.suppress(OSError): os.write(master, prior.GOOD)
        worker = threading.Thread(target=feed); worker.start()
        try:
            with patch.object(gps.time, 'sleep', side_effect=no_missing_wait):
                gps.seed()
        finally:
            stop.set(); worker.join()
        value = yaml.safe_load(self.config.read_bytes())
        self.assertTrue(gps.location_valid(value['node']['location']))
        self.assertEqual(value['sensors'], self.original['sensors'])
        self.assertNotEqual(before, self.config.read_bytes())
        # A separate opener proves TIOCEXCL was released before engine startup.
        fd = os.open(os.ttyname(slave), os.O_RDONLY | os.O_NONBLOCK); os.close(fd)
        # Restore fixture path and verify prepare consumes the SAME identity.
        self.world.state['qualified']['path'] = str(self.world.dev/'ttyUSB7')
        with patch.object(runtime, 'confirm', side_effect=AssertionError('qualified GPS reprobed')):
            selected = runtime.discover(self.world.state, 101)
        self.assertEqual(selected, current)
        with patch.object(runtime, 'ROOT', self.root/'transport'), \
             patch.object(runtime.os, 'mknod') as mknod, patch.object(runtime.os, 'chown'), \
             patch.object(runtime.os, 'chmod'), patch.object(runtime.os, 'replace'):
            runtime.prepare(self.world.state, selected)
        self.assertEqual(mknod.call_args.args[2], current[2])
        self.assertEqual(self.world.state['mapped'], current)

    def test_stale_or_unqualified_attachment_never_opens(self):
        _, slave, _ = self.qualify()
        valid = json.loads(self.state.read_text())
        for bad in ({}, {'qualified': dict(valid['qualified'], usb_id='0403:6001')},
                    {'qualified': dict(valid['qualified'], identity=[0,0,0,0])}):
            self.state.write_text(json.dumps(bad))
            with patch.object(gps, 'open_gps', side_effect=FileNotFoundError) as opened:
                with self.assertRaises(FileNotFoundError): gps.open_selected_gps('/dev/cybrrd_gps', 9600)
                opened.assert_called_once_with('/dev/cybrrd_gps', 9600)

    def test_replacement_between_resolution_and_open_is_closed(self):
        _, slave, _ = self.qualify()
        master2, slave2 = prior.pty.openpty(); self.world.fds.extend((master2, slave2))
        fd = os.open(os.ttyname(slave2), os.O_RDONLY | os.O_NONBLOCK)
        def open_fixture(path, baud):
            if path == '/dev/cybrrd_gps': raise FileNotFoundError()
            return fd
        with patch.object(gps, 'open_gps', side_effect=open_fixture), patch.object(gps, 'close_gps', wraps=gps.close_gps) as close:
            with self.assertRaises(FileNotFoundError): gps.open_selected_gps('/dev/cybrrd_gps',9600)
            close.assert_called_once_with(fd)

    def test_native_and_custom_paths_keep_existing_open_contract(self):
        with patch.object(gps, 'open_gps', return_value=77) as opened:
            self.assertEqual(gps.open_selected_gps('/dev/cybrrd_gps',9600), (77,'/dev/cybrrd_gps'))
            opened.assert_called_once_with('/dev/cybrrd_gps',9600)
        with patch.object(gps, 'open_gps', side_effect=FileNotFoundError) as opened:
            with self.assertRaises(FileNotFoundError): gps.open_selected_gps('/dev/custom-gps',9600)
            opened.assert_called_once_with('/dev/custom-gps',9600)

if __name__ == '__main__': unittest.main(verbosity=2)
