#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline edge cases: no kernel fault injection and no host installation."""
import contextlib
import datetime as dt
import importlib.util
import io
import json
import os
import re
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[5]
INSTALL = ROOT / 'Component/brrdfeeder/install'
SOURCE = (INSTALL / 'brrdfeeder-install.sh').read_text()
spec = importlib.util.spec_from_file_location('logger', INSTALL / 'install-log.py')
log = importlib.util.module_from_spec(spec)
spec.loader.exec_module(log)


class Firmware(unittest.TestCase):
    def test_status_and_bundle_continue_after_hung_firmware(self):
        for mode in ['--status', '--support-bundle']:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                d = Path(directory)
                stub = d/'vcgencmd'; stub.write_text('#!/bin/sh\nexec sleep 30\n'); stub.chmod(0o755)
                child = d/'fixture.sh'; child.write_text('echo STATUS-CONTINUED\n')
                real_capture = log.capture
                calls = []
                def capture(args, **kw):
                    if args[0] == 'vcgencmd':
                        calls.append(args)
                        return real_capture(args, **kw)
                    return 127, 'isolated fixture', 0
                output = io.StringIO()
                started = time.monotonic()
                with patch.dict(os.environ, PATH=directory+':'+os.environ['PATH']), \
                     patch.object(log, 'LOG_DIR', d/'logs'), patch.object(log, 'capture', side_effect=capture), \
                     patch.object(log, 'read_text', side_effect=FileNotFoundError), \
                     patch.object(log.pwd, 'getpwnam', side_effect=KeyError), contextlib.redirect_stdout(output):
                    rc = log.supervise(str(child), [mode])
                self.assertEqual(rc, 0)
                self.assertLess(time.monotonic()-started, 7)
                self.assertEqual(len(calls), 1)
                text = output.getvalue()
                self.assertIn('WARNING: power check: unknown (firmware did not answer)', text)
                if mode == '--status': self.assertIn('STATUS-CONTINUED', text)
                else:
                    archive = re.search(r'Support bundle: (/tmp/\S+\.tar\.gz)', text)
                    self.assertIsNotNone(archive)
                    Path(archive[1]).unlink()  # only this fixture's new archive

    def test_unkillable_child_and_open_pipe_do_not_hold_caller(self):
        real_popen, real_kill = subprocess.Popen, os.killpg
        children = []
        def launch(*a, **kw):
            self.assertTrue(kw['start_new_session'])
            self.assertEqual(kw['stdin'], subprocess.DEVNULL)
            p = real_popen(*a, **kw)
            children.append(p)
            return p
        try:
            # Deliberately ineffective SIGKILL models the property of a D-state
            # process we need: it remains alive with its stdout pipe still open.
            with patch.object(log.subprocess, 'Popen', side_effect=launch), \
                 patch.object(log.os, 'killpg', return_value=None):
                rc, out, elapsed = log.capture(['sleep', '30'], timeout=.15)
            self.assertEqual(rc, 124)
            self.assertLess(elapsed, .7)
            self.assertIsNone(children[0].poll())
            self.assertTrue(children[0].stdout.closed)
            self.assertIn('[timeout]', out)
        finally:
            for p in children:
                real_kill(p.pid, signal.SIGKILL)
                p.wait(timeout=2)  # fixture cleanup ONLY; not production capture

    def test_closed_pipe_but_live_child_has_bounded_wait_too(self):
        rc, _, elapsed = log.capture(['bash', '-c', 'exec 1>&- 2>&-; sleep 30'], timeout=.15)
        self.assertEqual(rc, 124)
        self.assertLess(elapsed, .7)

    def test_power_results_are_validated_and_cached(self):
        cases = [(0, 'throttled=0x0\n', 'observed'), (0, 'throttled=0x10001', 'observed'),
                 (124, '', 'unknown'), (127, '', 'unavailable'), (1, '', 'unknown'),
                 (0, 'throttled=0x0\nmalformed', 'unknown'), (0, '', 'unknown')]
        for rc, out, expected in cases:
            with self.subTest(rc=rc, out=out), patch.object(log, 'capture', return_value=(rc, out, 0)) as call:
                log.power_check.cache_clear()
                self.assertEqual(log.power_check()[0], expected)
                log.power_check()
                call.assert_called_once_with(['vcgencmd', 'get_throttled'], timeout=5, limit=256)
        log.power_check.cache_clear()

    def test_preserved_power_warning_bits(self):
        block = SOURCE.split('# --- Power-supply sanity', 1)[1].split('# --- Legacy migration', 1)[0]
        block = '# --- Power-supply sanity'+block
        for value, expected in [('0x0', 'power supply OK'), ('0x1', 'UNDERVOLTAGE'),
                                ('0x10000', 'UNDERVOLTAGE'), ('0x4', 'Power/temperature limits')]:
            p = subprocess.run(['bash', '-c', 'warn() { echo "$*"; }; ok() { echo "$*"; };\n'+block],
                               env=dict(os.environ, BRRDFEEDER_POWER_STATE='observed', BRRDFEEDER_POWER_VALUE=value),
                               capture_output=True, text=True, timeout=2)
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertIn(expected, p.stdout)

    def test_board_probe_hang_returns_warning(self):
        source = (ROOT / 'Component/brrdfeeder/tools/board-discovery.sh').read_text()
        code = source.split("<<'POWER_PY'\n", 1)[1].split('\nPOWER_PY', 1)[0]
        with tempfile.TemporaryDirectory() as directory:
            stub = Path(directory) / 'vcgencmd'
            stub.write_text('#!/bin/sh\nexec sleep 30\n'); stub.chmod(0o755)
            started = time.monotonic()
            p = subprocess.run(['python3', '-c', code], capture_output=True, text=True, timeout=7,
                               env=dict(os.environ, PATH=directory+':'+os.environ['PATH']))
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertLess(time.monotonic()-started, 6)
            self.assertIn('unknown (firmware did not answer)', p.stdout)


