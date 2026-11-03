#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Assemble a checksum-bound, UNBOUND native kit offline; never contact a host."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess

HERE = Path(__file__).resolve().parent
RELEASE = HERE.parents[1]
ROOT = RELEASE.parents[2]
BASE = '9faae55f53e27cea198401ee34b4c66c1898666a'
INSTALL = 'Component/brrdfeeder/install/brrdfeeder-install.sh'

def digest(raw):
    return hashlib.sha256(raw).hexdigest()

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--helper',required=True,type=Path)
    p.add_argument('--output',required=True,type=Path)
    a=p.parse_args()
    candidate=a.helper.read_bytes()
    installer=(ROOT/INSTALL).read_bytes()
    pin=re.search(rb'^readonly RELEASE_HELPER_SHA256="([a-f0-9]{64})"',installer,re.M).group(1).decode()
    if digest(candidate)!=pin or candidate[:6]!=b'\x7fELF\x02\x01' or candidate[18:20]!=b'\xb7\x00':
        raise ValueError('candidate must match the current installer ARM64 pin')
    old=subprocess.check_output(['git','show',BASE+':'+INSTALL],cwd=ROOT)
    oldpin=re.search(rb'^readonly RELEASE_HELPER_SHA256="([a-f0-9]{64})"',old,re.M).group(1).decode()
    a.output.mkdir(mode=0o700)  # do not overwrite an earlier kit/registration
    for name in ('README.md','binding.py','fixture.py','helper-preimages.py','gdb-fsync.py',
                 'operator-functions.sh','calibrate.c','validate.py','test_validator.py','test_preimages.py'):
        shutil.copyfile(HERE/name,a.output/name)
    (a.output/'helper5').write_bytes(candidate)
    (a.output/'helper5').chmod(0o755)
    (a.output/'install.sh').write_bytes(installer)
    (a.output/'0.9.0-install.sh').write_bytes(old)
    shutil.copyfile(RELEASE/'launcher.py',a.output/'launcher5.py')
    shutil.copyfile(ROOT/'LICENSES/AGPL-3.0-or-later.txt',a.output/'AGPL-3.0-or-later.txt')
    goroot=subprocess.check_output(['go','env','GOROOT'],text=True).strip()
    shutil.copyfile(Path(goroot)/'LICENSE',a.output/'GO-LICENSE')
    b=dict(from_build=4,to_build=5,baseline_version='0.9.0',baseline_source_commit=BASE,
           source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
           baseline_helper_sha256=oldpin)
    for field in ('hostname','node_id','operator','window','oob','physical_asset'): b[field]=None
    for name,field in [('helper5','candidate_sha256'),('install.sh','installer_sha256'),
                       ('0.9.0-install.sh','baseline_installer_sha256'),('launcher5.py','launcher_sha256'),
                       ('helper-preimages.py','preimages_tool_sha256'),('fixture.py','adapter_sha256'),
                       ('gdb-fsync.py','gdb_driver_sha256')]:
        b[field]=digest((a.output/name).read_bytes())
    (a.output/'binding.template.json').write_text(json.dumps(b,indent=2)+'\n')
    status=subprocess.check_output(['git','status','--porcelain'],cwd=ROOT,text=True)
    provenance=dict(status='UNBOUND_NATIVE_NOT_RUN',source_commit=b['source_commit'],
                    dirty=bool(status),diff_sha256=digest(subprocess.check_output(['git','diff','HEAD','--binary'],cwd=ROOT)),
                    compiler=subprocess.check_output(['go','version'],text=True).strip())
    (a.output/'SOURCE.json').write_text(json.dumps(provenance,indent=2)+'\n')
    (a.output/'SHA256SUMS').write_text(''.join(digest(p.read_bytes())+'  '+p.name+'\n'
                                            for p in sorted(a.output.iterdir()) if p.is_file()))
    print('UNBOUND_NATIVE_NOT_RUN '+str(a.output))

if __name__=='__main__': main()
