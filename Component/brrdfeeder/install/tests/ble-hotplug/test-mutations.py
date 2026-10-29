#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Kill disabled-absence, engine config-write and broad-udev mutations."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
ROOT=Path(__file__).resolve().parents[5]
paths=['Component/brrdfeeder/install/'+p for p in ['bluetooth-state.py','install-log.py','brrdfeeder-install.sh','uninstall.sh','tests/ble-hotplug/test-contract.py','tests/pi-native-p0/test-ble.py']]
paths += ['Component/aviary/'+p for p in ['engine/src/rid_ble.rs','deploy/udev/99-cybrrd-brrdfeeder.rules','deploy/quadlet/brrdfeeder-engine.container','deploy/quadlet/brrdfeeder-ble-takeover.service','deploy/quadlet/brrdfeeder-ble-takeover.timer']]
mutants=[
    ('absent-disabled','Component/brrdfeeder/install/bluetooth-state.py',"enabled = ble.get('enabled', True)","enabled = ble.get('enabled', False)"),
    ('engine-config-write','Component/aviary/engine/src/rid_ble.rs','const HCI_SYSFS:', 'fn forbidden_write() { std::fs::write("/etc/brrdfeeder/config.yaml", "").unwrap(); }\nconst HCI_SYSFS:'),
    ('udev-other-id','Component/aviary/deploy/udev/99-cybrrd-brrdfeeder.rules','ATTRS{idProduct}=="876e", TAG','ATTRS{idProduct}=="*", TAG'),
]
with tempfile.TemporaryDirectory(prefix='ble-hotplug-mutants-') as folder:
    root=Path(folder)
    for name in paths:
        p=root/name; p.parent.mkdir(parents=True,exist_ok=True); shutil.copyfile(ROOT/name,p)
    test=[sys.executable,str(root/'Component/brrdfeeder/install/tests/ble-hotplug/test-contract.py')]
    positive=subprocess.run(test,capture_output=True,text=True)
    assert positive.returncode==0,positive.stdout+positive.stderr
    print('positive: 20 tests PASS')
    for name,path,before,after in mutants:
        p=root/path; original=p.read_text(); assert before in original
        p.write_text(original.replace(before,after,1))
        result=subprocess.run(test,capture_output=True,text=True)
        p.write_text(original)
        assert result.returncode != 0 and 'FAIL:' in result.stderr,name+' survived or did not reach assertions: '+result.stderr
        print(name+': KILLED')
