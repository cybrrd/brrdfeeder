#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Read-only source/version checks; no tag, release, workflow or network writes."""
import importlib.util
import io
import os
from pathlib import Path
import re
import subprocess
import tomllib
from unittest.mock import patch

root = Path(__file__).resolve().parents[2]
os.chdir(root)
spec = importlib.util.spec_from_file_location('metadata', root / '.github/scripts/release-metadata.py')
metadata = importlib.util.module_from_spec(spec)
spec.loader.exec_module(metadata)

def git(*args):
    return subprocess.check_output(['git', *args], text=True).strip()

class Manifest:
    def __init__(self, data):
        self.data = data
    def open(self, mode):
        assert mode == 'rb'
        return io.BytesIO(self.data.encode())

builds = []
for branch, version in [('drill/bad-0.8.31-drill.1', '0.8.31-drill.1'),
                        ('drill/good-0.8.31', '0.8.31')]:
    revision = git('rev-parse', branch)
    manifest = Manifest(git('show', branch + ':Component/aviary/engine/Cargo.toml'))
    tag = 'v' + version
    assert re.fullmatch(r'v[0-9][A-Za-z0-9_.-]*', tag)
    assert metadata.product_version(tag, manifest) == version
    lock = tomllib.loads(git('show', branch + ':Component/aviary/Cargo.lock'))
    assert next(p['version'] for p in lock['package'] if p['name'] == 'engine') == version
    try:
        metadata.product_version('v0.8.30', manifest)
        raise AssertionError('wrong tag accepted')
    except ValueError:
        pass
    count = git('rev-list', '--count', branch)
    # Model only immutable Git reads, including a future tag resolution. No
    # actual tag is made. This checks source eligibility, not GitHub execution.
    reads = {('git', 'rev-parse', 'HEAD'): revision,
             ('git', 'rev-parse', 'refs/tags/' + tag + '^{commit}'): revision,
             ('git', 'rev-parse', '--is-shallow-repository'): 'false',
             ('git', 'rev-list', '--count', 'HEAD'): count}
    env = {'GITHUB_REPOSITORY': 'cybrrd/brrdfeeder', 'GITHUB_REF': 'refs/tags/' + tag,
           'GITHUB_REF_NAME': tag, 'GITHUB_SHA': revision,
           'GITHUB_WORKFLOW_REF': 'cybrrd/brrdfeeder/.github/workflows/release.yml@refs/tags/' + tag,
           'GITHUB_RUN_ID': '1', 'GITHUB_RUN_ATTEMPT': '1'}
    original = metadata.product_version
    with patch.object(metadata, 'command', side_effect=lambda *args: reads[args]), \
         patch.object(metadata, 'product_version', side_effect=lambda tag=None: original(tag, manifest)):
        for event in ['push', 'workflow_dispatch']:
            with patch.dict(os.environ, dict(env, GITHUB_EVENT_NAME=event)):
                ctx = metadata.context()
                assert ctx['product_version'] == version
                assert ctx['build_seq'] == 1000 + int(count)
    builds.append(ctx['build_seq'])
    print(branch, revision, 'build=' + str(ctx['build_seq']), 'version/tag/lock/context PASS (modeled tag reads)')
assert builds[1] > builds[0], builds
print('PASS: good build strictly exceeds bad on both components')
