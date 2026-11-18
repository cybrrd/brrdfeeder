#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Require prompt regressions to fail assertions in disposable source copies."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[5]
INSTALL = 'Component/brrdfeeder/install/brrdfeeder-install.sh'
RECOVERY = 'Component/aviary/engine/src/credential_renewal.rs'
COMPARE = 'Approve ONLY if the code shown in the browser exactly matches the code shown on this node.'
mutants = [
    ('installer-prefilled-first', INSTALL, '.verification_uri // .verification_uri_complete', '.verification_uri_complete // .verification_uri'),
    ('installer-no-fallback', INSTALL, '.verification_uri // .verification_uri_complete', '.verification_uri'),
    ('installer-no-comparison', INSTALL, COMPARE, 'Approve to continue.'),
    ('installer-no-warning', INSTALL, 'If you did not start this install, do not approve.', 'Continue with setup.'),
    ('recovery-prefilled-first', RECOVERY, 'let uri = auth["verification_uri"]\n        .as_str()\n        .or(auth["verification_uri_complete"].as_str())', 'let uri = auth["verification_uri_complete"]\n        .as_str()\n        .or(auth["verification_uri"].as_str())'),
    ('recovery-no-fallback', RECOVERY, '.or(auth["verification_uri_complete"].as_str())', ''),
    ('recovery-no-comparison', RECOVERY, COMPARE, 'Approve to continue.'),
    ('recovery-no-warning', RECOVERY, 'If you did not start this recovery, do not approve.', 'Continue with recovery.'),
]
with tempfile.TemporaryDirectory(prefix='device-prompt-mutants-') as tmp:
    root = Path(tmp)
    for name in subprocess.check_output(['git','ls-files','-z'],cwd=ROOT).decode().split('\0'):
        if not name: continue
        src = ROOT/name
        if src.is_file():
            dst = root/name; dst.parent.mkdir(parents=True,exist_ok=True); shutil.copyfile(src,dst)
    out = Path(os.environ.get('D44_EVIDENCE_DIR',str(root/'evidence')))
    out.mkdir(parents=True,exist_ok=True)
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    env.setdefault('CARGO_TARGET_DIR',str(ROOT/'Component/aviary/target'))
    def run(name, rust=False):
        command = (['cargo','test','--offline','--locked','-p','engine','device_prompt','--','--nocapture'] if rust
                   else ['python3',str(root/'Component/brrdfeeder/install/tests/device-code-confirm/test-contract.py')])
        result = subprocess.run(command,cwd=root/'Component/aviary',env=env,capture_output=True,text=True,timeout=900)
        text = result.stdout+result.stderr; (out/(name+'.log')).write_text(text)
        return result.returncode,text
    for rust in (False,True):
        code,text = run('recovery-positive' if rust else 'installer-positive',rust)
        assert code == 0,text
    results=[]
    for name,file,before,after in mutants:
        path=root/file; original=path.read_text(); assert original.count(before)==1,name
        path.write_text(original.replace(before,after))
        try: code,text=run(name,file==RECOVERY)
        finally: path.write_text(original)
        killed = code != 0 and ('test result: FAILED' in text if file==RECOVERY else 'FAIL:' in text)
        assert killed,name+' survived or failed before assertions: '+text
        results.append(dict(control=name,killed=killed,exit=code))
        print(name+': KILLED',flush=True)
    (out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
