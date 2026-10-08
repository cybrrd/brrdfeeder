#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Behavioral tests for the non-u-blox GPS opt-in (Adafruit Ultimate GPS #746).

Covers the three Synth amendments: headless --gps-usb-id flag discipline is
covered by bash-level checks here (validation, default-decline without tty);
the passive NMEA confirm (never writes, >=2 valid $GP/$GN sentences required);
and pre-existing-rule reporting (never deleted/overwritten). The udev render
assertions cover declaration-present vs absent.
"""
import re
import subprocess
import sys
import unittest
from pathlib import Path

INSTALL = Path(__file__).resolve().parents[2]
SCRIPT = INSTALL/'brrdfeeder-install.sh'

NMEA_CONFIRM_SRC = re.search(
    r"gps_nmea_confirm\(\) \{\n(.*?)\n\}\n\ngps_candidate_tty", SCRIPT.read_text(), re.S).group(1)
PROBE_SRC = re.search(r"<<'GPS_PROBE_EOF'\n(.*?)\nGPS_PROBE_EOF", NMEA_CONFIRM_SRC, re.S).group(1)


def run_probe(dev):
    return subprocess.run([sys.executable, '-c', PROBE_SRC, dev],
                          capture_output=True, text=True, timeout=20).stdout.strip()


class NmeaConfirm(unittest.TestCase):
    def test_no_device_reports_no_nmea(self):
        self.assertEqual(run_probe('/dev/nonexistent-gps-probe'), 'no-nmea')

    def test_silent_port_reports_no_nmea(self):
        import os
        r, w = os.pipe()
        # A pipe with no data and no NMEA: 5s read -> no-nmea.
        self.assertEqual(run_probe(f'/dev/fd/{r}'), 'no-nmea')
        os.close(r); os.close(w)

    def test_valid_stream_confirms(self):
        import pty, os
        master, slave = pty.openpty()
        good = (
            b'$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n'
            b'$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A\r\n'
        )
        import threading, time
        def feed():
            time.sleep(0.3)
            os.write(master, good)
        threading.Thread(target=feed, daemon=True).start()
        self.assertEqual(run_probe(os.ttyname(slave)), 'ok')

    def test_garbage_stream_reports_no_nmea(self):
        import pty, os, threading, time
        master, slave = pty.openpty()
        def feed():
            time.sleep(0.3)
            os.write(master, b'\x00\x01random bytes no dollar signs\r\n')
        threading.Thread(target=feed, daemon=True).start()
        # 5s window with garbage -> no-nmea
        self.assertEqual(run_probe(os.ttyname(slave)), 'no-nmea')


class ScriptContracts(unittest.TestCase):
    def setUp(self):
        self.text = SCRIPT.read_text()

    def test_flag_validated_as_vid_pid(self):
        self.assertIn('--gps-usb-id)', self.text)
        self.assertIn(
            "[[ $2 =~ ^[0-9A-Fa-f]{4}:[0-9A-Fa-f]{4}$ ]] || {", self.text)

    def test_probe_never_writes(self):
        # The probe opens O_RDONLY and only termios-tunes input modes.
        self.assertIn('os.O_RDONLY', PROBE_SRC)
        self.assertNotIn('os.write', PROBE_SRC)

    def test_declared_rule_rendered_only_after_confirm(self):
        # The render must be gated on BOTH the declaration and the NMEA
        # confirm outcome (a mutation that drops the confirm gate makes the
        # udev render fire for any declared id -> this fails).
        self.assertIn(
            'if [[ -n $gps_declared_usb_id && $GPS_USB_ID_PRESENT -eq 1 ]]; then\n  GPS_DECLARED_RULE=',
            self.text)
        self.assertIn('CYBRRD_GPS_DECLARED', self.text)

    def test_preexisting_rule_reported_not_removed(self):
        self.assertIn('Left untouched', self.text)
        self.assertIn('never delete files we did not create', self.text)

    def test_prompt_default_no_without_tty(self):
        self.assertIn('declined by default', self.text)
        self.assertIn('-y', self.text)
        # -y must not feed the GPS decision: the prompt is inside
        # gps_usb_preflight gated by GPS_PROMPT_NEEDED, independent of
        # ASSUME_YES.
        self.assertNotIn('ASSUME_YES -eq 1 ]] && [[ $GPS_PROMPT_NEEDED', self.text)

    def test_interim_rule_path_named(self):
        self.assertIn('97-cybrrd-gps-local-adafruit.rules', self.text)


if __name__ == '__main__':
    unittest.main(verbosity=2)
