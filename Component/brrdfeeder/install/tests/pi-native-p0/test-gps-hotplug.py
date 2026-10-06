#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""No USB hardware: generated udev rules, sysfs fixtures and real PTY lifecycle."""
import contextlib
import fnmatch
import importlib.util
import io
import json
import os
from pathlib import Path
import pty
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import yaml

ROOT = Path(__file__).resolve().parents[5]
INSTALL = ROOT/'Component/brrdfeeder/install'
SOURCE = (INSTALL/'brrdfeeder-install.sh').read_text()
spec = importlib.util.spec_from_file_location('gps', INSTALL/'gps-seed.py')
gps = importlib.util.module_from_spec(spec); spec.loader.exec_module(gps)
CONSTANTS = SOURCE.split('# Default USB vendor:product IDs', 1)[1].split('# Canonical host config tree', 1)[0]
CONSTANTS = CONSTANTS[CONSTANTS.index('\n'):]


def sentence(body):
    check = 0
    for byte in body.encode(): check ^= byte
    return f'${body}*{check:02X}\r\n'.encode()


GOOD = sentence('GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,')


class Contract(unittest.TestCase):
    def test_missing_then_plugged_midwait_then_first_fix(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); config=root/'config.yaml'; startup=root/'startup.json'; device=root/'gps'
            original={'node': {}, 'sensors': {'gps': {'device': '/dev/cybrrd_gps'}}}
            raw=yaml.safe_dump(original).encode(); config.write_bytes(raw); info=config.stat()
            real_open, real_report, real_matches, real_read = gps.open_gps, gps.report, gps.device_matches, os.read
            reports=[]; masters=[]; clock=[100.0]; waiting=[0]
            def report(state, message, *observations):
                real_report(state,message,*observations)
                reports.append(json.loads(startup.read_text()))
                if state=='gps-missing':
                    master,slave=pty.openpty(); masters.append(master)
                    device.symlink_to(os.ttyname(slave)); os.close(slave)
                elif state=='gps-waiting':
                    waiting[0]+=1
                    os.write(masters[0], sentence('GPGGA,123519,4807.038,N,01131.000,E,0,03,4.2,545.4,M,46.9,M,,') if waiting[0]==1 else GOOD)
            def read(fd,count):
                data=real_read(fd,count); clock[0]+=6; return data
            try:
                with patch.object(gps,'CONFIG',config), patch.object(gps,'STARTUP',startup), \
                     patch.object(gps,'prestart_only'), patch.object(gps,'held_elsewhere',return_value=False), \
                     patch.object(gps,'load_config',return_value=(original,raw,info)), \
                     patch.object(gps,'open_gps',side_effect=lambda _,baud: real_open(str(device),baud)), \
                     patch.object(gps,'device_matches',side_effect=lambda fd,_: real_matches(fd,str(device))), \
                     patch.object(gps,'report',side_effect=report), patch.object(gps,'adapter_ids',return_value=['10c4:ea60']), \
                     patch.object(gps.time,'sleep'), patch.object(gps.time,'monotonic',side_effect=lambda:clock[0]), \
                     patch.object(gps.os,'read',side_effect=read), contextlib.redirect_stdout(io.StringIO()):
                    gps.seed()
            finally:
                for fd in masters: os.close(fd)
            self.assertEqual(reports[0]['state'],'gps-missing')
            self.assertEqual(reports[0]['usb_adapter_ids'],['10c4:ea60'])
            self.assertTrue(any(r['state']=='gps-waiting' and r['gps']['satellites_used']==3 and r['gps']['hdop']==4.2 for r in reports))
            self.assertEqual(reports[-1]['state'],'gps-fix')
            self.assertTrue(gps.location_valid(yaml.safe_load(config.read_text())['node']['location']))
            self.assertFalse(startup.exists())

    def rules(self):
        block = SOURCE.split('# Verbatim aviary hardened block.', 1)[1].split('\n)\n', 1)[0]+'\n)\n'
        block = block[block.index('\n'):]
        command = 'set -eu\nGPS_SYMLINK=cybrrd_gps; BLE_SYMLINK=cybrrd_ble\n'+CONSTANTS+block+'\nprintf "%s\\n" "$NEW_UDEV_CONTENT"\n'
        result = subprocess.run(['bash', '-c', command], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def test_five_supported_ids_and_negative_matches(self):
        rules = self.rules()
        entries = [line for line in rules.splitlines() if 'SYMLINK+="cybrrd_gps"' in line]
        def matches(vendor, product):
            return any(fnmatch.fnmatchcase(vendor, re.search(r'ATTRS\{idVendor\}=="([^"]+)"', line)[1]) and
                       fnmatch.fnmatchcase(product, re.search(r'ATTRS\{idProduct\}=="([^"]+)"', line)[1]) for line in entries)
        for product in ['01a5', '01a6', '01a7', '01a8', '01a9']:
            with self.subTest(product=product): self.assertTrue(matches('1546', product))
        for vendor, product in [('1546', '01a4'), ('1546', '01aa'), ('1546', 'ffff'), ('1234', '01a8'), ('1a86', '7523')]:
            self.assertFalse(matches(vendor, product))
        for line in entries:
            for invariant in ['SUBSYSTEM=="tty"', 'GROUP="dialout"', 'MODE="0660"']:
                self.assertIn(invariant, line)
        if os.environ.get('P0_EVIDENCE'):
            (Path(os.environ['P0_EVIDENCE'])/'gps-udev.rules').write_text(rules)
        with tempfile.TemporaryDirectory() as d:
            path = Path(d)/'99-fixture.rules'; path.write_text(rules)
            p = subprocess.run(['udevadm', 'verify', str(path)], capture_output=True, text=True)
            self.assertEqual(p.returncode, 0, p.stdout+p.stderr)

    def inventory(self, usb_id):
        block = SOURCE.split('# Verify USB hardware visible', 1)[1].split('# Capture interface declared', 1)[0]
        block = block[block.index('\n'):]
        with tempfile.TemporaryDirectory() as d:
            sysfs = Path(d)/'sys'
            device = sysfs/'devices/usb1/1-1'; (device/'interface').mkdir(parents=True)
            (device/'idVendor').write_text(usb_id.split(':')[0]+'\n')
            (device/'idProduct').write_text(usb_id.split(':')[1]+'\n')
            tty = sysfs/'class/tty/ttyACM0'; tty.mkdir(parents=True)
            (tty/'device').symlink_to(device/'interface')
            script = 'set -eu\n'+CONSTANTS+f'GPS_SYMLINK=cybrrd_gps; FIXTURE_ID={usb_id}\n'+'''
say() { echo "$*"; }
ok() { echo "$*"; }
warn() { echo "$*"; }
lsusb() {
  if [[ $* == '-d 1546:' && $FIXTURE_ID == 1546:* || $* == "-d $FIXTURE_ID" ]]; then
    echo "Bus 001 Device 002: ID $FIXTURE_ID fixture receiver"
  else return 1; fi
}
'''+block.replace('/sys/', str(sysfs)+'/')+'\necho supported=$HAVE_UBLOX\n'
            result = subprocess.run(['bash', '-c', script], text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            return result.stdout

    def test_preflight_supported_family(self):
        for product in ['01a5', '01a6', '01a7', '01a8', '01a9']:
            with self.subTest(product=product):
                output = self.inventory('1546:'+product)
                self.assertIn('supported=1', output)
                self.assertIn('1546:'+product, output)

    def test_preflight_names_unknown_and_clone_without_mapping(self):
        samples = []
        for usb_id in ['1546:01aa', '1a86:7523', '10c4:ea60', '0403:6001']:
            with self.subTest(usb_id=usb_id):
                output = self.inventory(usb_id)
                self.assertIn('supported=0', output)
                self.assertIn(usb_id, output)
                self.assertIn('not a supported GPS', output)
                samples.append(output)
        if os.environ.get('P0_EVIDENCE'):
            (Path(os.environ['P0_EVIDENCE'])/'gps-unsupported-inventory.log').write_text('\n'.join(samples))

    def test_real_pty_unplug_replug_recovers_new_fix(self):
        with tempfile.TemporaryDirectory() as d:
            config = Path(d)/'config.yaml'; startup = Path(d)/'startup.json'; link = Path(d)/'gps'
            original = {'node': {'id': 'kept'}, 'sensors': {'gps': {'device': '/dev/cybrrd_gps'}}}
            config.write_text(yaml.safe_dump(original)); config.chmod(0o640)
            info = config.stat(); raw = config.read_bytes()
            master, slave = pty.openpty(); target = os.ttyname(slave); os.close(slave)
            link.symlink_to(target)
            state = {'master': master, 'guard': (), 'opens': [], 'closes': [], 'reports': [], 'disconnected': False, 'replugged': False}
            real_open = gps.open_gps
            real_close = gps.close_gps
            real_read = gps.os.read
            real_report = gps.report
            journal = io.StringIO()
            cut = len(GOOD)//2
            def read_fixture(fd, count):
                data = real_read(fd, count)
                if not state['disconnected']:
                    # Disconnect after receipt of old diagnostics + a partial fix.
                    os.close(state['master']); state['master'] = None
                    link.unlink(); state['disconnected'] = True
                return data
            def open_fixture(_device, baud):
                fd = real_open(str(link), baud); state['opens'].append(fd); return fd
            def close_fixture(fd):
                real_close(fd)
                with self.assertRaises(OSError): os.fstat(fd)
                state['closes'].append(fd)
            def report(status, message, *diagnostics):
                state['reports'].append((status, message))
                real_report(status, message, *diagnostics)
                if status == 'gps-waiting' and not state['disconnected']:
                    os.write(state['master'], sentence('GPGSV,1,1,01,01,10,100,20')+GOOD[:cut])
                elif status == 'gps-missing' and not state['replugged']:
                    # Occupy the old PTY number so the replacement uses a NEW path.
                    state['guard'] = pty.openpty()
                    master, slave = pty.openpty()
                    self.assertNotEqual(os.ttyname(slave), target)
                    link.symlink_to(os.ttyname(slave)); os.close(slave)
                    state['master'] = master; state['replugged'] = True
                elif status == 'gps-waiting' and state['replugged']:
                    # The old tail must not combine with a previous receiver's prefix.
                    if diagnostics:
                        self.assertTrue(all(v is None for v in diagnostics[0].values()))
                    os.write(state['master'], GOOD[cut:]+sentence('GPGGA,123519,4907.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,'))
            try:
                with contextlib.ExitStack() as stack:
                    stack.enter_context(contextlib.redirect_stdout(journal))
                    stack.enter_context(patch.dict(os.environ, {'NOTIFY_SOCKET': ''}))
                    for name, value in [('prestart_only', lambda: None), ('held_elsewhere', lambda _: False),
                                        ('open_gps', open_fixture), ('close_gps', close_fixture), ('report', report)]:
                        stack.enter_context(patch.object(gps, name, side_effect=value))
                    stack.enter_context(patch.object(gps, 'CONFIG', config))
                    stack.enter_context(patch.object(gps, 'STARTUP', startup))
                    stack.enter_context(patch.object(gps, 'load_config', return_value=(original, raw, info)))
                    stack.enter_context(patch.object(gps.time, 'sleep'))
                    stack.enter_context(patch.object(gps.os, 'read', side_effect=read_fixture))
                    # Force the real read/EOF path, independently of the new path-identity guard.
                    if hasattr(gps, 'device_matches'):
                        stack.enter_context(patch.object(gps, 'device_matches', return_value=True))
                    gps.seed()
                self.assertEqual(len(state['opens']), 2)
                self.assertTrue(state['replugged'])
                self.assertTrue(gps.location_valid(yaml.safe_load(config.read_bytes())['node']['location']))
                self.assertAlmostEqual(yaml.safe_load(config.read_bytes())['node']['location']['latitude'], 49.1173)
                self.assertEqual(state['closes'], state['opens'])
                self.assertTrue(any('disconnected' in message.lower() for _, message in state['reports']))
                if os.environ.get('P0_EVIDENCE'):
                    (Path(os.environ['P0_EVIDENCE'])/'gps-hotplug-journal.log').write_text(journal.getvalue())
            finally:
                if state['master'] is not None: os.close(state['master'])
                for fd in state['guard']: os.close(fd)

    def test_device_identity_detects_symlink_replacement(self):
        self.assertTrue(hasattr(gps, 'device_matches'), 'no open-device identity check')
        with tempfile.TemporaryDirectory() as d:
            link = Path(d)/'gps'; m1, s1 = pty.openpty(); m2, s2 = pty.openpty()
            try:
                link.symlink_to(os.ttyname(s1))
                self.assertTrue(gps.device_matches(s1, str(link)))
                link.unlink(); link.symlink_to(os.ttyname(s2))
                self.assertFalse(gps.device_matches(s1, str(link)))
                link.unlink()
                self.assertFalse(gps.device_matches(s1, str(link)))
            finally:
                for fd in [m1, s1, m2, s2]: os.close(fd)

    def retry_fixture(self, kind):
        class EndFixture(Exception): pass
        clock = [10.0]
        def select_once(*_):
            if kind == 'error': raise OSError('SEEDED_PRIVATE_DEVICE_ERROR')
            if clock[0] > 10: raise EndFixture()
            clock[0] += 16
            return [], [], []
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(gps, 'prestart_only'))
            stack.enter_context(patch.object(gps, 'load_config', return_value=({'node': {}}, b'', None)))
            opened = stack.enter_context(patch.object(gps, 'open_gps', side_effect=[77, EndFixture()]))
            closed = stack.enter_context(patch.object(gps, 'close_gps'))
            reports = stack.enter_context(patch.object(gps, 'report'))
            stack.enter_context(patch.object(gps.select, 'select', side_effect=select_once))
            stack.enter_context(patch.object(gps.time, 'sleep'))
            stack.enter_context(patch.object(gps.time, 'monotonic', side_effect=lambda: clock[0]))
            if hasattr(gps, 'device_matches'):
                stack.enter_context(patch.object(gps, 'device_matches', return_value=(kind != 'replacement')))
            with self.assertRaises(EndFixture): gps.seed()
            self.assertEqual(opened.call_count, 2, 'did not retry with a new descriptor')
            closed.assert_called_once_with(77)
            messages = '\n'.join(call.args[1] for call in reports.call_args_list)
            self.assertNotIn('SEEDED_PRIVATE_DEVICE_ERROR', messages)
            return messages

    def test_serial_error_reports_disconnect_and_retries(self):
        self.assertIn('disconnected', self.retry_fixture('error').lower())

    def test_silent_descriptor_has_bounded_retry(self):
        self.assertIn('silent', self.retry_fixture('silent').lower())

    def test_replaced_path_does_not_wait_on_old_descriptor(self):
        self.assertIn('replaced', self.retry_fixture('replacement').lower())

    def test_replacement_during_read_cannot_seed_old_fix(self):
        class EndFixture(Exception): pass
        with patch.object(gps, 'prestart_only'), \
             patch.object(gps, 'load_config', return_value=({'node': {}}, b'', None)), \
             patch.object(gps, 'open_gps', side_effect=[77, EndFixture()]), \
             patch.object(gps, 'close_gps'), patch.object(gps, 'report'), \
             patch.object(gps, 'device_matches', side_effect=[True, False, False]), \
             patch.object(gps.select, 'select', return_value=([77], [], [])), \
             patch.object(gps.os, 'read', return_value=GOOD), \
             patch.object(gps.time, 'sleep'), patch.object(gps, 'atomic_write') as writer:
            with self.assertRaises(EndFixture): gps.seed()
            writer.assert_not_called()


if __name__ == '__main__': unittest.main(verbosity=2)
