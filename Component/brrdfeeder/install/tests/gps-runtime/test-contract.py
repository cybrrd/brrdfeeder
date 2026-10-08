#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
import importlib.util
import contextlib
import datetime
import io
import json
from pathlib import Path
import stat
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

INSTALL = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('runtime', INSTALL/'gps-runtime.py')
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class Runtime(unittest.TestCase):
    def test_check_never_starts_stopped_engine_and_persists_before_restart(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); config=root/'config.yaml'
            config.write_text('sensors:\n  gps:\n    device: /dev/null\n')
            calls=[]; active=[False]
            def systemctl(args, **kwargs):
                calls.append(args)
                if args[1]=='is-active': return SimpleNamespace(returncode=0 if active[0] else 3)
                state=json.loads((root/'state.json').read_text())
                self.assertEqual(state['attempted'],runtime.identity())
                self.assertEqual(len(state['restarts']),1)
                return SimpleNamespace(returncode=0)
            with patch.object(runtime,'ROOT',root), patch.object(runtime,'CONFIG',config), \
                 patch.object(runtime.sys,'argv',['gps-runtime','check']), \
                 patch.object(Path,'lstat',return_value=SimpleNamespace(st_mode=stat.S_IFDIR|0o700,st_uid=0)), \
                 patch.object(runtime.subprocess,'run',side_effect=systemctl):
                runtime.main()
                self.assertFalse((root/'state.json').exists())
                active[0]=True
                runtime.main()
                runtime.main()
            self.assertEqual(len([c for c in calls if c[1]=='try-restart']),1)
            self.assertIn(['systemctl','try-restart','--no-block',runtime.UNIT],calls)

    def test_status_distinguishes_first_fix_states_and_saved_position(self):
        with tempfile.TemporaryDirectory() as folder, patch.object(runtime, 'STATUS', Path(folder)):
            now=datetime.datetime.now(datetime.timezone.utc).isoformat()
            for state, gps, expected in [('gps-missing', {}, 'No GPS device found'),
                                         ('gps-waiting', {'satellites_used': 3, 'hdop': 4.2}, 'device present, no fix yet; satellites=3 HDOP=4.2')]:
                record=dict(state=state, gps=gps, written_at=now, usb_adapter_ids=['10c4:ea60'])
                (Path(folder)/'startup.json').write_text(json.dumps(record))
                out=io.StringIO()
                with contextlib.redirect_stdout(out): runtime.status()
                self.assertIn(expected, out.getvalue())
                if state=='gps-missing': self.assertIn('10c4:ea60',out.getvalue())
            (Path(folder)/'startup.json').unlink()
            (Path(folder)/'status.json').write_text(json.dumps(dict(written_at=now, status_interval_secs=5, heartbeat={'gps': {'state': 'failed'}})))
            out=io.StringIO()
            with contextlib.redirect_stdout(out): runtime.status()
            self.assertIn('position preserved, GPS not live',out.getvalue())

    def test_present_prepare_copies_exact_device_not_usb_symlink(self):
        with tempfile.TemporaryDirectory() as folder, patch.object(runtime, 'ROOT', Path(folder)), patch.object(runtime.os, 'mknod') as node, patch.object(runtime.os, 'replace') as replace, patch.object(runtime.os, 'chown'), patch.object(runtime.os, 'chmod'):
            state = {}
            current=[1, 2, runtime.os.makedev(166, 0), 4]
            runtime.prepare(state, current)
            node.assert_called_once_with(Path(folder)/'device.new', stat.S_IFCHR | 0o660, current[2])
            replace.assert_called_once_with(Path(folder)/'device.new', Path(folder)/'device')
            self.assertEqual(state['mapped'],current)

    def test_absence_never_restarts_and_replug_gets_one_attempt(self):
        state = {'mapped': [1]}
        for now in range(1000):
            self.assertFalse(runtime.should_restart(state, None, now))
        self.assertTrue(runtime.should_restart(state, [2], 1000))
        self.assertFalse(runtime.should_restart(state, [2], 2000))

    def test_flapping_is_rate_limited(self):
        state = {}
        self.assertTrue(runtime.should_restart(state, [1], 100))
        self.assertFalse(runtime.should_restart(state, [2], 101))
        self.assertTrue(runtime.should_restart(state, [2], 160))
        self.assertTrue(runtime.should_restart(state, [3], 220))
        self.assertFalse(runtime.should_restart(state, [4], 280))
        self.assertTrue(runtime.should_restart(state, [4], 700))

    def test_install_wiring(self):
        source = (INSTALL/'brrdfeeder-install.sh').read_text()
        self.assertIn('AddDevice=/run/brrdfeeder-gps/device:/dev/${GPS_SYMLINK}:rw', source)
        self.assertIn('ExecStartPre=/usr/local/libexec/brrdfeeder-gps-runtime prepare', source)
        self.assertIn('OnUnitInactiveSec=5s', source)
        self.assertIn((INSTALL/'gps-runtime.py').read_text(), source)


if __name__ == '__main__':
    unittest.main(verbosity=2)
