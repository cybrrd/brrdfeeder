#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline source-bound acceptance; evidence only when explicitly requested."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT=Path(__file__).resolve().parents[5]
temporary=tempfile.TemporaryDirectory(prefix='ble-proof-')
out=Path(os.environ['D44_EVIDENCE_DIR']) if 'D44_EVIDENCE_DIR' in os.environ else (ROOT/'Component/brrdfeeder/install/tests/ble-rfkill/evidence' if '--record-evidence' in sys.argv else Path(temporary.name))
out.mkdir(parents=True,exist_ok=True)
receipt=dict(native_pi_runs=0,production_operations=0,result='PASS',commands=[],mutations=[])
tests=['ble-rfkill/test-contract.py','pi-native-p0/test-ble.py','simple-uninstall/test-contract.py',
       'simple-uninstall/test-behavior.py','pi-native-p0/test-contract.py',
       'pi-native-p0/test-gps-diagnostics.py','pi-native-p0/test-gps-hotplug.py',
       'gps-position/test-contract.py','install-log/test-contract.py','uninstall/test-contract.py',
       'package-console/test-package.py']
commands=[[sys.executable,'Component/brrdfeeder/install/tests/'+name] for name in tests]
commands += [['bash','-n','Component/brrdfeeder/install/'+name] for name in ['brrdfeeder-install.sh','uninstall.sh']]
for i,command in enumerate(commands):
    result=subprocess.run(command,cwd=ROOT,env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1'),
                          text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=180)
    name=f'check-{i}.txt';(out/name).write_text(result.stdout)
    receipt['commands'].append(dict(command=command,exit=result.returncode,output=name))
    print(name,result.returncode,flush=True)
    if result.returncode: receipt['result']='FAIL'

cases=[('unblock-default','unblock_rfkill=True','unblock_rfkill=False','Policy.test_new_and_rerun_missing_policy'),
       ('explicit-false',"if 'unblock_rfkill' not in ble:",'if True:','Policy.test_explicit_policies_win'),
       ('stale-green','not 0 <= now-written.timestamp() <= 3*interval','False','Summary.test_freshness_scales_and_missing_is_unknown'),
       ('failed-green','state, detail = match.groups()',"state, detail = match.groups(); state = 'Healthy'",'Summary.test_latest_engine_state_wins_with_reason')]
for name,old,new,test in cases:
    with tempfile.TemporaryDirectory(prefix='ble-mutant-') as tmp:
        dst=Path(tmp);install=dst/'Component/brrdfeeder/install';install.mkdir(parents=True)
        source=(ROOT/'Component/brrdfeeder/install/bluetooth-state.py').read_text()
        assert source.count(old)==1
        (install/'bluetooth-state.py').write_text(source.replace(old,new))
        testsdir=dst/'Component/brrdfeeder/install/tests/ble-rfkill';testsdir.mkdir(parents=True)
        shutil.copy2(ROOT/'Component/brrdfeeder/install/tests/ble-rfkill/test-contract.py',testsdir/'test-contract.py')
        result=subprocess.run([sys.executable,str(testsdir/'test-contract.py'),test],cwd=dst,
                              text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,
                              env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1'),timeout=30)
        caught=result.returncode!=0 and 'FAIL:' in result.stdout
        receipt['mutations'].append(dict(name=name,caught=caught))
        (out/(name+'.txt')).write_text(result.stdout)
        print(name,'CAUGHT' if caught else 'MISSED',flush=True)
        if not caught: receipt['result']='FAIL'
paths=['Component/brrdfeeder/install/'+name for name in ['brrdfeeder-install.sh','bluetooth-state.py','uninstall.sh']]
paths += ['Component/aviary/engine/src/'+name for name in ['node_config.rs','rid_ble.rs','rfkill.rs']]
receipt['sources']={name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in paths}
(out/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
sys.exit(receipt['result']!='PASS')
