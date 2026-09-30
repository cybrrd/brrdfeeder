#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline CI parity and public closure; no credentials, network or builds."""
import ast
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
import unittest
import yaml
ROOT=Path(__file__).resolve().parents[5]
TESTS=ROOT/'Component/brrdfeeder/install/tests'
sys.path.insert(0,str(TESTS/'github-scaffold'))
from release_fixture import ReleaseFixture

class Gate(unittest.TestCase):
    def test_single_lockfile_gate_is_required_before_provisioning(self):
        path = 'Component/brrdfeeder/install/tests/public-ci/check-single-lockfile.py'
        steps = yaml.safe_load((ROOT/'.github/workflows/test.yml').read_text())['jobs']['contracts']['steps']
        positions = [i for i, step in enumerate(steps) if step.get('run') == 'python3 ' + path]
        self.assertEqual(len(positions), 1)
        step = steps[positions[0]]
        self.assertNotIn('if', step)
        self.assertNotIn('continue-on-error', step)
        provision = next(i for i, step in enumerate(steps) if 'setup-tests.sh' in step.get('run', ''))
        self.assertLess(positions[0], provision)
        runner = (TESTS/'run.py').read_text()
        self.assertIn("run('single-lockfile', [sys.executable, HERE/'public-ci/check-single-lockfile.py'], timeout=30)", runner)
        self.assertIn(path, (ROOT/'.github/dependabot.yml').read_text())

    def test_ble_contracts_without_interpreter_bluetooth_constants(self):
        # setup-python may omit Bluetooth support. Exercise the entire suite,
        # including its mocked controller syscalls, without either constant.
        program='''import runpy, socket, sys
for name in ('AF_BLUETOOTH', 'BTPROTO_HCI'):
    if hasattr(socket, name): delattr(socket, name)
sys.argv = [sys.argv[1]]
runpy.run_path(sys.argv[0], run_name='__main__')
'''
        result=subprocess.run([sys.executable,'-c',program,
                               str(TESTS/'pi-native-p0/test-ble.py')],
                              cwd=ROOT,capture_output=True,text=True,timeout=120)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertIn('Ran 15 tests',result.stderr)
        self.assertNotIn('skipped',result.stderr)

    def test_public_contract_step_does_not_claim_same_runner(self):
        workflow=yaml.safe_load((ROOT/'.github/workflows/test.yml').read_text())
        steps=workflow['jobs']['contracts']['steps']
        contract=[step for step in steps if 'tests/run.py --group all' in step.get('run','')]
        self.assertEqual(len(contract),1)
        self.assertEqual(contract[0]['name'],'All offline contracts')

    def test_rustup_executes_named_verified_installer(self):
        text=(ROOT/'.github/scripts/setup-tests.sh').read_text()
        self.assertIn('rust_installer_dir=$(mktemp -d)',text)
        self.assertIn('rust_installer="$rust_installer_dir/rustup-init"',text)
        self.assertIn('trap \'rm -f "$rust_installer"; rmdir "$rust_installer_dir"\' EXIT',text)
        verify="printf '%s  %s\\n' \"$rust_sha\" \"$rust_installer\" | sha256sum -c -"
        execute='"$rust_installer" -y --profile minimal --default-toolchain 1.88.0 --no-modify-path'
        self.assertIn(verify,text)
        self.assertIn(execute,text)
        self.assertLess(text.index(verify),text.index(execute))
        self.assertIn('set -euo pipefail',text)
        for digest in ('e3853c5a252fca15252d07cb23a1bdd9377a8c6f3efa01531109281ae47f841c',
                       '20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c'):
            self.assertIn(digest,text)

    def test_dependabot_workspace_uses_only_root_lockfile(self):
        updates=yaml.safe_load((ROOT/'.github/dependabot.yml').read_text())['updates']
        cargo=[u for u in updates if u['package-ecosystem']=='cargo']
        self.assertEqual(len(cargo),1)
        self.assertEqual(cargo[0]['directory'],'/Component/aviary')
        workspace=ROOT/'Component/aviary'
        result = subprocess.run([sys.executable, str(TESTS/'public-ci/check-single-lockfile.py')],
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        manifest=tomllib.loads((workspace/'Cargo.toml').read_text())
        self.assertTrue((workspace/'Cargo.lock').is_file())
        for member in manifest['workspace']['members']:
            self.assertTrue((workspace/member/'Cargo.toml').is_file(),member)
            self.assertFalse((workspace/member/'Cargo.lock').exists(),
                             'stale member lockfile creates a separate Dependabot security-update root: '+member)

    def test_cargo_metadata_resolves_root_and_every_member_offline(self):
        workspace=ROOT/'Component/aviary'
        members=tomllib.loads((workspace/'Cargo.toml').read_text())['workspace']['members']
        lock=(workspace/'Cargo.lock').read_bytes()
        roots=[]
        for directory in [workspace,*(workspace/member for member in members)]:
            result=subprocess.run(['cargo','metadata','--locked','--offline','--format-version','1',
                                   '--manifest-path',str(directory/'Cargo.toml')],
                                  cwd=ROOT,capture_output=True,text=True,timeout=120)
            self.assertEqual(result.returncode,0,result.stderr)
            value=json.loads(result.stdout)
            self.assertEqual(Path(value['workspace_root']),workspace)
            roots.append(set(value['workspace_members']))
            for package in value['packages']:
                if package['source'] is None:
                    self.assertTrue(Path(package['manifest_path']).is_relative_to(workspace))
        self.assertTrue(all(root==roots[0] for root in roots))
        self.assertEqual(len(roots[0]),len(members))
        self.assertEqual((workspace/'Cargo.lock').read_bytes(),lock)

    def test_readme_links_root_license(self):
        text=(ROOT/'README.md').read_text()
        self.assertIn('[LICENSE](LICENSE)',text)
        self.assertNotIn('the component LICENSE files',text)
        self.assertIn('GNU AFFERO GENERAL PUBLIC LICENSE',(ROOT/'LICENSE').read_text())

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
class SingleLockfile(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='single-lockfile-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.lock('Component/aviary/Cargo.lock')

    def lock(self, path):
        target = self.root/path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text('# isolated lockfile fixture\n')
        return target

    def run_gate(self):
        return subprocess.run([sys.executable, str(TESTS/'public-ci/check-single-lockfile.py'),
                               '--root', str(self.root)], capture_output=True, text=True, timeout=30)

    def reject(self, paths):
        result = self.run_gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('Use the workspace root lock: Component/aviary/Cargo.lock', result.stderr)
        self.assertEqual([line.strip() for line in result.stderr.splitlines() if line.startswith('  ')], sorted(paths))

    def test_root_lock_only_passes(self):
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('PASS:', result.stdout)

    def test_member_lock_rejected(self):
        path = 'Component/aviary/engine/Cargo.lock'
        self.lock(path)
        self.reject([path])

    def test_untracked_deep_lock_rejected_without_git(self):
        path = 'Component/aviary/tools/x/Cargo.lock'
        self.lock(path)
        self.assertFalse((self.root/'.git').exists())
        self.reject([path])

    def test_all_hidden_and_ignored_paths_reported_in_order(self):
        paths = ['Component/aviary/target/deep/Cargo.lock', 'Component/aviary/.hidden/Cargo.lock']
        (self.root/'.gitignore').write_text('target/\n.hidden/\n')
        for path in paths:
            self.lock(path)
        self.reject(paths)

    def test_dangling_lock_symlink_rejected(self):
        path = 'Component/aviary/engine/Cargo.lock'
        target = self.root/path
        target.parent.mkdir(parents=True)
        target.symlink_to('absent.lock')
        self.reject([path])

    def test_directory_symlink_cycle_is_not_followed(self):
        (self.root/'Component/aviary/loop').symlink_to('.', target_is_directory=True)
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_other_component_locks_are_out_of_scope(self):
        self.lock('Component/another/Cargo.lock')
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_missing_workspace_root_lock_fails(self):
        (self.root/'Component/aviary/Cargo.lock').unlink()
        result = self.run_gate()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('missing workspace root lock Component/aviary/Cargo.lock', result.stderr)


if __name__=='__main__': unittest.main(verbosity=2)
