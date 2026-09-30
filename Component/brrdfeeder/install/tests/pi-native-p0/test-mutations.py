#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Negative controls in disposable copies/in-memory modules; no hardware calls."""
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[4]
OUT=Path(os.environ.get('D44_EVIDENCE_DIR',str(HERE/'evidence')))
OUT.mkdir(parents=True,exist_ok=True)
cases=[]
for label,old,new,test in [
    ('ble-enable','dict(enabled=True, unblock_rfkill=True, adapter=dict(usb_id=SUPPORTED))','dict(enabled=False, unblock_rfkill=True, adapter=dict(usb_id=SUPPORTED))','BLE.test_rendered_config_present_absent_and_explicit'),
    ('receipt-unit',"value['unit'] != UNIT",'False','BLE.test_receipt_and_config_refuse_before_mutation'),
    ('restore-active',"if prior['active']: ctl('start', UNIT)",'if False: ctl("start", UNIT)','BLE.test_state_matrix_and_missing_receipt'),
    ('engine-owner',"engine != 'inactive'",'False','Controller.test_live_owners_never_open_socket'),
    ('down-flags','flags & 1','False','Controller.test_still_up_read_failure_and_identity_change_refuse'),
]:
    spec=importlib.util.spec_from_file_location('mutant_test',HERE/'test-ble.py')
    module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    source=(ROOT/'Component/brrdfeeder/install/bluetooth-state.py').read_text()
    assert source.count(old)==1,(label,source.count(old))
    exec(compile(source.replace(old,new),'<mutant-'+label+'>','exec'),module.ble.__dict__)
    stream=io.StringIO()
    result=unittest.TextTestRunner(stream=stream,verbosity=2).run(unittest.defaultTestLoader.loadTestsFromName(test,module))
    (OUT/('mutation-'+label+'.log')).write_text(stream.getvalue())
    cases.append(dict(control=label,detected=bool(result.failures) and not result.errors,tests=result.testsRun))

with tempfile.TemporaryDirectory(prefix='p0-gps-mutation-') as tmp:
    copy=Path(tmp)/'console'; shutil.copytree(ROOT/'Component/brrdhouse',copy)
    path=copy/'gps.go'; source=path.read_text()
    assert source.count('return "No fix"')==1
    path.write_text(source.replace('return "No fix"','return "Good"'))
    result=subprocess.run(['go','test','-run','TestGPSRatingPolicy','-count=1','.'],cwd=copy,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120)
    (OUT/'mutation-gps-rating.log').write_text(result.stdout)
    cases.append(dict(control='gps-no-fix',detected=result.returncode!=0 and '--- FAIL:' in result.stdout and 'build failed' not in result.stdout,exit=result.returncode))
(OUT/'mutations.json').write_text(json.dumps(dict(cases=cases,source_mutated=False),indent=2)+'\n')
print(json.dumps(cases,indent=2))
raise SystemExit(not all(c['detected'] for c in cases))
