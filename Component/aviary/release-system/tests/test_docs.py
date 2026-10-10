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
        self.assertFalse('governance/reviews/' in text, 'public README references absent private proof paths')

    def test_standalone_compiler_matches_build_script(self):
        build = (ROOT / 'build-host.sh').read_text()
        expected = re.search(r'Go ([0-9.]+) required', build).group(1)
        self.assertTrue('Go ' + expected in (ROOT / 'README.md').read_text(),
                      'README does not name the standalone script compiler')

    def test_signer_client_is_not_approval_or_deployment(self):
        text = (ROOT / 'README.md').read_text()
        self.assertTrue('low-level signer client' in text, 'publisher boundary is undocumented')
        self.assertTrue('not evidence that S5 is deployed' in text, 'source is mistaken for live signer evidence')

    def test_join_is_conditional(self):
        text = (ROOT / 'README.md').read_text()
        self.assertTrue('Classify the installed baseline first' in text,
                      'README unconditionally directs uninstall instead of baseline classification')

    def test_local_links_exist(self):
        for name in ('README.md', 'PROOF.md'):
            for link in re.findall(r'\]\(([^)]+)\)', (ROOT / name).read_text()):
                if '://' not in link:
                    self.assertTrue((ROOT / link.split('#')[0]).exists(), 'missing local documentation target: ' + link)

    def test_proof_keeps_native_and_fixture_boundaries(self):
        text = (ROOT / 'PROOF.md').read_text()
        for phrase in ('not executed native proof', 'NOT_RUN', 'bad-first',
                       'same proved good image', 'physical power-cut',
                       'Never request a remote downgrade', 'independently received Red'):
            self.assertTrue(phrase in text, 'missing proof boundary: ' + phrase)


if __name__ == '__main__':
    unittest.main()
