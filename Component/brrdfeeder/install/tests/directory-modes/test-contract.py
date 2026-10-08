#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Execute extracted directory operations only, in temporary paths, never installer."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

INSTALL = Path(__file__).resolve().parents[2]/'brrdfeeder-install.sh'
SOURCE = INSTALL.read_text()


class Modes(unittest.TestCase):
    def execute(self, source, broken=False):
        helper = re.search(r'^public_directories\(\) \{.*?^}', source, re.M | re.S)
        quadlet = source.split('QUADLET_DIR="$(dirname "$QUADLET_FILE")"\n')[1].split('\nNEW_QUADLET=')[0]
        console = source.split('  run chown root:root "$CONSOLE_QUADLET_FILE"\n')[1].split('  run ln -sfn')[0]
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            # uutils semantics: missing parents inherit umask; only explicitly
            # named operands receive -m. No chown/host paths are executed.
            shim = root/'install.py'
            shim.write_text('import os, pathlib, sys\n'
                            'for arg in sys.argv[1:]:\n'
                            ' if arg.startswith("/"):\n'
                            '  p=pathlib.Path(arg); p.mkdir(parents=True, exist_ok=True)\n'
                            '  if not os.environ.get("BROKEN_INSTALL"): p.chmod(0o755)\n')
            program = 'set -eu\numask 077\nDRY_RUN=0\nCONSOLE_UID=12345\n'
            program += 'run() { "$@"; }\nfatal() { echo "$*" >&2; exit 1; }\n'
            program += 'install() { python3 "$SHIM" "$@"; }\n'
            program += (helper.group()+'\n' if helper else '')
            program += 'QUADLET_DIR=/etc/containers/systemd\n'+quadlet+console
            program = program.replace('/etc/containers', str(root/'etc/containers'))
            result = subprocess.run(['bash', '-c', program], text=True, capture_output=True,
                                    env=dict(os.environ, SHIM=str(shim), BROKEN_INSTALL='1' if broken else ''))
            modes = {str(p.relative_to(root)): p.stat().st_mode & 0o777
                     for p in root.rglob('*') if p.is_dir()}
            return result, modes

    def test_uutils_missing_parents_are_traversable(self):
        result, modes = self.execute(SOURCE)
        self.assertEqual(result.returncode, 0, result.stderr)
        for path in ('etc/containers', 'etc/containers/systemd',
                     'etc/containers/systemd/users', 'etc/containers/systemd/users/12345'):
            self.assertEqual(modes[path], 0o755, path)

    def test_mode_failure_is_fatal(self):
        result, _ = self.execute(SOURCE, broken=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('0755', result.stderr)

    def test_parent_omission_mutant_is_detected(self):
        mutant = SOURCE.replace('/etc/containers/systemd/users \\\n', '')
        self.assertTrue(mutant != SOURCE, 'mutation site must exist')
        result, modes = self.execute(mutant)
        self.assertTrue(result.returncode != 0 or modes['etc/containers/systemd/users'] != 0o755)


if __name__ == '__main__':
    unittest.main(verbosity=2)
