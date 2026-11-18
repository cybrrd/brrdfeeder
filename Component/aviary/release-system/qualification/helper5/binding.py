#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Require off-box registration before any helper-5 native kit operation."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import stat
import sys

KIT = Path(__file__).resolve().parent
HASH_FIELDS = ('candidate_sha256', 'baseline_helper_sha256', 'installer_sha256',
               'baseline_installer_sha256', 'launcher_sha256', 'preimages_tool_sha256',
               'adapter_sha256', 'gdb_driver_sha256')
TEXT_FIELDS = ('hostname', 'node_id', 'operator', 'window', 'oob', 'physical_asset')

def digest(data):
    return hashlib.sha256(data).hexdigest()

def validate(b):
    if b.get('from_build') != 4 or b.get('to_build') != 5 or b.get('baseline_version') != '0.9.0':
        raise ValueError('requires released fresh 0.9.0 helper 4 to helper 5')
    for key in HASH_FIELDS:
        if not re.fullmatch('[0-9a-f]{64}', b.get(key) or ''):
            raise ValueError('unbound '+key)
    for key in ('source_commit', 'baseline_source_commit'):
        if not re.fullmatch('[0-9a-f]{40}', b.get(key) or ''):
            raise ValueError('unbound '+key)
    for key in TEXT_FIELDS:
        if not isinstance(b.get(key),str) or not b[key].strip() or b[key].upper() in ('PENDING','UNKNOWN','TBD'):
            raise ValueError('unbound '+key)
    return b

def load_binding():
    raw = (KIT/'binding.json').read_bytes()
    if digest(raw) != os.environ.get('HELPER5_BINDING_SHA256'):
        raise ValueError('independently registered binding SHA256 required')
    b = validate(json.loads(raw))
    for name, key in [('helper5','candidate_sha256'),('install.sh','installer_sha256'),
                      ('0.9.0-install.sh','baseline_installer_sha256'),('launcher5.py','launcher_sha256'),
                      ('helper-preimages.py','preimages_tool_sha256'),('fixture.py','adapter_sha256'),
                      ('gdb-fsync.py','gdb_driver_sha256')]:
        if digest((KIT/name).read_bytes()) != b[key]:
            raise ValueError('kit artifact changed: '+name)
    return b

def guard():
    b = load_binding()
    if os.geteuid()!=0 or platform.machine() not in ('aarch64','arm64') or platform.node()!=b['hostname']:
        raise ValueError('wrong native host/root/architecture')
    for path in (KIT,*KIT.parents):
        st=path.lstat()
        if not stat.S_ISDIR(st.st_mode) or st.st_uid!=0 or st.st_mode & 0o022:
            raise ValueError('unsafe kit directory')
    # Observe only; never seed a node ID or either credential.
    config=Path('/etc/brrdfeeder/config.yaml').read_text()
    ids=re.findall(r'^  id:\s*["\']?([^"\'\s]+)["\']?\s*$',config,re.M)
    if ids != [b['node_id']]:
        raise ValueError('node identity mismatch')
    for name in ('/etc/brrdfeeder/secrets/brrdfeeder.creds',
                 '/var/lib/brrdfeeder/credentials/brrdfeeder.creds'):
        st=Path(name).lstat()
        if not stat.S_ISREG(st.st_mode) or st.st_nlink!=1 or st.st_size==0:
            raise ValueError('fresh baseline requires both naturally created credentials')
    return b

if __name__ == '__main__':
    if sys.argv[1:] == ['guard']: guard()
    else: print(load_binding()[sys.argv[1]])
