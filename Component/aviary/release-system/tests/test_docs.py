#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Offline documentation contracts; these do not exercise a deployed updater."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class Docs(unittest.TestCase):
    def test_public_readme_has_no_absent_private_paths(self):
        text = (ROOT / 'README.md').read_text()
        self.assertNotIn('governance/reviews/', text, 'public README references absent private proof paths')

    def test_standalone_compiler_matches_build_script(self):
        build = (ROOT / 'build-host.sh').read_text()
        expected = re.search(r'Go ([0-9.]+) required', build).group(1)
        self.assertIn('Go ' + expected, (ROOT / 'README.md').read_text(),
                      'README does not name the standalone script compiler')

    def test_signer_client_is_not_approval_or_deployment(self):
        text = (ROOT / 'README.md').read_text()
        self.assertIn('low-level signer client', text, 'publisher boundary is undocumented')
        self.assertIn('not evidence that S5 is deployed', text, 'source is mistaken for live signer evidence')

    def test_join_is_conditional(self):
        text = (ROOT / 'README.md').read_text()
        self.assertIn('Classify the installed baseline first', text,
                      'README unconditionally directs uninstall instead of baseline classification')


if __name__ == '__main__':
    unittest.main()
