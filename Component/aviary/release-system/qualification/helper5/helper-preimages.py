#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""ARM64 operator-plan companion for helper 5; native qualification NOT_RUN.

Save only helper transaction preimages, never credentials or workload state.
Call restore under launch.lock then lock, after approved timer quiescence and
protected-state comparison. A committed rollback needs explicit operator go.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import platform
import stat
import tempfile

from binding import load_binding, guard
PIN = load_binding()['candidate_sha256']
BINARY = Path('/usr/local/libexec/brrdfeeder-release')
LAUNCH = Path(str(BINARY)+'-launch')
RECEIPT = Path('/etc/brrdfeeder/.updater-helper.sha256')
FLOOR = Path('/var/lib/brrdfeeder-updater/host-state.json')
TX = FLOOR.with_name('host-transaction.json')
PATHS = [BINARY, RECEIPT, FLOOR, Path(str(BINARY)+'.previous'),
         Path(str(BINARY)+'.previous.sha256'), LAUNCH]


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def secure_parents(path):
    for parent in path.parents:
        st = parent.lstat()
        if not stat.S_ISDIR(st.st_mode) or st.st_uid != 0 or st.st_mode & 0o022:
            raise RuntimeError('unsafe parent: '+str(parent))


def read(path):
    secure_parents(path)
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        return None
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode) or st.st_uid != 0 or st.st_nlink != 1 or st.st_mode & 0o022:
            raise RuntimeError('unsafe file: '+str(path))
        if st.st_size > 32 << 20:
            raise RuntimeError('oversize file')
        with os.fdopen(fd, 'rb', closefd=False) as stream:
            raw = stream.read((32 << 20)+1)
        if len(raw) != st.st_size:
            raise RuntimeError('file changed during read')
        return dict(data=base64.b64encode(raw).decode(), sha256=digest(raw),
                    mode=stat.S_IMODE(st.st_mode), uid=st.st_uid, gid=st.st_gid)
    finally:
        os.close(fd)


def sync_dir(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)


