# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Live-node read-only predicates for the rehearsed GDB driver.

This adapter NEVER creates identities, credentials, floors or product state.
Only explicit capture-protected writes a NEW evidence file beside this module.
"""
import base64, hashlib, json, os, platform, pwd, stat, sys
from pathlib import Path

LAB = Path(__file__).resolve().parent
B = Path('/usr/local/libexec/brrdfeeder-release')
L = Path(str(B) + '-launch')
C = Path('/etc/brrdfeeder')
U = Path('/var/lib/brrdfeeder-updater')
R = C / '.updater-helper.sha256'
F = U / 'host-state.json'
T = U / 'host-transaction.json'
PATHS = [B, R, F, Path(str(B)+'.previous'), Path(str(B)+'.previous.sha256'), L]
from binding import load_binding, guard as binding_guard
NEW = load_binding()['candidate_sha256']
PHASES = ['prepared', 'switched', 'receipted', 'floored', 'committed']
EXTRA = [Path('/var/lib/brrdfeeder/credentials'),
         Path('/var/lib/brrdfeeder/policy_state.json'),
         Path('/etc/containers/systemd')]

def sha(raw):
    return hashlib.sha256(raw).hexdigest()

def raw(path):
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        return None
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        assert stat.S_ISREG(info.st_mode) and info.st_size <= 32 << 20, str(path)
        result = stream.read((32 << 20)+1)
        assert len(result) == info.st_size, 'changed or oversized file: '+str(path)
        return result

def meta(path):
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    result = dict(mode=stat.S_IMODE(info.st_mode), uid=info.st_uid,
                  gid=info.st_gid, ino=info.st_ino)
    if stat.S_ISLNK(info.st_mode):
        result.update(kind='symlink', target=os.readlink(path))
    elif stat.S_ISDIR(info.st_mode):
        # Receipt/floor renames legitimately change parent-directory mtimes.
        result['kind'] = 'directory'
    else:
        assert stat.S_ISREG(info.st_mode), 'unexpected special file: '+str(path)
        result.update(kind='file', sha256=sha(raw(path)), nlink=info.st_nlink,
                      mtime_ns=info.st_mtime_ns)
    return result

def guard():
    assert os.geteuid() == 0 and platform.machine() in ('aarch64', 'arm64')
    binding_guard()
    for path in (LAB, *LAB.parents):
        info = path.lstat()
        assert stat.S_ISDIR(info.st_mode) and info.st_uid == 0 and not info.st_mode & 0o022

def registered(profile='live'):
    assert profile == 'live', 'never synthesize or switch live host floors'
    data = raw(LAB/'preimages/preimages.json')
    expected = os.environ['HELPER5_PREIMAGES_SHA256']
    assert len(expected) == 64 and sha(data) == expected, 'registered preimages changed'
    manifest = json.loads(data)
    assert set(manifest) == {str(path) for path in PATHS}
    assert manifest[str(B)]['sha256'] == load_binding()['baseline_helper_sha256']
    for item in manifest.values():
        if item is not None:
            assert sha(base64.b64decode(item['data'], validate=True)) == item['sha256']
            assert item['uid'] == 0 and not item['mode'] & 0o022
    return manifest

def prior_floor(manifest):
    item = manifest[str(F)]
    return base64.b64decode(item['data']) if item else b''

def next_floor(old):
    result = json.loads(old) if old else {
        'next_poll':'0001-01-01T00:00:00Z', 'seen':{'sequence':0,'hash':''}, 'attempted':{}}
    result['build_seq'] = 5
    return result

def protection():
    excluded = set(PATHS) | {T}
    result = {}
    def visit(path):
        if path in excluded:
            return
        value = meta(path)
        result[str(path)] = value
        if value and value['kind'] == 'directory':
            for child in sorted(path.iterdir()):
                visit(child)
    for path in [C, U, *EXTRA]:
        visit(path)
    for path in sorted(Path('/etc/systemd/system').glob('brrdfeeder-*')):
        visit(path)
    for path in sorted(Path('/usr/local/bin').glob('brrdfeeder*')):
        visit(path)
    accounts = {}
    for name in ('brrdfeeder','brrdhouse'):
        user = pwd.getpwnam(name)
        accounts[name] = dict(uid=user.pw_uid, gid=user.pw_gid, home=user.pw_dir)
        if name == 'brrdhouse':
            visit(Path(user.pw_dir)/'.config/containers/systemd')
    result['service_accounts'] = accounts
    return result

def check_protected():
    expected = json.loads(raw(LAB/'protected.json'))
    actual = protection()
    changed = [key for key in sorted(set(actual)|set(expected)) if actual.get(key) != expected.get(key)]
    assert not changed, 'protected state changed (do not restore credentials): '+repr(changed)

def check_baseline(profile='live'):
    manifest = registered(profile)
    assert raw(T) is None, 'pending host transaction'
    assert raw(U/'pending_update.json') is None, 'pending workload request'
    workload=raw(U/'state.json')
    assert workload is None or json.loads(workload).get('active') is None, 'pending workload transaction'
    for path in PATHS:
        item = manifest[str(path)]
        if item is None:
            assert meta(path) is None, str(path)
        else:
            assert raw(path) == base64.b64decode(item['data']), str(path)
            info = meta(path)
            assert all(info[key] == item[key] for key in ('mode','uid','gid','sha256')), str(path)
    check_protected()

def state_phase(profile='live'):
    manifest = registered(profile)
    old_hash = manifest[str(B)]['sha256']
    old_floor = prior_floor(manifest)
    binary, receipt, floor, journal = map(raw, (B, R, F, T))
    if raw(L) != raw(LAB/'launcher5.py') or raw(PATHS[3]) != base64.b64decode(manifest[str(B)]['data']):
        return None
    if raw(PATHS[4]) != (old_hash+'\n').encode():
        return None
    for path, mode in ((B,0o755),(R,0o600),(L,0o755),(PATHS[3],0o755),(PATHS[4],0o600)):
        info = meta(path)
        if not info or info['uid'] != 0 or info['mode'] != mode or info.get('nlink') != 1:
            return None
    old_receipt = (old_hash+'  '+str(B)+'\n').encode()
    new_receipt = (NEW+'  '+str(B)+'\n').encode()
    floor_old = (floor or b'') == old_floor
    floor_new = floor is not None and json.loads(floor) == next_floor(old_floor)
    if floor_new:
        info = meta(F)
        floor_new = info['uid'] == 0 and info['mode'] == 0o600 and info['nlink'] == 1
    if journal is None:
        return 'committed' if sha(binary) == NEW and receipt == new_receipt and floor_new else None
    expected = dict(previous_sha256=old_hash, candidate_sha256=NEW, local_upgrade=True,
                    previous_host_state=base64.b64encode(old_floor).decode(), previous_host_state_sha256=sha(old_floor))
    info = meta(T)
    if json.loads(journal) != expected or info['uid'] != 0 or info['mode'] != 0o600 or info['nlink'] != 1:
        return None
    if sha(binary) == old_hash and receipt == old_receipt and floor_old: return 'prepared'
    if sha(binary) == NEW and receipt == old_receipt and floor_old: return 'switched'
    if sha(binary) == NEW and receipt == new_receipt and floor_old: return 'receipted'
    if sha(binary) == NEW and receipt == new_receipt and floor_new: return 'floored'
    return None

def recovered(committed=False):
    manifest = registered()
    old_hash = manifest[str(B)]['sha256']
    expected = NEW if committed else old_hash
    assert raw(T) is None and sha(raw(B)) == expected
    assert raw(R) == (expected+'  '+str(B)+'\n').encode()
    assert raw(L) == raw(LAB/'launcher5.py')
    assert raw(PATHS[3]) == base64.b64decode(manifest[str(B)]['data'])
    assert raw(PATHS[4]) == (old_hash+'\n').encode()
    if committed: assert json.loads(raw(F)) == next_floor(prior_floor(manifest))
    else: assert (raw(F) or b'') == prior_floor(manifest)
    check_protected()

def snapshot():
    return {str(path):meta(path) for path in PATHS+[T]}

if __name__ == '__main__':
    guard()
    command = sys.argv[1]
    if command == 'capture-protected':
        registered()
        with (LAB/'protected.json').open('x') as stream:
            os.fchmod(stream.fileno(),0o600)
            json.dump(protection(),stream,sort_keys=True,indent=2)
            stream.write('\n');stream.flush();os.fsync(stream.fileno())
    elif command == 'protected': check_protected()
    elif command == 'baseline': check_baseline()
    elif command == 'phase': assert state_phase() == sys.argv[2]
    elif command == 'recovered': recovered(len(sys.argv)>2 and sys.argv[2]=='committed')
    elif command == 'snapshot': print(json.dumps(snapshot(),sort_keys=True,indent=2))
    else: raise SystemExit('unknown read-only inspection command')
    if command != 'snapshot': print('PASS '+command)
