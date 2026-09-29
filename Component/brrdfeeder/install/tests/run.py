#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Identical public/private offline gate; only isolated fixtures execute effectors.

Provision dependencies before running. No test contacts production, signs, or
publishes. Logs are written outside the source tree; --out retains local receipts.
The containers group requires a prebuilt disposable OS image (TEST_OS_IMAGE).
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
INSTALL = ROOT/'Component/brrdfeeder/install'
RELEASE = ROOT/'Component/aviary/release-system'
BASELINES = [
    'ownership/test-contract.py',
    'github-scaffold/test-contract.py',
    'github-scaffold/test-release.py',
    'build-sequence/test-contract.py',
    'public-ci/test-contract.py',
    'package-console/test-package.py',
    'uninstall/test-contract.py',
    'install-log/test-contract.py',
    'gps-position/test-contract.py',
    'brrdfeeder-naming/test-contract.py',
    'pi-native-p0/test-contract.py',
    'pi-native-p0/test-ble.py',
    'pi-native-p0/test-gps-diagnostics.py',
    'pi-native-p0/test-gps-hotplug.py',
    'ble-rfkill/test-contract.py',
    'oneliner-A/test-contract.py',
    'simple-uninstall/test-contract.py',
    'simple-uninstall/test-behavior.py',
    'installer-vcgencmd/test-safety.py',
    'installer-vcgencmd/test-contract.py',
    'd44-rework/test-friday.py',
    'self-update-2026-09-29/test-integration.py',
]
MUTATIONS = [
    'github-scaffold/test-mutations.py',
    'source-date/test-mutations.py',
    'simple-uninstall/test-mutations.py',
    'pi-native-p0/test-mutations.py',
    'ble-rfkill/run-proof.py',
]
GROUPS = ('installer', 'go', 'rust', 'mutations', 'containers')
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--group', choices=(*GROUPS, 'all'), default='all')
p.add_argument('--out', type=Path)
p.add_argument('--list', action='store_true')
args = p.parse_args()
temporary = tempfile.TemporaryDirectory(prefix='brrd-contracts-')
out = (args.out or Path(temporary.name)).resolve()
out.mkdir(parents=True, exist_ok=True)
records = []
env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', GOPROXY='off', GOSUMDB='off', GOTOOLCHAIN='local')
env.pop('D44_PODMAN', None)
env['P0_EVIDENCE'] = str(out/'pi')
env['INSTALLER_PROOF_OUT'] = str(out/'screens')
Path(env['P0_EVIDENCE']).mkdir(exist_ok=True)

def run(name, command, cwd=ROOT, timeout=1800):
    if args.list:
        print(name+': '+' '.join(map(str,command)))
        return
    current = dict(env, D44_EVIDENCE_DIR=str(out/name))
    start = time.monotonic()
    try:
        r = subprocess.run(list(map(str,command)), cwd=cwd, env=current,
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                           text=True, timeout=timeout)
        output, code = r.stdout, r.returncode
    except subprocess.TimeoutExpired as e:
        output = e.stdout or b''
        if isinstance(output, bytes): output = output.decode(errors='replace')
        output += '\nHARNESS TIMEOUT\n'
        code = 124
    (out/(name+'.log')).write_text(output)
    skips = re.findall(r'^--- SKIP: (\S+)|^test .* \.\.\. ignored|^.* \.\.\. skipped (.*)', output, re.M)
    # Native Rust has two explicitly broker-dependent ignored tests; container
    # integration is opt-in in Go. Neither is misreported as an offline pass.
    row = dict(name=name, exit=code, seconds=round(time.monotonic()-start,2),
               python_tests=[int(n) for n in re.findall(r'Ran (\d+) tests?',output)],
               go_passes=len(re.findall(r'^--- PASS:',output,re.M)),
               skips=skips, rust_results=re.findall(r'^test result:.*',output,re.M))
    records.append(row)
    (out/'summary.json').write_text(json.dumps(records,indent=2)+'\n')
    print(name+': '+('PASS' if code == 0 else 'FAIL')+' ('+str(row['seconds'])+'s)',flush=True)
    if code: print(output[-3000:], flush=True)