def replace(path, item):
    secure_parents(path)
    read(path)  # refuse a symlink/nonregular/unowned destination
    if item is None:
        if path.exists():
            path.unlink()
            sync_dir(path.parent)
        return
    raw = base64.b64decode(item['data'], validate=True)
    if digest(raw) != item['sha256'] or item['uid'] != 0:
        raise RuntimeError('invalid preimage')
    fd, name = tempfile.mkstemp(prefix='.host-recover-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            os.fchown(stream.fileno(), item['uid'], item['gid'])
            os.fchmod(stream.fileno(), item['mode'])
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
        sync_dir(path.parent)
    finally:
        if os.path.exists(name): os.unlink(name)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('operation', choices=['save', 'restore'])
    p.add_argument('directory', type=Path)
    p.add_argument('--manifest-sha256')
    args = p.parse_args()
    guard()
    if os.geteuid() != 0 or platform.machine() not in ('aarch64', 'arm64'):
        raise RuntimeError('approved native ARM64 root shell required')
    folder = args.directory.absolute()
    secure_parents(folder)
    manifest = folder/'preimages.json'
    if args.operation == 'save':
        if read(TX) is not None or read(FLOOR.with_name('pending_update.json')) is not None:
            raise RuntimeError('transaction/request already present')
        workload=read(FLOOR.with_name('state.json'))
        if workload and json.loads(base64.b64decode(workload['data'])).get('active') is not None:
            raise RuntimeError('workload transaction already present')
        data = {str(path): read(path) for path in PATHS}
        if any(data[str(path)] is None for path in (BINARY, RECEIPT, LAUNCH)):
            raise RuntimeError('required installed helper files absent')
        if data[str(BINARY)]['sha256'] != load_binding()['baseline_helper_sha256']:
            raise RuntimeError('not the registered released helper 4')
        import subprocess
        if subprocess.check_output([str(BINARY), 'build-seq']) != b'4\n':
            raise RuntimeError('baseline must report exactly build 4')
        prior = data[str(FLOOR)]
        if prior and json.loads(base64.b64decode(prior['data']))['build_seq'] > 4:
            raise RuntimeError('baseline effective floor exceeds 4')
        receipt = base64.b64decode(data[str(RECEIPT)]['data']).decode()
        expected = data[str(BINARY)]['sha256']+'  '+str(BINARY)+'\n'
        if receipt != expected: raise RuntimeError('receipt mismatch')
        folder.mkdir(mode=0o700)  # never overwrite a previous registration
        raw = (json.dumps(data, sort_keys=True, indent=2)+'\n').encode()
        replace(manifest, dict(data=base64.b64encode(raw).decode(),
                              sha256=digest(raw), mode=0o600, uid=0, gid=0))
        print(digest(raw))  # independently register before any upgrade
        return
    saved = read(manifest)
    if saved is None or not args.manifest_sha256 or saved['sha256'] != args.manifest_sha256:
        raise RuntimeError('independently registered manifest hash required')
    data = json.loads(base64.b64decode(saved['data']))
    if set(data) != {str(path) for path in PATHS}:
        raise RuntimeError('unexpected restoration targets')
    # Validate every preimage and destination before the first mutation.
    for path in PATHS:
        item = data[str(path)]
        if item is not None:
            raw = base64.b64decode(item['data'], validate=True)
            if digest(raw) != item['sha256'] or item['uid'] != 0 or item['mode'] & 0o022:
                raise RuntimeError('invalid preimage metadata/checksum')
        read(path)
    old = data[str(BINARY)]['sha256']
    if old != load_binding()['baseline_helper_sha256']:
        raise RuntimeError('registered baseline identity mismatch')
    if base64.b64decode(data[str(RECEIPT)]['data']) != (old+'  '+str(BINARY)+'\n').encode():
        raise RuntimeError('registered receipt mismatch')
    current = read(BINARY)
    if current is None or current['sha256'] not in (old, PIN):
        raise RuntimeError('unexpected installed binary; console recovery required')
    journal = read(TX)
    if journal:
        j = json.loads(base64.b64decode(journal['data']))
        if j.get('local_upgrade') is not True or j.get('previous_sha256') != old or j.get('candidate_sha256') != PIN:
            raise RuntimeError('not this qualification transaction')
    # A later signed host update must not be overwritten by an old snapshot.
    prior = data[str(FLOOR)]
    prior_raw = base64.b64decode(prior['data']) if prior else b''
    expected = json.loads(prior_raw) if prior_raw else {
        'next_poll':'0001-01-01T00:00:00Z', 'seen':{'sequence':0,'hash':''}, 'attempted':{}}
    expected['build_seq'] = 5
    if journal:
        expected_journal = dict(previous_sha256=old, candidate_sha256=PIN, local_upgrade=True,
                                previous_host_state=base64.b64encode(prior_raw).decode(),
                                previous_host_state_sha256=digest(prior_raw))
        if j != expected_journal:
            raise RuntimeError('journal floor is not the registered preimage')
    current_floor = read(FLOOR)
    raw_floor = base64.b64decode(current_floor['data']) if current_floor else b''
    if raw_floor != prior_raw and (not raw_floor or json.loads(raw_floor) != expected):
        raise RuntimeError('host floor changed beyond this qualification')
    # Refuse unrelated changes to helper companions, not just the main binary.
    allowed = {
        RECEIPT: [(old+'  '+str(BINARY)+'\n').encode(), (PIN+'  '+str(BINARY)+'\n').encode()],
        Path(str(BINARY)+'.previous'): [base64.b64decode(data[str(BINARY)]['data'])],
        Path(str(BINARY)+'.previous.sha256'): [(old+'\n').encode()],
        LAUNCH: [Path(__file__).with_name('launcher5.py').read_bytes()],
    }
    for path, values in allowed.items():
        item=read(path); saved=data[str(path)]
        if item == saved: continue
        if item is None or base64.b64decode(item['data']) not in values:
            raise RuntimeError('helper companion changed beyond this qualification')
    for path in PATHS[:-1]: replace(path, data[str(path)])
    replace(TX, None)  # coherent binary/receipt/floor before journal removal
    replace(LAUNCH, data[str(LAUNCH)])  # old launcher last
    for path in PATHS:
        if read(path) != data[str(path)]: raise RuntimeError('restore verification failed')
    print('Helper preimages restored; run native self-test and protected-state checks.')


if __name__ == '__main__':
    main()
