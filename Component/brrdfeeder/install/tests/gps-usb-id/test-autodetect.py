#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline GPS discovery: real shell blocks, sysfs fixtures, PTYs and udev predicates.

GPS_TEST_INSTALLER selects an old installer for revert controls; production code
has no fixture/environment bypass. No installed rules or physical tty is touched.
"""
import errno
import fnmatch
import os
from pathlib import Path
import pty
import re
import select
import subprocess
import sys
import tempfile
import termios
import time
import unittest

SCRIPT = Path(os.environ.get('GPS_TEST_INSTALLER', Path(__file__).resolve().parents[2]/'brrdfeeder-install.sh'))
SOURCE = SCRIPT.read_text()
BLOCK = SOURCE[SOURCE.index('HAVE_UBLOX=0; HAVE_NORDIC=0'):SOURCE.index('# Passive USB inventory only;')]
DEFS, STATE = BLOCK.split('gps_declared_usb_id=""', 1)
STATE = 'gps_declared_usb_id=""' + STATE
RULES = SOURCE[SOURCE.index('GPS_UDEV_RULES=$('):SOURCE.index('if [[ -f "$UDEV_RULES_FILE" ]] && diff')]
PROBE = SOURCE.split("<<'GPS_PROBE_EOF'\n", 1)[1].split('\nGPS_PROBE_EOF', 1)[0]
CONSTANTS = '''UBLOX_VENDOR=1546; UBLOX_PRODUCTS=(01a5 01a6 01a7 01a8 01a9)
GPS_SYMLINK=cybrrd_gps; BLE_SYMLINK=cybrrd_ble; NORDIC_VENDOR=1915; NORDIC_PRODUCT=c00a
say() { echo "$*"; }; ok() { echo "$*"; }; warn() { echo "$*"; }
lsusb() { return 1; }
'''


def event(rule, vendor='10c4', product='ea60', subsystem='tty'):
    """Evaluate every token of this deliberately small generated rule grammar.

    Match predicates are evaluated against an event with NO preseeded properties.
    Unknown keys/operators fail the test instead of silently weakening the model.
    Syntax is additionally checked with real udevadm verify.
    """
    environment, assignments = {}, {}
    fields = {'SUBSYSTEM': subsystem, 'ATTRS{idVendor}': vendor, 'ATTRS{idProduct}': product}
    for token in rule.split(', '):
        match = re.fullmatch(r'([A-Za-z0-9_{}]+)(==|\+=|=)"([^"]*)"', token)
        if not match:
            raise AssertionError('Unmodelled rule token: '+token)
        key, operator, value = match.groups()
        if operator == '==':
            if key.startswith('ENV{'):
                actual = environment.get(key[4:-1], '')
            else:
                actual = fields[key]
            if not fnmatch.fnmatchcase(actual, value):
                return {}
        elif key.startswith('ENV{') and operator == '=':
            environment[key[4:-1]] = value
        else:
            assert key in ('SYMLINK', 'GROUP', 'MODE'), key
            assignments[key] = value
    return assignments


class Discovery(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.sysfs = self.root/'sys'
        self.rules = self.root/'rules'; self.rules.mkdir()
        self.config = self.root/'config.yaml'; self.config.write_text('{}\n')

    def tty(self, name='ttyUSB0', usb='10c4:ea60', port='1-1.3'):
        device = self.sysfs/'devices/usb1'/port
        endpoint = device/(port+':1.0')/name; endpoint.mkdir(parents=True)
        (device/'idVendor').write_text(usb.split(':')[0]+'\n')
        (device/'idProduct').write_text(usb.split(':')[1]+'\n')
        klass = self.sysfs/'class/tty'/name; klass.mkdir(parents=True)
        (klass/'device').symlink_to(endpoint)

    def run_shell(self, outcome='ok', flag='', candidate=False, confirmed=None):
        defs = DEFS.replace('/sys/', str(self.sysfs)+'/')
        rules = RULES.replace('/etc/udev/rules.d/', str(self.rules)+'/')
        header = 'set -eu\n'+CONSTANTS+f'CONFIG_PATH={self.config}\nUDEV_RULES_FILE={self.rules}/99-cybrrd-brrdfeeder.rules\nGPS_USB_ID_FLAG={flag}\n'
        if candidate:
            body = defs+'\ngps_candidate_tty 10c4:ea60\n'
        else:
            body = defs+f'''\ngps_nmea_confirm() {{ echo "PROBED:$1" >&2; echo {outcome}; }}
'''+STATE+'\n'
            if confirmed is not None:
                body += f'gps_declared_usb_id=10c4:ea60; GPS_USB_ID_PRESENT={confirmed}\n'
            body += rules+'\nprintf "BEGIN_RULES\\n%s\\nEND_RULES\\n" "$NEW_UDEV_CONTENT"\n'
        result = subprocess.run(['bash', '-c', header+body], capture_output=True, text=True,
                                input='', timeout=8, start_new_session=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result

    def gps_rules(self, result):
        rendered = result.stdout.split('BEGIN_RULES\n')[1].split('END_RULES')[0]
        return [line for line in rendered.splitlines() if 'SYMLINK+="cybrrd_gps"' in line]

    def test_cp210x_sysfs_selects_tty_not_interface(self):
        self.tty()
        self.assertEqual(self.run_shell(candidate=True).stdout.strip(), '/dev/ttyUSB0')

    def test_automatic_confirm_maps_without_terminal(self):
        self.tty()
        result = self.run_shell()
        self.assertIn('PROBED:/dev/ttyUSB0', result.stderr)
        self.assertTrue(any(event(line).get('SYMLINK') == 'cybrrd_gps' for line in self.gps_rules(result)))
        self.assertNotIn('declined', result.stdout)

    def test_no_nmea_unmapped_with_recovery_message(self):
        self.tty()
        result = self.run_shell('no-nmea')
        self.assertFalse(any(event(line) for line in self.gps_rules(result)))
        for word in ('10c4:ea60', '/dev/ttyUSB0', 'wiring', 'antenna', 're-run'):
            self.assertIn(word, result.stdout)
        self.assertNotIn('not a supported GPS', result.stdout)

    def test_busy_or_unknown_unmapped_and_explained(self):
        self.tty()
        for outcome in ('busy', 'ownership-unknown'):
            with self.subTest(outcome=outcome):
                result = self.run_shell(outcome)
                self.assertFalse(any(event(line) for line in self.gps_rules(result)))
                self.assertIn('skipped NMEA read and tty configuration', result.stdout)
                self.assertIn(outcome, result.stdout)

    def test_ublox_prevents_automatic_probe_in_either_order(self):
        for names in [('ttyUSB0', 'ttyUSB1'), ('ttyUSB1', 'ttyUSB0')]:
            with self.subTest(names=names), tempfile.TemporaryDirectory() as d:
                self.sysfs = Path(d)/'sys'
                self.tty(names[0]); self.tty(names[1], '1546:01a7', '1-1.4')
                result = self.run_shell()
                self.assertNotIn('PROBED:', result.stderr)
                rules = self.gps_rules(result)
                self.assertEqual(len(rules), 5)
                self.assertTrue(any(event(line, '1546', '01a7') for line in rules))

    def test_native_ublox7_ttyacm0_never_enters_bridge_probe(self):
        # Field configuration: u-blox 7 on ttyACM0 must retain native mapping.
        self.tty('ttyACM0', '1546:01a7', '1-1.2')
        original = None
        for with_bridge in (False, True):
            with self.subTest(with_bridge=with_bridge):
                if with_bridge:
                    self.tty('ttyUSB0', '10c4:ea60', '1-1.3')
                result = self.run_shell()
                self.assertIn('Supported GPS USB ID 1546:01a7 detected', result.stdout)
                self.assertNotIn('PROBED:', result.stderr)
                rules = self.gps_rules(result)
                self.assertEqual(len(rules), 5)
                self.assertTrue(any(event(line, '1546', '01a7').get('SYMLINK') == 'cybrrd_gps'
                                    for line in rules))
                self.assertFalse(any(event(line) for line in rules))
                if original is not None:
                    self.assertEqual(rules, original)
                original = rules

    def test_explicit_other_id_still_requires_confirm(self):
        self.tty(usb='0403:6001')
        for outcome in ('ok', 'no-nmea'):
            with self.subTest(outcome=outcome):
                result = self.run_shell(outcome, flag='0403:6001')
                self.assertIn('PROBED:/dev/ttyUSB0', result.stderr)
                self.assertEqual(any(event(line, '0403', '6001') for line in self.gps_rules(result)), outcome == 'ok')
                self.assertNotIn('not a supported GPS', result.stdout)

    def test_saved_id_rerun_is_idempotent_without_prompt(self):
        self.tty(usb='0403:6001')
        self.config.write_text('sensors:\n  gps:\n    usb_id: "0403:6001"\n')
        first, second = self.run_shell(), self.run_shell()
        self.assertEqual(self.gps_rules(first), self.gps_rules(second))
        self.assertIn('PROBED:/dev/ttyUSB0', second.stderr)
        self.assertTrue(any(event(line, '0403', '6001') for line in self.gps_rules(second)))

    def test_busy_rerun_retains_only_exact_previously_confirmed_rule(self):
        self.tty()
        initial = self.gps_rules(self.run_shell())
        installed = self.rules/'99-cybrrd-brrdfeeder.rules'
        installed.write_text('\n'.join(initial)+'\n')
        self.config.write_text('sensors:\n  gps:\n    usb_id: "10c4:ea60"\n')
        result = self.run_shell('busy')
        self.assertEqual(self.gps_rules(result), initial)
        self.assertIn('Retaining the previously NMEA-confirmed mapping', result.stdout)
        # A failed fresh capability check must not inherit the old success.
        self.assertEqual(len(self.gps_rules(self.run_shell('no-nmea'))), 5)
        installed.write_text(installed.read_text().replace('ENV{CYBRRD_GPS_DECLARED}="1"',
                                                           'ENV{CYBRRD_GPS_DECLARED}=="1"'))
        self.assertEqual(len(self.gps_rules(self.run_shell('busy'))), 5)

    def test_declared_rule_fires_without_preseeded_environment(self):
        result = self.run_shell(confirmed=1)
        rules = self.gps_rules(result)
        line = next(line for line in rules if '10c4' in line)
        self.assertEqual(event(line), {'SYMLINK': 'cybrrd_gps', 'GROUP': 'dialout', 'MODE': '0660'})
        self.assertEqual(event(line, product='ea61'), {})
        self.assertEqual(event(line, subsystem='usb'), {})
        path = self.root/'99-test.rules'; path.write_text('\n'.join(rules)+'\n')
        result = subprocess.run(['udevadm', 'verify', str(path)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout+result.stderr)
        self.assertFalse(any(event(line) for line in self.gps_rules(self.run_shell(confirmed=0))))

    def test_no_interactive_reads_in_gps_path(self):
        self.assertNotIn('/dev/tty', BLOCK)
        self.assertNotIn('BRRDFEEDER_TTY_FD', BLOCK)
        self.assertNotRegex(BLOCK, r'read[^\n]*-t')
        self.assertNotIn('No terminal', BLOCK)


class Probe(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.proc = self.root/'proc'; self.proc.mkdir()
        self.master, slave = pty.openpty()
        self.addCleanup(os.close, self.master)
        self.dev = os.ttyname(slave)
        self.before = termios.tcgetattr(slave)
        os.close(slave)
        # Isolate ownership enumeration from the test runner's namespace. For
        # busy cases use a symlink to a real process's /proc/PID/fd directory.
        self.source = PROBE.replace("Path('/proc')", f"Path({str(self.proc)!r})")

    def start(self):
        return subprocess.Popen([sys.executable, '-c', self.source, self.dev],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    def finish(self, child, expected):
        out, err = child.communicate(timeout=8)
        self.assertEqual(child.returncode, 0, err)
        self.assertEqual(out.strip(), expected)
        self.assertEqual(termios.tcgetattr(self.master), self.before)

    def test_nmea_received_no_bytes_written_settings_restored(self):
        child = self.start()
        deadline = time.monotonic()+2
        while termios.tcgetattr(self.master) == self.before and time.monotonic() < deadline:
            time.sleep(.01)
        self.assertNotEqual(termios.tcgetattr(self.master), self.before)
        os.write(self.master, b'$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n'
                 b'$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A\r\n')
        self.finish(child, 'ok')
        if select.select([self.master], [], [], 0)[0]:
            try:
                self.assertEqual(os.read(self.master, 4096), b'', 'probe wrote or echoed bytes')
            except OSError as error:
                self.assertEqual(error.errno, errno.EIO)  # last slave fd closed

    def test_silent_timeout_restores_settings(self):
        started = time.monotonic()
        self.finish(self.start(), 'no-nmea')
        self.assertLess(time.monotonic()-started, 7)

    def test_busy_alias_is_not_opened_or_configured(self):
        alias = self.root/'gps'; alias.symlink_to(self.dev)
        held = os.open(alias, os.O_RDONLY | os.O_NONBLOCK)
        try:
            (self.proc/str(os.getpid())).symlink_to(Path('/proc')/str(os.getpid()))
            # Poison os.open in the probe: returning busy must happen before it.
            self.source = self.source.replace('print(confirm(sys.argv[1]))',
                "os.open = lambda *args: (_ for _ in ()).throw(AssertionError('busy tty opened'))\nprint(confirm(sys.argv[1]))")
            self.finish(self.start(), 'busy')
        finally:
            os.close(held)

    def test_incomplete_ownership_visibility_skips_open(self):
        self.source = self.source.replace('print(confirm(sys.argv[1]))',
            "Path.iterdir = lambda self: (_ for _ in ()).throw(PermissionError())\n"
            "os.open = lambda *args: (_ for _ in ()).throw(AssertionError('unknown owner tty opened'))\nprint(confirm(sys.argv[1]))")
        self.finish(self.start(), 'ownership-unknown')

    def test_read_failure_restores_settings(self):
        self.source = self.source.replace('print(confirm(sys.argv[1]))',
            "select.select = lambda *args: ([1], [], [])\n"
            "os.read = lambda *args: (_ for _ in ()).throw(OSError())\nprint(confirm(sys.argv[1]))")
        self.finish(self.start(), 'no-nmea')


if __name__ == '__main__':
    unittest.main(verbosity=2)
