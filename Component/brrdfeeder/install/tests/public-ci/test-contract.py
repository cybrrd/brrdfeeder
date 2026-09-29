#!/usr/bin/env python3
"""Offline CI parity and public closure; no credentials, network or builds."""
import ast
from pathlib import Path
import re
import sys
import unittest
import yaml
ROOT=Path(__file__).resolve().parents[5]
TESTS=ROOT/'Component/brrdfeeder/install/tests'
sys.path.insert(0,str(TESTS/'github-scaffold'))
from release_fixture import ReleaseFixture

class Gate(unittest.TestCase):
    def test_release_cli_uses_one_digest_and_correct_archives(self):
        with ReleaseFixture() as fixture:
            fixture.build_both()
            self.assertFalse(fixture.writes(), 'build phase wrote to registry or signed')
            result=fixture.run('publish-images.sh')
            self.assertEqual(result.returncode,0,result.stdout+result.stderr)
            calls=fixture.calls()
            for component,repo,hexchar in [('engine','brrdfeeder','b'),('console','brrdhouse','c')]:
                target='ghcr.io/cybrrd/'+repo+'@sha256:'+hexchar*64
                self.assertEqual((fixture.work/'out/release'/component/'image.txt').read_text().strip(),target)
                self.assertIn(['cosign',['sign','--yes',target]],calls)
            copies=[argv for name,argv in calls if name=='skopeo' and argv[0]=='copy']
            self.assertEqual(len(copies),2)
            for args in copies:
                self.assertIn('--preserve-digests',args)
                self.assertTrue(any(arg.startswith('oci-archive:') for arg in args))
    def test_release_cli_refuses_foreign_repository_before_tools(self):
        with ReleaseFixture() as fixture:
            p=fixture.build('engine',GITHUB_REPOSITORY='foreign/fork')
            self.assertNotEqual(p.returncode,0)
            self.assertFalse(fixture.calls())
    def test_release_cli_refuses_doubled_digest_before_attestation(self):
        with ReleaseFixture() as fixture:
            p=fixture.build('engine',FIXTURE_DIGEST='sha256:sha256:'+'b'*64)
            self.assertNotEqual(p.returncode,0)
            self.assertFalse(fixture.writes())
    def test_every_moved_suite_is_catalogued(self):
        tree=ast.parse((TESTS/'run.py').read_text())
        lists={n.targets[0].id:ast.literal_eval(n.value) for n in tree.body
               if isinstance(n,ast.Assign) and isinstance(n.targets[0],ast.Name)
               and n.targets[0].id in ('BASELINES','MUTATIONS')}
        registered=set(lists['BASELINES']+lists['MUTATIONS']+['uninstall-wtmpdb/test-history.py'])
        found={p.relative_to(TESTS).as_posix() for p in TESTS.rglob('test-*.py')}
        self.assertEqual(found,{name for name in registered if name.split('/')[-1].startswith('test-')})
    def test_public_gate_runs_all_groups_and_no_private_paths(self):
        text=(ROOT/'.github/workflows/test.yml').read_text()
        self.assertIn('Component/brrdfeeder/install/tests/run.py --group all',text)
        self.assertNotIn('governance/',text)
        self.assertNotIn('continue-on-error',text)
        for action in re.findall(r'uses: ([^\s]+)',text):
            self.assertRegex(action,r'@[0-9a-f]{40}$')
        self.assertEqual(yaml.safe_load(text)['permissions'],{'contents':'read'})
    def test_release_is_test_gated_and_scoped(self):
        text=(ROOT/'.github/workflows/release.yml').read_text()
        value=yaml.safe_load(text)
        self.assertIn('gate',value['jobs'])
        for name,job in value['jobs'].items():
            if name!='gate': self.assertIn('gate',job['needs'])
        self.assertIn("github.repository == 'cybrrd/brrdfeeder'",text)
        self.assertNotIn('/latest/',text)
        self.assertNotIn('SET-AT-CREATION',text)
        self.assertNotIn('ci-build.sh',text)
    def test_no_source_archive_or_console_disclosure(self):
        self.assertFalse((ROOT/'Component/brrdhouse/source.tar.gz').exists())
        text=(ROOT/'Component/brrdhouse/web/page.html').read_text()
        self.assertNotIn('BRRDhouse Open',text)
        self.assertNotIn('source.tar.gz',text)
        self.assertNotIn('github.com/',text)
if __name__=='__main__': unittest.main(verbosity=2)