class Readiness(unittest.TestCase):
    def snapshot(self, *, age=0, node='fixture', state='active', substate='running',
                 journal='[rid_ble] state=Healthy detail=fixture', changed=False,
                 sidecar_state='gps-waiting', sidecar_age=0):
        now = time.time()
        context = dict(node_id='fixture', started_at=int(now-10))
        status = dict(schema_version=1, written_at=dt.datetime.fromtimestamp(now-age, dt.timezone.utc).isoformat(),
                      status_interval_secs=30, heartbeat=dict(node_id=node, gps=dict(state='healthy'), radio_status='up'),
                      links=dict(nats_state='connected'), inventory=dict(capture=[dict(monitor_mode=True)]))
        sidecar = dict(state=sidecar_state, written_at=dt.datetime.fromtimestamp(now-sidecar_age, dt.timezone.utc).isoformat())
        calls = []
        def capture(args, **kw):
            self.assertGreater(kw['timeout'], 0)
            self.assertLessEqual(kw['timeout'], 2)
            calls.append(args)
            if args[0] == 'journalctl':
                self.assertEqual(args[-1], '_SYSTEMD_INVOCATION_ID='+'a'*32)
                return 0, journal, 0
            invocation = ('b' if changed and len(calls)>1 else 'a')*32
            return 0, f'ActiveState={state}\nSubState={substate}\nInvocationID={invocation}\n', 0
        with patch.object(log, 'capture', side_effect=capture), patch.object(log, 'read_text', side_effect=lambda p: json.dumps(sidecar if 'startup' in p else status)):
            return log.engine_snapshot(context, time.monotonic()+3)

    def test_ready_current_invocation(self):
        self.assertTrue(all(v == 'OK' for v in self.snapshot()['states'].values()))

    def test_old_future_wrong_node_and_restart_not_healthy(self):
        for kw in [dict(age=120), dict(age=20), dict(age=-10), dict(node='other'), dict(changed=True)]:
            with self.subTest(kw=kw):
                self.assertTrue(all(v != 'OK' for v in self.snapshot(**kw)['states'].values()))

    def test_latest_ble_failure_overrides_old_healthy(self):
        result = self.snapshot(journal='[rid_ble] state=Healthy\n[rid_ble] state=Failed detail=blocked')
        self.assertEqual(result['states']['Bluetooth'], 'needs attention')

    def test_gps_waiting_requires_fresh_valid_sidecar(self):
        for kw, valid in [({}, True), (dict(sidecar_age=120), False), (dict(sidecar_age=-20), False),
                          (dict(sidecar_state='not-gps'), False)]:
            self.assertEqual(self.snapshot(state='activating', substate='start-pre', **kw)['gps_waiting'], valid)
        self.assertIsNotNone(self.snapshot(state='failed')['failure'])


