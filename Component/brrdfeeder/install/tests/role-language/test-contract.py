#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Role-language gate boundary and future-file controls."""
import importlib.util
import os
import re
from pathlib import Path
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('role_check', HERE / 'check.py')
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)

class Contract(unittest.TestCase):
    def test_shipped_tree(self):
        self.assertEqual(gate.scan(gate.ROOT), [])

    def test_case_and_boundaries(self):
        name = 'S' + 'ynth'
        self.assertTrue(gate.inspect('new.md', (name.upper() + ' approves').encode()))
        self.assertFalse(gate.inspect('new.md', ('photosynthesis cyBRRD ' + name + 'esis').encode()))

    def test_each_person_token_rejects_mixed_case(self):
        definitions = gate.PATTERN.pattern.split('the [p]ack')[0]
        names = [a + b for a, b in re.findall(r'\[([a-z])\]([a-z]+)', definitions)]
        self.assertGreaterEqual(len(names), 20)
        for name in names:
            with self.subTest(name=name):
                self.assertTrue(gate.inspect('new.md', (name.upper() + ' approves').encode()))

    def test_protocol_and_product_terms(self):
        self.assertFalse(gate.inspect('new.md', b'ASTM Message Pack; pack decoding; round-robin'))
        self.assertFalse(gate.inspect('new.md', b'Claude Code CLI is a product example'))

    def test_exceptions_are_exact_and_cannot_move(self):
        marker = 'P' + 'ack-canonical chrony override'
        self.assertTrue(gate.inspect('new.md', marker.encode()))
        for (path, digest) in gate.ALLOWLIST:
            self.assertEqual(len(digest), 64)
            self.assertFalse(path.startswith('governance/'))
            lines = (gate.ROOT / path).read_text().splitlines()
            matches = [line for line in lines if gate.hashlib.sha256(line.strip().encode()).hexdigest() == digest]
            self.assertEqual(len(matches), 1, (path, digest))
            line = matches[0].encode()
            self.assertEqual(gate.inspect(path, line), [])
            self.assertTrue(gate.inspect('new.md', line))
            self.assertTrue(gate.inspect(path, line + b' changed'))

    def test_legacy_account_is_explicit_and_fails_before_mutation(self):
        script = gate.ROOT / 'Component/aviary/deploy/bootstrap/brrdfeeder-install.sh'
        for user in ('', 'root', '../operator', 'operator;false'):
            result = subprocess.run(['bash', str(script), '--dry-run'], capture_output=True,
                text=True, timeout=3, env=dict(os.environ, BRRDFEEDER_LEGACY_USER=user))
            self.assertEqual(result.returncode, 2)
            self.assertIn('set BRRDFEEDER_LEGACY_USER', result.stderr)

    def test_new_updater_files_use_current_markers(self):
        for directory in ('deploy/host-updater', 'release-system/units'):
            for suffix, marker in (('sh', 'installer'), ('service', 'service')):
                path = gate.ROOT / 'Component/aviary' / directory / ('brrdfeeder-updater.' + suffix)
                text = path.read_text()
                self.assertIn('# BRRDfeeder signed-update ' + marker + '.', text)
                self.assertEqual(gate.inspect(str(path.relative_to(gate.ROOT)), text.encode()), [])

    def test_new_index_file_is_scanned(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            subprocess.run(['git', 'init', '-q', tmp], check=True)
            (root / 'new.md').write_text('C' + 'y approves')
            subprocess.run(['git', 'add', 'new.md'], cwd=root, check=True)
            self.assertEqual(len(gate.scan(root)), 1)

if __name__ == '__main__':
    unittest.main(verbosity=2)
