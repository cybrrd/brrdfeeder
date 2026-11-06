#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Negative controls for GPS hotplug qualification and ownership safety."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
HELPER = HERE.parents[1]/'gps-runtime.py'
CONTRACT = HERE/'test-contract.py'


def run(helper):
    env = os.environ.copy()
    env.update(GPS_HOTPLUG_RUNTIME=str(helper), PYTHONDONTWRITEBYTECODE='1')
    return subprocess.run([sys.executable, CONTRACT], capture_output=True, text=True,
                          env=env, timeout=30)


class Mutations(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        control = run(HELPER)
        if control.returncode:
            raise AssertionError('clean GPS hotplug contract failed:\n'+control.stdout+control.stderr)

    def test_each_safety_or_revert_mutation_is_caught(self):
        mutations = {
            'revert-discovery': (
                '    now = time.monotonic() if now is None else now\n    native = identity()\n',
                '    return identity()  # mutation: pre-0.9.1 behavior\n'),
            'open-before-busy-refusal': (
                '        ownership = held_elsewhere(info.st_rdev)\n        if ownership:\n            return ownership\n        fd = os.open',
                '        ownership = held_elsewhere(info.st_rdev)\n        if False:  # mutation: ignore pre-open owner\n            return ownership\n        fd = os.open'),
            'restore-failure-weakened': (
                "                return 'restore-failed'\n",
                "                return 'no-nmea'  # mutation: hide uncertain tty state\n"),
            'no-nmea-adopted': (
                "    if outcome != 'ok':\n",
                "    if False:  # mutation: adopt every probe outcome\n"),
        }
        source = HELPER.read_text()
        for name, (old, new) in mutations.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as folder:
                self.assertEqual(source.count(old), 1, name)
                mutant = Path(folder)/'gps-runtime.py'
                mutant.write_text(source.replace(old, new))
                result = run(mutant)
                self.assertNotEqual(result.returncode, 0, name+' survived:\n'+result.stdout+result.stderr)


if __name__ == '__main__': unittest.main(verbosity=2)
