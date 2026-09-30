#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Ownership, packaging and fail-closed REUSE contracts; offline only."""
# REUSE-IgnoreStart
from pathlib import Path
import hashlib
import subprocess
import tempfile
import unittest
import tomllib

ROOT=Path(__file__).resolve().parents[5]
NOTICE='''Copyright © 2026 Macawi LLC, Iowa, USA.
BRRDfeeder and BRRDhouse are licensed under the GNU Affero General Public License v3.0 or later (see LICENSE).
cyBRRD, BRRDfeeder and BRRDhouse are developed and operated by cyBRRD Corporation, Iowa, USA, under license from Macawi LLC.
'''

class Ownership(unittest.TestCase):
    def test_exact_notice_and_readme(self):
        self.assertEqual((ROOT/'NOTICE').read_text(),NOTICE)
        self.assertIn(NOTICE,(ROOT/'README.md').read_text())
        for component in ('aviary','brrdhouse'):
            self.assertEqual((ROOT/'Component'/component/'NOTICE').read_text(),NOTICE)

    def test_both_images_package_license_notice_and_vendor(self):
        for path,component in [('Component/aviary/engine/Containerfile','brrdfeeder'),
                               ('Component/brrdhouse/Containerfile','brrdhouse')]:
            text=(ROOT/path).read_text()
            self.assertIn('org.opencontainers.image.vendor="cyBRRD Corporation"',text)
            self.assertRegex(text,r'COPY[^\n]*NOTICE[^\n]*LICENSE[^\n]*/usr/share/doc/'+component+r'/')
            self.assertIn('com.macawi.'+component+'.build_seq',text)

    def test_cargo_authors_and_authoritative_policy(self):
        for path,key in [('Component/aviary/Cargo.toml','workspace'),
                         ('Component/aviary/cybrrd-rid-core/Cargo.toml','package')]:
            data=tomllib.loads((ROOT/path).read_text())
            package=data[key]['package'] if key=='workspace' else data[key]
            self.assertEqual(package['authors'],['cyBRRD Corporation <admin@cybrrd.com>'])
        # Repository-owner policy at public base 83fc7b0 is authoritative.
        # Do not add definitions or headers to those exact bytes; use sidecars.
        for path, digest in {
            'SECURITY.md': 'd4c087d61352b311c7ecc624f69cdd4309e40f9092c87af9fe5fa13462f1407d',
            'CONTRIBUTING.md': 'aebe9d4fe190d48e8e5d2a16c541cb3a5bc9b17611aa712d2fce04da00abbc51',
        }.items():
            self.assertEqual(hashlib.sha256((ROOT/path).read_bytes()).hexdigest(), digest)
            self.assertIn('SPDX-FileCopyrightText: 2026 Macawi LLC', (ROOT/(path+'.license')).read_text())

    def test_gate_is_pinned_and_registered(self):
        setup=(ROOT/'.github/scripts/setup-tests.sh').read_text()
        self.assertIn('reuse==6.2.0',setup)
        runner=(ROOT/'Component/brrdfeeder/install/tests/run.py').read_text()
        self.assertIn("'ownership/test-contract.py'",runner)
        self.assertIn("'reuse-lint'",runner)

    def test_reuse_rejects_missing_owner_and_license(self):
        # No blanket fallback: a new source file must declare both fields.
        with tempfile.TemporaryDirectory(prefix='ownership-negative-') as folder:
            root=Path(folder)
            (root/'LICENSES').mkdir()
            (root/'LICENSES/AGPL-3.0-or-later.txt').write_bytes((ROOT/'LICENSE').read_bytes())
            source=root/'example.py'
            license_line='# SPDX-License-Identifier: AGPL-3.0-or-later\n'
            owner_line='# SPDX-FileCopyrightText: 2026 Macawi LLC\n'
            for text,good in [(license_line+owner_line,True),(license_line,False),(owner_line,False)]:
                source.write_text(text+'print("example")\n')
                result=subprocess.run(['reuse','--root',str(root),'lint'],capture_output=True,text=True)
                self.assertEqual(result.returncode==0,good,result.stdout+result.stderr)

if __name__=='__main__': unittest.main(verbosity=2)
# REUSE-IgnoreEnd
