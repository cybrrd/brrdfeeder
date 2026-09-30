#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[5]
spec = importlib.util.spec_from_file_location('gps', ROOT/'Component/brrdfeeder/install/gps-seed.py')
gps = importlib.util.module_from_spec(spec); spec.loader.exec_module(gps)


def sentence(body):
    checksum = 0
    for byte in body.encode(): checksum ^= byte
    return f'${body}*{checksum:02X}\r\n'.encode()


GGA = 'GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,'
GSA = 'GPGSA,A,3,04,05,09,12,24,25,29,31,,,,,1.8,1.0,1.5'


class Diagnostics(unittest.TestCase):
    def make(self):
        self.assertTrue(hasattr(gps, 'NMEADiagnostics'), 'GPS diagnostics not implemented')
        return gps.NMEADiagnostics()

    def test_silent_then_no_lock_then_measured_fix(self):
        d = self.make()
        self.assertTrue(all(v is None for v in d.snapshot(10).values()))
        d.observe(sentence(GGA.replace(',1,08,', ',0,00,')), 10)
        d.observe(sentence(GSA.replace(',A,3,', ',A,1,')), 10)
        sample = d.snapshot(12)
        self.assertEqual((sample['satellites_used'], sample['fix_quality'], sample['fix_mode'], sample['nmea_age_secs']), (0, 0, 1, 2))
        d.observe(sentence(GGA), 12)
        d.observe(sentence(GSA), 12)
        sample = d.snapshot(12)
        self.assertEqual((sample['satellites_used'], sample['fix_quality'], sample['fix_mode']), (8, 1, 3))
        self.assertIsNotNone(gps.parse_fix(sentence(GGA)))

    def test_gsv_complete_cycles_no_double_counting(self):
        d = self.make()
        d.observe(sentence('GPGSV,2,1,05,01,10,100,20,02,20,110,30,03,30,120,,04,40,130,40'), 10)
        self.assertIsNone(d.snapshot(10)['satellites_in_view'])
        d.observe(sentence('GPGSV,2,2,05,05,50,140,50'), 11)
        sample = d.snapshot(11)
        self.assertEqual((sample['satellites_in_view'], sample['snr_max_dbhz'], sample['snr_avg_dbhz']), (5, 50, 35))
        d.observe(sentence('GLGSV,1,1,01,65,50,140,15'), 11)
        self.assertEqual(d.snapshot(11)['satellites_in_view'], 6)
        d.observe(sentence('GNGSV,1,1,02,01,50,140,10,65,50,140,20'), 12)
        sample = d.snapshot(12)
        self.assertEqual((sample['satellites_in_view'], sample['snr_avg_dbhz']), (2, 15))

    def test_expiry_invalid_checksum_and_numeric_ranges(self):
        d = self.make()
        for raw in [b'$junk\n', sentence(GGA)[:-4]+b'00\r\n', b'x'*5000]:
            d.observe(raw, 10)
        self.assertIsNone(d.snapshot(10)['nmea_age_secs'])
        d.observe(sentence(GGA), 10)
        d.observe(sentence('GPGSV,1,1,01,01,10,100,1000'), 10)
        d.observe(sentence(GSA.replace(',A,3,', ',A,9,')), 10)
        self.assertIsNone(d.snapshot(10)['snr_max_dbhz'])
        self.assertIsNone(d.snapshot(10)['fix_mode'])
        old = d.snapshot(26)
        self.assertEqual(old['nmea_age_secs'], 16)
        self.assertIsNone(old['satellites_used'])
        self.assertIsNone(old['fix_quality'])

    def test_partial_reordered_and_unbounded_talkers(self):
        d = self.make()
        d.observe(sentence('GPGSV,2,2,05,05,50,140,50'), 10)
        self.assertIsNone(d.snapshot(10)['satellites_in_view'])
        for a in 'ABCDEFGHIJKLMNOPQRSTUVWXYZ':
            for b in 'ABCDEFGHIJKLMNOPQRSTUVWXYZ':
                d.observe(sentence(a+b+'GSV,9,1,36,01,10,100,20,02,20,110,30,03,30,120,40,04,40,130,50'), 10)
        self.assertLessEqual(len(d.pending), 8)
        self.assertLessEqual(len(d.completed), 8)
        self.assertLess(len(json.dumps(d.snapshot(10))), 1024)

    def test_startup_record_and_journal_are_bounded_public_numeric(self):
        d = self.make(); d.observe(sentence(GGA), 10); d.observe(sentence(GSA), 10)
        with tempfile.TemporaryDirectory() as directory:
            out = io.StringIO(); path = Path(directory)/'startup.json'
            with patch.object(gps, 'STARTUP', path), contextlib.redirect_stdout(out), patch.dict(os.environ, {'NOTIFY_SOCKET': ''}):
                gps.report('gps-waiting', 'Waiting for GPS fix', d.snapshot(12))
            raw = path.read_bytes(); record = json.loads(raw)
            self.assertLess(len(raw), 4096)
            self.assertEqual(record['gps']['satellites_used'], 8)
            self.assertEqual(record['gps']['nmea_age_secs'], 2)
            self.assertEqual(record['status_interval_secs'], 5)
            self.assertEqual(path.stat().st_mode & 0o777, 0o644)
            for value in ['satellites_used=8', 'fix_quality=1', 'fix_mode=3', 'nmea_age_secs=2', 'snr_max_dbhz=unknown']:
                self.assertIn(value, out.getvalue())
            self.assertNotIn('4807', raw.decode())
            if os.environ.get('P0_EVIDENCE'):
                evidence = Path(os.environ['P0_EVIDENCE'])
                (evidence/'startup-sample.json').write_bytes(raw)
                (evidence/'gps-journal-sample.log').write_text(out.getvalue())

    def test_waiter_loop_reports_observations_without_seeding_no_fix(self):
        self.make()
        class EndFixture(Exception): pass
        clock = [10.0]
        chunks = [sentence(GGA.replace(',1,08,', ',0,00,'))+sentence(GSA.replace(',A,3,', ',A,1,'))]
        def read(_fd, _count):
            if not chunks: raise EndFixture()
            clock[0] += 6
            return chunks.pop()
        with patch.object(gps, 'prestart_only'), \
             patch.object(gps, 'load_config', return_value=({'node': {}}, b'original', None)), \
             patch.object(gps, 'open_gps', return_value=77), patch.object(gps, 'device_matches', return_value=True), patch.object(gps, 'close_gps'), \
             patch.object(gps.time, 'monotonic', side_effect=lambda: clock[0]), \
             patch.object(gps.select, 'select', return_value=([77], [], [])), \
             patch.object(gps.os, 'read', side_effect=read), \
             patch.object(gps, 'report') as report, patch.object(gps, 'atomic_write') as writer:
            with self.assertRaises(EndFixture): gps.seed()
            writer.assert_not_called()
            state, _, sample = report.call_args[0]
            self.assertEqual(state, 'gps-waiting')
            self.assertEqual((sample['satellites_used'], sample['fix_quality'], sample['fix_mode']), (0, 0, 1))


if __name__ == '__main__': unittest.main(verbosity=2)
