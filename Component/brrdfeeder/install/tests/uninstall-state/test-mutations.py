#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Clean control before EVERY mutation; removal runs only in an offline OS."""
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[4]
INSTALL=ROOT/'Component/brrdfeeder/install'

# Each acceptance rule is independently removed from the actual embedded
# validator. Nothing is written to product source; mutated scripts live in /tmp
# inside the disposable OS and execute the complete uninstall entrypoint.
RULES=[
    ('public-attempt', "if path == STATE / 'release_attempt.json':", "if path == STATE / 'missing-release-attempt.json':"),
    ('private-attempt', "elif path == PRIVATE / 'attempt.json':", "elif path == PRIVATE / 'missing-attempt.json':"),
    ('credentials-directory', "elif path == STATE / 'credentials':", "elif path == STATE / 'missing-credentials':"),
    ('active-credential', "('brrdfeeder.creds', 'previous.creds', '.renew.lock')", "('previous.creds', '.renew.lock')"),
    ('previous-credential', "('brrdfeeder.creds', 'previous.creds', '.renew.lock')", "('brrdfeeder.creds', '.renew.lock')"),
    ('renewal-lock', "('brrdfeeder.creds', 'previous.creds', '.renew.lock')", "('brrdfeeder.creds', 'previous.creds')"),
    ('renewal-temporary', "r'\\.renew-U[A-Z2-7]{55}'", "r'\\.missing-U[A-Z2-7]{55}'"),
    ('capture-directory', "elif path == STATE / 'capture':", "elif path == STATE / 'missing-capture':"),
    ('capture-slot', "r'capture-(0[0-9]|1[0-5])\\.pcap'", "r'capture-(1[0-5])\\.pcap'"),
    ('legacy-status', "(name == 'status.json' or", "(False or"),
    ('legacy-status-temporary', "r'status\\.json\\.tmp\\.[1-9][0-9]*\\.(0|[1-9][0-9]*)'", "r'missing\\.json\\.tmp\\.[1-9][0-9]*\\.(0|[1-9][0-9]*)'"),
    ('pending-target', "(name == 'pending_update.json' or go_temporary", "(False or go_temporary"),
    ('pending-temporary', "or go_temporary(name, '.pending-')", "or False"),
    ('pending-internal-hardlink-pair', 'links=(1, 2)', 'links=(1,)'),
    ('private-recovery-temporary', 'path.parent in (PRIVATE, CONFIG) and python_temporary', 'path.parent == CONFIG and python_temporary'),
    ('config-recovery-temporary', 'path.parent in (PRIVATE, CONFIG) and python_temporary', 'path.parent == PRIVATE and python_temporary'),
    ('config-go-temporary', "go_temporary(name, '.update-')", "go_temporary(name, '.missing-')"),
    ('config-gps-temporary', "python_temporary(name, '.config.yaml-')", "python_temporary(name, '.missing.yaml-')"),
]


def inside():
    from fixture import Fixture,guard
    guard()
    records=[]
    for name,old,new in RULES:
        fixture=Fixture()
        fixture.setUp()
        try:
            fixture.seed(optional=True,crash=True)
            fixture.positive()  # must pass, immediately before this mutant
            original=fixture.helper
            assert original.count(old)==1,(name,original.count(old))
            fixture.helper=original.replace(old,new)
            fixture.seed(optional=True,crash=True)
            before=fixture.snapshot()
            result=fixture.uninstall(dry=True)
            assert result.returncode==1 and 'REFUSED' in result.stdout+result.stderr,(name,result.stdout,result.stderr)
            assert 'syntax error' not in result.stderr and 'Traceback' not in result.stderr,(name,result.stderr)
            assert fixture.snapshot()==before and not fixture.calls.exists(),name
            # A real apply must fail at the same validation boundary too.
            result=fixture.uninstall()
            assert result.returncode==1 and 'REFUSED' in result.stdout+result.stderr,(name,result.stdout,result.stderr)
            assert fixture.snapshot()==before and not fixture.calls.exists(),name
            records.append(dict(name=name,clean_control=True,detected=True,dry_and_apply=True))
            print('MUTATION '+name+': clean control PASS; dry/apply removal detected',flush=True)
        finally:
            fixture.tearDown()
    print('STATE_MUTATIONS_JSON='+json.dumps(records))
    return records


def load(path,name):
    spec=importlib.util.spec_from_file_location(name,path)
    module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    return module


def catalogue_mutations():
    module=load(INSTALL/'tests/public-ci/test-contract.py','catalogue')
    records=[]
    for name in ('delete-gate','deregister-local-gate','deregister-public-gate'):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder)
            for relative in ('.github/workflows/test.yml','Component/brrdfeeder/install/tests/run.py',
                             'Component/brrdfeeder/install/tests/uninstall-state/check-writers.py',
                             'Component/brrdfeeder/install/tests/uninstall-state/writer-paths.json'):
                target=root/relative; target.parent.mkdir(parents=True,exist_ok=True)
                shutil.copyfile(ROOT/relative,target)
            def run():
                output=io.StringIO()
                with patch.object(module,'ROOT',root),patch.object(module,'TESTS',root/'Component/brrdfeeder/install/tests'):
                    result=unittest.TextTestRunner(stream=output).run(module.Gate('test_uninstall_writer_gate_is_required'))
                return result,output.getvalue()
            control,log=run(); assert control.wasSuccessful(),log
            if name=='delete-gate':
                (root/'Component/brrdfeeder/install/tests/uninstall-state/check-writers.py').unlink()
            elif name=='deregister-local-gate':
                path=root/'Component/brrdfeeder/install/tests/run.py'
                path.write_text('\n'.join(line for line in path.read_text().splitlines() if "run('uninstall-writer-paths'" not in line)+'\n')
            else:
                path=root/'.github/workflows/test.yml'
                path.write_text(path.read_text().replace('python3 Component/brrdfeeder/install/tests/uninstall-state/check-writers.py','true'))
            mutant,log=run()
            assert not mutant.wasSuccessful() and mutant.failures and not mutant.errors,(name,log)
            records.append(dict(name=name,clean_control=True,detected=True))
            print('MUTATION '+name+': clean control PASS; catalogue guard caught removal',flush=True)
    return records


def main():
    if '--inside' in sys.argv:
        inside(); return
    image=os.environ.get('TEST_OS_IMAGE')
    if not image: raise SystemExit('TEST_OS_IMAGE is required; no host filesystem fallback')
    command=['podman','run','--rm','--pull=never','--network=none','-v',str(ROOT)+':/repo:ro',
             '-e','PYTHONDONTWRITEBYTECODE=1','-e','BRRD_UNINSTALL_STATE_CONTAINER=1',image,
             'python3','/repo/Component/brrdfeeder/install/tests/uninstall-state/test-mutations.py','--inside']
    result=subprocess.run(command,capture_output=True,text=True,timeout=900)
    print(result.stdout,end=''); print(result.stderr,end='',file=sys.stderr)
    if result.returncode: raise SystemExit(result.returncode)
    records=json.loads(next(line.split('=',1)[1] for line in result.stdout.splitlines() if line.startswith('STATE_MUTATIONS_JSON=')))
    records+=catalogue_mutations()
    out=Path(os.environ.get('D44_EVIDENCE_DIR',tempfile.mkdtemp(prefix='uninstall-mutations-')))
    out.mkdir(parents=True,exist_ok=True)
    (out/'mutations.json').write_text(json.dumps(records,indent=2)+'\n')
    print(f'Uninstall state mutations: {len(records)}/{len(records)} detected, {len(records)}/{len(records)} immediately preceding controls passed.')


if __name__=='__main__': main()
