#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Real embedded logger + parser + shell helpers; no install/host mutations.

Run with optional git revision (e.g. c4c884b) and evidence destination.
Effectors are synthetic; logging, events, progress and parsing are not mocked.
Only environment inventory and log destination are replaced. No network calls.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[5]
REV = sys.argv[1] if len(sys.argv) > 1 else 'working'
OUT = Path(sys.argv[2]) if len(sys.argv) > 2 else ROOT/'Component/brrdfeeder/install/tests/oneliner-verbose/evidence'/REV
OUT.mkdir(parents=True, exist_ok=True)
def source(name):
    path = 'Component/brrdfeeder/install/'+name
    return ((ROOT/path).read_text() if REV == 'working' else
            subprocess.check_output(['git', 'show', REV+':'+path], cwd=ROOT, text=True))

script = source('brrdfeeder-install.sh')
logger = script.split("<<'INSTALL_LOG_PY'\n", 1)[1].split('\nINSTALL_LOG_PY', 1)[0]
assert logger+'\n' == source('install-log.py')
normal = script[script.index('case "${1:-}" in'):script.index('# brrdfeeder-install.sh —')]
parser = script[script.index('DRY_RUN=0\n'):script.index('\nif [[ $STATUS_ONLY -eq 1 ]]')]
helpers = '\n'.join(re.findall(r'^(?:say|ok|warn|fatal|gate)\(\).*$', script, re.M))
rows = []
with tempfile.TemporaryDirectory(prefix='verbose-proof-') as tmp:
    tmp = Path(tmp)
    runner = tmp/'runner.py'
    runner.write_text("import sys\nfrom pathlib import Path\n"+
        "scope={'__name__':'fixture'}\nexec("+repr(logger)+",scope)\n"+
        "scope['LOG_DIR']=Path(sys.argv[1]);scope['environment']=lambda **kw:'offline fixture'\n"+
        "sys.exit(scope['supervise'](sys.argv[2],sys.argv[3:]))\n")
    for mode in ('install', 'uninstall'):
        for quiet in (False, True):
            for failure in (False, True):
                name = mode+('-quiet' if quiet else '-default')+('-failure' if failure else '')
                logs = tmp/name
                child = tmp/(name+'.sh')
                child.write_text('#!/bin/bash\nset -euo pipefail\n'+normal+'\n'+parser+'\n'+
                    source('log-events.sh')+'\n'+helpers+'\n'+
                    ("log_event NOTICE 'Removing BRRDfeeder…'\n" if mode=='uninstall' else '')+
                    "gate fixture 'Fixture phase'\nsay ROUTINE_SAY\nok ROUTINE_OK\n"+
                    "run_step fixture/command bash -c 'echo ROUTINE_COMMAND'\n"+
                    "warn VISIBLE_WARNING\n"+
                    ("fatal VISIBLE_ERROR\n" if failure else
                     "sleep 4.5\ngate enrollment 'Enrollment display'\n"+
                     "log_secret device-code 'FIXTURE-CODE-9172'\nsay 'Device Flow: FIXTURE-CODE-9172'\n"+
                     "gate complete Complete\nsay VISIBLE_COMPLETION\n"))
                args = (['uninstall'] if mode=='uninstall' else [])+(['--no-verbose'] if quiet else [])
                # Normalize as the real entrypoint does BEFORE supervision.
                if mode=='uninstall': args[0]='--uninstall'
                result = subprocess.run([sys.executable, str(runner), str(logs), str(child), *args],
                    text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=20,
                    start_new_session=True, env={**os.environ, 'BRRDFEEDER_RUN_ID':'1234abcd', 'PYTHONDONTWRITEBYTECODE':'1'})
                transcript = result.stdout
                files = list(logs.glob('*.log'))
                log = '\n'.join(p.read_text() for p in files)
                checks = dict(exit=result.returncode == (1 if failure else 0),
                    routine_terminal=all((s not in transcript if quiet or mode=='uninstall' else s in transcript) for s in ('ROUTINE_SAY','ROUTINE_OK','ROUTINE_COMMAND')),
                    routine_log=all(s in log for s in ('ROUTINE_SAY','ROUTINE_OK','ROUTINE_COMMAND')),
                    ack=('Removing BRRDfeeder…' if mode=='uninstall' else 'Installing BRRDfeeder (usually about') in transcript,
                    phases='Fixture phase …' in transcript and ('Fixture phase — failed' if failure else 'Fixture phase — done') in transcript,
                    warning='VISIBLE_WARNING' in transcript, final='Log:' in transcript and 'run 1234abcd' in transcript)
                if failure:
                    checks.update(error='VISIBLE_ERROR' in transcript, failed='FAILED at fixture/check' in transcript)
                else:
                    checks.update(heartbeat='still working' in transcript, completion='VISIBLE_COMPLETION' in transcript,
                        enrollment='FIXTURE-CODE-9172' in transcript,
                        redacted='FIXTURE-CODE-9172' not in log and '[REDACTED:device-code]' in log)
                (OUT/(name+'.txt')).write_text(transcript)
                (OUT/(name+'.log')).write_text(log)
                rows.append(dict(name=name, exit=result.returncode, checks=checks, passed=all(checks.values())))
                print(name, checks, flush=True)
    # Bare local-command normalization and parser acceptance without effectors.
    for args in (['uninstall','--no-verbose'], ['--no-verbose'], ['--uninstall','--no-verbose']):
        r = subprocess.run(['bash','-c','set -eu\n'+normal+'\n'+parser+'\necho ACCEPTED', 'brrdfeeder', *args], capture_output=True, text=True)
        rows.append(dict(name='parser '+repr(args), passed=r.returncode==0 and 'ACCEPTED' in r.stdout, exit=r.returncode))

receipt = dict(revision=REV, native_pi_runs=0, source_sha256=hashlib.sha256(script.encode()).hexdigest(),
               cases=rows, result='PASS' if all(r['passed'] for r in rows) else 'FAIL')
(OUT/'receipt.json').write_text(json.dumps(receipt, indent=2)+'\n')
sys.exit(receipt['result'] != 'PASS')