class Ownership(unittest.TestCase):
    def test_clock_migration_owned_foreign_symlink_and_dryrun(self):
        block = 'gate clock '+SOURCE.split('gate clock ', 1)[1].split('# Step 3 — udev rules', 1)[0]
        for kind in ['owned', 'foreign', 'symlink', 'dryrun', 'new-foreign']:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                d = Path(directory); old = d/'legacy'; new = d/'current'
                if kind == 'symlink': old.symlink_to(d/'elsewhere')
                else: old.write_text('# Pack-canonical chrony override\nmakestep 1.0 -1\n' if kind != 'foreign' else 'foreign\n')
                if kind == 'new-foreign': new.write_text('foreign\n')
                before = old.read_text() if old.is_file() else None
                script = '''set -eu
gate() { :; }; ok() { :; }; say() { :; }
fatal() { echo "$*"; exit 1; }
atomic_install() { cat > "$4"; }
# Parent modes have a separate real-filesystem fixture; never touch /etc here.
public_directories() { :; }
run() { if [[ $1 == systemctl || $1 == install ]]; then :; else "$@"; fi; }
stat() { echo 0; }
'''+block
                p = subprocess.run(['bash', '-c', script], capture_output=True, text=True, timeout=3,
                                   env=dict(os.environ, CHRONY_DROPIN=str(new), LEGACY_CHRONY_DROPIN=str(old), DRY_RUN='1' if kind=='dryrun' else '0'))
                if kind == 'owned':
                    self.assertEqual(p.returncode, 0, p.stderr)
                    self.assertFalse(old.exists())
                    self.assertIn('makestep 1.0 -1', new.read_text())
                    self.assertNotIn('Pack', new.read_text())
                    self.assertNotIn('P' + 'ack-canonical', p.stdout + p.stderr)
                else:
                    self.assertEqual(p.returncode, 0 if kind=='dryrun' else 1, p.stderr)
                    self.assertTrue(old.exists() or old.is_symlink())
                    if before is not None: self.assertEqual(old.read_text(), before)
                    if kind != 'new-foreign': self.assertFalse(new.exists())

    def test_updater_legacy_and_current_markers_only(self):
        source = (INSTALL/'uninstall.sh').read_text()
        # Execute the actual check expression with each literal registered pair.
        self.assertIn('${signature%%|*}', source)
        self.assertIn('${signature#*|}', source)
        for marker in ['installer', 'watcher', 'service']:
            signature = '# BRRDfeeder signed-update '+marker+'.|#185 Drop 2'
            for content, expected in [(signature.split('|')[0], 0), ('#185 Drop 2', 0), ('foreign', 1)]:
                with tempfile.NamedTemporaryFile(mode='w') as f:
                    f.write(content); f.flush()
                    p = subprocess.run(['bash', '-c', 'grep -qF "${signature%%|*}" "$path" || grep -qF "${signature#*|}" "$path"'],
                                       env=dict(os.environ, signature=signature, path=f.name), timeout=2)
                    self.assertEqual(p.returncode, expected)


if __name__ == '__main__': unittest.main(verbosity=2)
