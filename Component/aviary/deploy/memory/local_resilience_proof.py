#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Local transient user-unit proof, no nodes and no deliberate OOM.

Starts a private fixture that exits four times, then remains running. Reads
systemctl show and the kernel's actual memory.max; removes both transient units.
This proves host facilities and unit semantics, not deployment on a fleet node.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[3]


def command(args, check=True):
    return subprocess.run(args,check=check,text=True,capture_output=True,timeout=20)


def properties(unit):
    text=command(['systemctl','--user','show',unit,'--property=ControlGroup,MemoryMax,MemorySwapMax,OOMPolicy,Restart,RestartUSec,StartLimitIntervalUSec,NRestarts,ActiveState,SubState,Result']).stdout
    return dict(line.split('=',1) for line in text.splitlines())


def payload(root):
    count_file=root/'starts'
    count=int(count_file.read_text())+1 if count_file.exists() else 1
    count_file.write_text(str(count))
    if count<=4:
        raise SystemExit(1)
    time.sleep(60)


def proof(output):
    output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='b06-resilience-') as folder:
        root=Path(folder)
        unit='b06-resilience-test-'+str(os.getpid())+'.service'
        updater='b06-updater-test-'+str(os.getpid())+'.service'
        engine=(HERE.parent/'quadlet/brrdfeeder-engine.container').read_text()
        updater_source=(ROOT/'Component/aviary/release-system/units/brrdfeeder-updater.service').read_text()
        required=('MemoryAccounting=yes','MemoryMax=256M','MemorySwapMax=0','OOMPolicy=kill','Restart=always','RestartSec=5','StartLimitIntervalSec=0')
        assert all(line in engine.splitlines() for line in required)
        assert 'StartLimitIntervalSec=0' in updater_source.splitlines()
        assert 'Type=oneshot' in updater_source.splitlines()
        try:
            command(['systemd-run','--user','--unit='+unit,'--description=TEST ONLY B06 bounded memory and four restart failures',
                     *('--property='+line for line in required),sys.executable,str(Path(__file__).resolve()),'payload',str(root)])
            end=time.monotonic()+45
            while True:
                props=properties(unit)
                if (int(props['NRestarts'])>=4 and props['SubState']=='running'
                        and (root/'starts').exists() and int((root/'starts').read_text())>=5):break
                assert time.monotonic()<end,props
                time.sleep(.2)
            assert props['MemoryMax']=='268435456' and props['MemorySwapMax']=='0',props
            assert props['Restart']=='always' and props['StartLimitIntervalUSec']=='0',props
            group=Path('/sys/fs/cgroup')/props['ControlGroup'].lstrip('/')
            assert (group/'memory.max').read_text().strip()=='268435456'
            assert (group/'memory.swap.max').read_text().strip()=='0'
            assert 'memory' in Path('/sys/fs/cgroup/cgroup.controllers').read_text().split()
            assert int((root/'starts').read_text())>=5
            command(['systemd-run','--user','--unit='+updater,'--description=TEST ONLY B06 repeated oneshot failure',
                     '--property=Type=oneshot','--property=StartLimitIntervalSec=0','/bin/false'],check=False)
            for _ in range(5):command(['systemctl','--user','start',updater],check=False)
            update_props=properties(updater)
            assert update_props['StartLimitIntervalUSec']=='0' and update_props['Result']=='exit-code',update_props
            result={'scope':'local transient user units; no node deployment',
                    'engine':props,'kernel_memory_max':268435456,'kernel_memory_swap_max':0,
                    'engine_starts':int((root/'starts').read_text()),'updater':update_props,
                    'pass':True}
            (output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
            print(json.dumps(result,indent=2),flush=True)
        finally:
            for item in (unit,updater):
                command(['systemctl','--user','stop',item],check=False)
                command(['systemctl','--user','reset-failed',item],check=False)


if __name__=='__main__':
    if len(sys.argv)==3 and sys.argv[1]=='payload':payload(Path(sys.argv[2]))
    elif len(sys.argv)==2:proof(Path(sys.argv[1]).resolve())
    else:raise SystemExit('usage: local_resilience_proof.py OUTPUT_DIRECTORY')