groups = GROUPS if args.group == 'all' else (args.group,)
for group in groups:
    if group == 'installer':
        run('reuse-lint',[sys.executable,HERE/'ownership/check-reuse.py'],timeout=240)
        run('actionlint', ['actionlint', '-shellcheck=', '-pyflakes=', *sorted((ROOT/'.github/workflows').glob('*.yml'))])
        for path in sorted((ROOT/'.github/scripts').glob('*.sh')):
            run('syntax-github-'+path.stem, ['bash', '-n', path], timeout=30)
        for name in BASELINES:
            run(name.replace('/','-').removesuffix('.py'),[sys.executable,HERE/name],timeout=240)
        run('launcher',[sys.executable,'launcher_test.py'],RELEASE,240)
        for name in ['test-image-identity.py','test-build-arm64.py']:
            run(name.removesuffix('.py'),[sys.executable,ROOT/'Component/aviary/tools'/name])
        run('bt-dongle-gate',['bash',ROOT/'Component/aviary/tools/test-bt-dongle-gate.sh'])
        run('quadlet-cidfile',[sys.executable,ROOT/'Component/aviary/tools/check-quadlet-cidfile.py'])
        run('verbose',[sys.executable,HERE/'oneliner-verbose/proof.py','working',out/'verbose'])
        for path in sorted((ROOT/'Component').rglob('*.sh')):
            if (path.is_relative_to(ROOT/'Component/aviary') or path.is_relative_to(ROOT/'Component/brrdfeeder') or path.is_relative_to(ROOT/'Component/brrdhouse')) and not any(part in path.parts for part in ('target','.git')):
                run('syntax-'+str(path.relative_to(ROOT)).replace('/','-'),['bash','-n',path],timeout=30)
    elif group == 'go':
        for name,folder in [('console',ROOT/'Component/brrdhouse'),('release',RELEASE)]:
            run(name,['go','test','-race','-count=1','-v','./...'],folder)
            run(name+'-vet',['go','vet','./...'],folder)
    elif group == 'rust':
        run('rust-workspace',['cargo','test','--offline','--locked','--workspace','--','--nocapture'],
            ROOT/'Component/aviary',3600)
    elif group == 'mutations':
        for name in MUTATIONS:
            run(name.replace('/','-').removesuffix('.py'),[sys.executable,HERE/name])
        for name in ('run-go-proofs','run-rework-proofs','run-rust-proofs'):
            run(name,[sys.executable,RELEASE/'tests'/(name+'.py')],timeout=3600)
    elif group == 'containers':
        run('ownership-image-packaging',[sys.executable,HERE/'ownership/check-images.py'])
        image = os.environ.get('TEST_OS_IMAGE')
        if not image:
            p.error('containers requires TEST_OS_IMAGE: an immutable, prebuilt disposable OS fixture')
        base = ['podman','run','--rm','--pull=never','--network=none','-v',str(ROOT)+':/repo:ro',
                '-e','PYTHONDONTWRITEBYTECODE=1']
        run('uninstall-wtmpdb',base+[image,'python3','/repo/Component/brrdfeeder/install/tests/uninstall-wtmpdb/test-history.py'])
        run('naming-compat',base+['-e','BRRD_NAMING_CONTAINER=1','-e','NAMING_ROOT=/repo',
            image,'python3','/repo/Component/brrdfeeder/install/tests/brrdfeeder-naming/container-tests.py'])
        run('release-real-podman',[sys.executable,RELEASE/'tests/run-runtime-proof.py'])
        run('rust-local-broker',[sys.executable,ROOT/'Component/aviary/engine/tests/run-broker-tests.py'])
        for name,guard in [('installer-quadlet','D12_INSTALLER_TEST_CONTAINER'),
                           ('installer-storage-class','D8_INSTALLER_TEST_CONTAINER'),
                           ('rfkill-boot-state','D8_INSTALLER_TEST_CONTAINER'),
                           ('blackbox-flush','D8_INSTALLER_TEST_CONTAINER')]:
            run(name,base+['-e',guard+'=1',image,'bash','/repo/Component/aviary/tools/test-'+name+'.sh'])
sys.exit(any(r['exit'] for r in records))
