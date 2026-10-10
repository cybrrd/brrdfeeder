# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Real one-commit source, real release scripts, inert external effectors."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[5]
HOST_ENV_PREFIXES = ('GITHUB_', 'RUNNER_', 'ACTIONS_', 'FIXTURE_')
MOCK = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
name=Path(sys.argv[0]).name; args=sys.argv[1:]
with open(os.environ['TRACE'],'a') as f: f.write(json.dumps([name,args])+'\n')
if name=='uname': print(os.environ.get('FIXTURE_UNAME','aarch64'))
if name=='skopeo' and args[0]=='login': sys.stdin.read()
if name=='skopeo' and args[0]=='inspect':
    target=args[-1]; component='console' if ('/console/' in target or 'brrdhouse' in target) else 'engine'
    if '--config' in args:
        key='com.macawi.'+('brrdhouse' if component=='console' else 'brrdfeeder')+'.build_seq'
        print(json.dumps({'architecture':os.environ.get('FIXTURE_ARCH','arm64'),'os':'linux','config':{'Labels':{
            key:os.environ.get('FIXTURE_SEQUENCE','1001'),
            'org.opencontainers.image.revision':os.environ.get('FIXTURE_REVISION',os.environ['GITHUB_SHA'])}}}))
    else:
        value='sha256:'+('c' if component=='console' else 'b')*64
        print(os.environ.get('FIXTURE_REMOTE_DIGEST',value) if target.startswith('docker://') else os.environ.get('FIXTURE_DIGEST',value))
if name=='syft': print(json.dumps({'bomFormat':'CycloneDX','specVersion':'1.6','version':1}))
if name=='gh' and not os.environ.get('GH_TOKEN'): sys.exit('GH_TOKEN missing')
if name=='gh' and os.environ.get('FIXTURE_EXISTING_RELEASE'): sys.exit('release already exists')
if name=='cosign' and args[0]=='sign-blob':
    if os.environ.get('FIXTURE_RECEIPT_SIGN_FAIL'): sys.exit('fixture signing refused')
    Path(args[args.index('--bundle')+1]).write_text('{"fixture_only":true}')
'''

class ReleaseFixture:
    def __enter__(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='scaffold-cli-')
        self.work = Path(self.tmp.name)
        binary = self.work/'bin'; binary.mkdir()
        self.trace = self.work/'trace.jsonl'
        for name in ('podman','skopeo','syft','cosign','gh','uname'):
            path=binary/name;path.write_text(MOCK);path.chmod(0o700)
        shutil.copytree(ROOT/'.github/scripts', self.work/'.github/scripts')
        manifest=self.work/'Component/aviary/engine/Cargo.toml';manifest.parent.mkdir(parents=True)
        manifest.write_text('[package]\nname = "engine"\nversion = "1.2.3"\n')
        engine=self.work/'Component/aviary/tools';engine.mkdir(parents=True)
        (engine/'build-arm64.sh').write_text('''#!/bin/bash
set -eu
mkdir -p "$2"
printf engine-fixture > "$2/brrdfeeder-engine-$1.tar"
(cd "$2" && sha256sum "brrdfeeder-engine-$1.tar" > "brrdfeeder-engine-$1.tar.sha256")
''')
        console=self.work/'Component/brrdhouse';console.mkdir(parents=True)
        shutil.copyfile(ROOT/'Component/brrdhouse/Containerfile',console/'Containerfile')
        (console/'build.sh').write_text('printf console-fixture > Component/brrdhouse/brrdhouse-arm64.oci.tar\n')
        host=b'synthetic-host-helper'
        install=self.work/'Component/brrdfeeder/install';install.mkdir(parents=True)
        (install/'brrdfeeder-install.sh').write_text('readonly RELEASE_HELPER_SHA256="'+hashlib.sha256(host).hexdigest()+'"\n')
        # The fixture models a tag push unless a test overrides it. Never let
        # the event that launched the outer test runner select fixture behavior.
        host_env = {name: value for name, value in os.environ.items()
                    if not name.startswith(HOST_ENV_PREFIXES)}
        self.env=dict(host_env,PATH=str(binary)+':'+os.environ['PATH'],TRACE=str(self.trace),
            GITHUB_REPOSITORY='cybrrd/brrdfeeder',GITHUB_REF='refs/tags/v1.2.3',GITHUB_REF_NAME='v1.2.3',
            GITHUB_EVENT_NAME='push',GITHUB_ACTOR='fixture',REGISTRY_TOKEN='not-a-secret',RELEASE_APPROVAL_CONFIGURED='true',
            GITHUB_RUN_ID='12345',GITHUB_RUN_ATTEMPT='1',
            GITHUB_WORKFLOW_REF='cybrrd/brrdfeeder/.github/workflows/release.yml@refs/tags/v1.2.3',
            GH_TOKEN='fixture-token',
            RUNNER_TEMP=str(self.work/'out'),GITHUB_STEP_SUMMARY=str(self.work/'summary'),GITHUB_OUTPUT=str(self.work/'outputs'),
            GIT_AUTHOR_NAME='Development Team',GIT_COMMITTER_NAME='Development Team',GIT_AUTHOR_EMAIL='operator@cybrrd.com',GIT_COMMITTER_EMAIL='operator@cybrrd.com',
            ENGINE_ATTESTATION_URL='https://github.com/cybrrd/brrdfeeder/attestations/123',
            CONSOLE_ATTESTATION_URL='https://github.com/cybrrd/brrdfeeder/attestations/456',PYTHONDONTWRITEBYTECODE='1')
        for args in [['init','-b','main'],['add','.'],['-c','commit.gpgsign=false','commit','-m','Synthetic release fixture'],['tag','v1.2.3']]:
            subprocess.run(['git',*args],cwd=self.work,env=self.env,check=True,capture_output=True)
        self.env['GITHUB_SHA']=subprocess.check_output(['git','rev-parse','HEAD'],cwd=self.work,text=True).strip()
        folder=self.work/'out/host-updater';folder.mkdir(parents=True)
        (folder/'brrdfeeder-release-arm64').write_bytes(host)
        (folder/'GO-LICENSE').write_text('Synthetic fixture license')
        return self

    def __exit__(self,*args):
        self.tmp.cleanup()

    def run(self,script,**env):
        return subprocess.run(['python3' if script.endswith('.py') else 'bash','.github/scripts/'+script],
            cwd=self.work,env=dict(self.env,**env),text=True,capture_output=True,timeout=30)

    def build(self,component,**env):
        return self.run('release-image.sh',COMPONENT=component,**env)

    def build_both(self):
        for component in ('engine','console'):
            result=self.build(component)
            if result.returncode: raise AssertionError(result.stdout+result.stderr)

    def calls(self):
        return [json.loads(line) for line in self.trace.read_text().splitlines()] if self.trace.exists() else []

    def writes(self):
        return [(name,args) for name,args in self.calls() if name in ('cosign','gh') or name=='skopeo' and args[0] in ('login','copy')]
