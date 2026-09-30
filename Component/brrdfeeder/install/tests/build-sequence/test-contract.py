#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Execute build scripts in a real one-commit repo with inert build effectors."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[5]
SCRIPTS=['Component/aviary/tools/build-arm64.sh','Component/brrdhouse/build.sh']

class BuildSequence(unittest.TestCase):
    def test_nested_workflow_cannot_bypass_sequence_offset(self):
        workflow=ROOT/'Component/aviary/.github/workflows/build-container.yml'
        if not workflow.exists():
            # This legacy workflow is outside the approved public snapshot.
            self.assertTrue((ROOT/'.github/workflows/release.yml').is_file())
            return
        source=workflow.read_text()
        self.assertIn('readonly BUILD_SEQ_OFFSET=1000',source)
        self.assertIn('seq=$((BUILD_SEQ_OFFSET + commit_count))',source)
    def fixture(self,root):
        repo=root/'repo';repo.mkdir()
        for name in SCRIPTS:
            dst=repo/name;dst.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(ROOT/name,dst)
        env=dict(os.environ,GIT_AUTHOR_NAME='Synth',GIT_AUTHOR_EMAIL='synth@cybrrd.com',
            GIT_COMMITTER_NAME='Synth',GIT_COMMITTER_EMAIL='synth@cybrrd.com')
        for args in [['init','-b','main'],['add','.'],['commit','-m','Synthetic one-commit sequence fixture']]:
            subprocess.run(['git',*args],cwd=repo,env=env,check=True,capture_output=True)
        binary=root/'bin';binary.mkdir()
        for name,source in {'uname':'#!/bin/sh\necho aarch64\n',
                'podman':'#!/bin/sh\nprintf "%s\\n" "$*" >> "$FIXTURE_CALLS"\n',
                'tar':'#!/bin/sh\nexit 0\n'}.items():
            path=binary/name;path.write_text(source);path.chmod(0o755)
        env.update(PATH=str(binary)+':'+env['PATH'],FIXTURE_CALLS=str(root/'calls'),BUILD_SEQ_OFFSET='0',BUILD_SEQ='1')
        return repo,env
    def test_one_commit_build_scripts_emit_1001_despite_ambient_override(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);repo,env=self.fixture(root)
            self.assertEqual(subprocess.check_output(['git','rev-list','--count','HEAD'],cwd=repo,text=True).strip(),'1')
            engine=subprocess.run(['bash',str(repo/SCRIPTS[0]),'--check'],env=env,text=True,capture_output=True)
            self.assertEqual(engine.returncode,0,engine.stderr)
            self.assertIn('BUILD_SEQ=1001\n',engine.stdout)
            console=subprocess.run(['sh',str(repo/SCRIPTS[1]),'arm64'],env=env,text=True,capture_output=True)
            self.assertEqual(console.returncode,0,console.stderr)
            calls=(root/'calls').read_text()
            self.assertIn('--build-arg BUILD_SEQ=1001',calls)
            self.assertNotIn('--build-arg BUILD_SEQ=1 ',calls)
            self.assertGreater(1001,824)
    def test_both_build_scripts_refuse_shallow_history(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);repo,env=self.fixture(root)
            shallow=root/'shallow'
            subprocess.run(['git','clone','--depth=1',repo.as_uri(),str(shallow)],check=True,capture_output=True)
            for script,args in [(SCRIPTS[0],['--check']),(SCRIPTS[1],['arm64'])]:
                result=subprocess.run(['bash',str(shallow/script),*args],env=env,text=True,capture_output=True)
                self.assertNotEqual(result.returncode,0,script)
                self.assertIn('shallow',result.stderr.lower())

if __name__=='__main__':unittest.main(verbosity=2)
