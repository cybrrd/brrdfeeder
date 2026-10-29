#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Compile real Rust config mutants in an isolated source copy; no hardware."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[5]
with tempfile.TemporaryDirectory(prefix='ble-config-mutants-') as tmp:
    root = Path(tmp)
    # Copy tracked source bytes from the working tree, including uncommitted fixes.
    for name in subprocess.check_output(['git','ls-files','-z'],cwd=ROOT).decode().split('\0'):
        if not name: continue
        src = ROOT/name
        if src.is_file():
            dst = root/name
            dst.parent.mkdir(parents=True,exist_ok=True)
            shutil.copyfile(src,dst)
    out = Path(os.environ.get('D44_EVIDENCE_DIR', str(root/'evidence')))
    out.mkdir(parents=True,exist_ok=True)
    env = dict(os.environ)
    env.setdefault('CARGO_TARGET_DIR',str(ROOT/'Component/aviary/target'))
    command = ['cargo','test','--offline','--locked','-p','engine','hotplug_','--','--nocapture']
    def run(label):
        result = subprocess.run(command,cwd=root/'Component/aviary',env=env,
                                capture_output=True,text=True,timeout=900)
        text = result.stdout+result.stderr
        (out/(label+'.log')).write_text(text)
        return result.returncode,text
    code,text = run('positive')
    assert code == 0,text
    print('Rust positive: hotplug lifecycle/config tests PASS',flush=True)
    path = root/'Component/aviary/engine/src/node_config.rs'
    original = path.read_text()
    controls = [
        ('absent-disabled','pub fn absent() -> Self {','enabled: true,','enabled: false,','hotplug_absent_section_enables_auto_and_targeted_unblock'),
        ('explicit-unblock','impl Default for RidBleYaml {','unblock_rfkill: false,','unblock_rfkill: true,','hotplug_explicit_section_preserves_legacy_defaults_and_policies'),
        ('explicit-enable','impl Default for RidBleYaml {','enabled: false,','enabled: true,','hotplug_explicit_section_preserves_legacy_defaults_and_policies'),
    ]
    records = []
    for label,anchor,before,after,test in controls:
        index = original.index(anchor)
        assert before in original[index:]
        path.write_text(original[:index]+original[index:].replace(before,after,1))
        code,text = run(label)
        path.write_text(original)
        killed = code != 0 and test+' ... FAILED' in text and 'test result: FAILED' in text
        records.append(dict(control=label,killed=killed,exit=code))
        assert killed,label+' survived or failed before assertions: '+text
        print(label+': KILLED',flush=True)
    (out/'results.json').write_text(json.dumps(records,indent=2)+'\n')
