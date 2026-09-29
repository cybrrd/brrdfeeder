#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Role-language gate boundary and future-file controls."""
import importlib.util
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

    def test_protocol_and_product_terms(self):
        self.assertFalse(gate.inspect('new.md', b'ASTM Message Pack; decoder rejects the pack; round-robin'))
        self.assertFalse(gate.inspect('new.md', b'Claude Code CLI is a product example'))

    def test_exceptions_are_exact_and_cannot_move(self):
        marker = 'P' + 'ack-canonical chrony override'
        self.assertTrue(gate.inspect('new.md', marker.encode()))
        for (path, digest) in gate.ALLOWLIST:
            self.assertEqual(len(digest), 64)
            self.assertFalse(path.startswith('governance/'))

    def test_new_index_file_is_scanned(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            subprocess.run(['git', 'init', '-q', tmp], check=True)
            (root / 'new.md').write_text('C' + 'y approves')
            subprocess.run(['git', 'add', 'new.md'], cwd=root, check=True)
            self.assertEqual(len(gate.scan(root)), 1)

if __name__ == '__main__':
    unittest.main(verbosity=2)
