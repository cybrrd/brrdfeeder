#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Instrument an isolated source copy at real I/O calls; never edit product code.

Synthetic error/panic boundaries exercise recovery control flow. This does not
claim native syscall interception, reboot durability or physical power-cut proof.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

HERE=Path(__file__).resolve().parent
SOURCE=HERE.parent
out=Path(os.environ.get('D44_EVIDENCE_DIR',str(HERE/'evidence/helper5-io')))
out.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(prefix='helper5-io-') as directory:
    work=Path(directory)/'source';shutil.copytree(SOURCE,work,ignore=shutil.ignore_patterns('__pycache__','evidence'))
    for filename,start,end,replacements in [
        ('storage.go','func atomicFile(', 'func atomicJSON(', [('f.Write(raw)','proofWrite(f, raw)'),('f.Sync()','proofSync(f)'),('os.Rename(tmp, path)','proofRename(tmp, path)'),('d.Sync()','proofSync(d)')]),
        ('updater.go','func removeOptional(',None,[('os.Remove(path)','proofRemove(path)'),('d.Sync()','proofSync(d)')])]:
        p=work/filename;text=p.read_text();begin=text.index(start);stop=text.index(end,begin) if end else len(text)
        body=text[begin:stop]
        for before,after in replacements:
            assert body.count(before)==1,(filename,before,body.count(before))
            body=body.replace(before,after)
        p.write_text(text[:begin]+body+text[stop:])
    (work/'helper5_io_generated_test.go').write_bytes((HERE/'helper5_io_test.go.in').read_bytes())
    result=subprocess.run(['go','test','-count=1','-v','-run','^TestHelper5IOFaults$'],cwd=work,
                          env=dict(os.environ,GOPROXY='off',GOSUMDB='off',PYTHONDONTWRITEBYTECODE='1'),
                          stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
    (out/'io-faults.log').write_text(result.stdout)
    print(result.stdout[-3000:])
    if result.returncode: raise SystemExit(result.returncode)
    assert 'PASS: TestHelper5IOFaults' in result.stdout and 'injected boundaries=' in result.stdout
