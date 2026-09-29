#!/usr/bin/env python3
"""Read-only legacy-account audit. No new packages, writes or network calls."""
import gzip
import os
from pathlib import Path
import platform
import re
import sqlite3
import stat
import struct
import subprocess
import sys

LOG = Path('/var/log')
WTMPDB = Path('/var/lib/wtmpdb')
LASTLOG2 = Path('/var/lib/lastlog')
PROC = Path('/proc')
LIMIT = 64*1024*1024
HISTORY_ROOTS = (Path('/var/log'), Path('/var/lib'))


def history_files():
    paths = set(LOG.glob('wtmp*')) | set(WTMPDB.glob('wtmp*.db')) | set(LASTLOG2.glob('lastlog2*.db'))
    seen = set()
    result = []
    for path in sorted(paths):
        if path.name.endswith(('-wal','-shm','-journal')): continue
        if path.is_symlink():
            try:
                real = path.resolve(strict=True)
                info = real.stat()
            except (OSError, RuntimeError) as error:
                raise ValueError('login history alias cannot resolve safely: '+str(path)) from error
            if not any(real.is_relative_to(root) for root in HISTORY_ROOTS):
                raise ValueError('login history alias target is outside /var/log and /var/lib: '+str(path))
            if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
                raise ValueError('login history alias target must be regular, root-owned and not group/other-writable: '+str(path))
        else:
            real = path
            info = real.stat()
            if not stat.S_ISREG(info.st_mode):
                raise ValueError('login history has an unexpected file type: '+str(path))
        # Validate every alias before deduplication, even if another path has
        # already selected this inode. A safe Trixie alias is not a second DB.
        key = (info.st_dev, info.st_ino)
        if key not in seen:
            seen.add(key)
            result.append((real, info))
    return result


def unchanged_file(before, after):
    return all(getattr(before, field) == getattr(after, field) for field in
               ('st_dev','st_ino','st_mode','st_uid','st_gid','st_size','st_mtime_ns','st_ctime_ns'))


def history(user, uid):
    # Debian bookworm's binary records and trixie's replacements, without
    # requiring last/lastlog/lastlog2 packages. Read SQL into memory: even a
    # read-only on-disk SQLite connection can create WAL shared-memory files.
    for path, selected in history_files():
        for suffix in ('-wal','-journal'):
            side = Path(str(path)+suffix)
            if side.exists() and side.stat().st_size:
                raise ValueError('login history is being updated; retry after its database transaction completes')
        before = path.stat()
        if path.is_symlink() or not unchanged_file(selected, before):
            raise ValueError('login history changed after path validation')
        opener = gzip.open if path.suffix=='.gz' else open
        with opener(path,'rb') as stream: data=stream.read(LIMIT+1)
        after = path.stat()
        for suffix in ('-wal','-journal'):
            side = Path(str(path)+suffix)
            if side.exists() and side.stat().st_size:
                raise ValueError('login history is being updated; retry after its database transaction completes')
        if len(data)>LIMIT or not unchanged_file(before, after):
            raise ValueError('login history is too large or changed during inspection; cannot safely recognise this account')
        if not data: continue
        if data.startswith(b'SQLite format 3\x00'):
            with sqlite3.connect(':memory:') as db:
                db.deserialize(data)
                db.execute('PRAGMA trusted_schema=OFF')
                if path.name.startswith('lastlog2'):
                    found=db.execute('SELECT 1 FROM Lastlog2 WHERE Name=? AND COALESCE(Time,0) != 0 LIMIT 1',(user,)).fetchone()
                else:
                    found=db.execute('SELECT 1 FROM wtmp WHERE User=? LIMIT 1',(user,)).fetchone()
        else:
            arch=platform.machine()
            # glibc utmp: x86_64's TIME64_COMPAT32 layout is 384 bytes;
            # Linux aarch64 uses 64-bit session/time fields and 400 bytes.
            size={'x86_64':384,'aarch64':400,'arm64':400}.get(arch)
            if not size or len(data)%size: raise ValueError('login history binary format is not recognised on this host')
            found=any(struct.unpack_from('@h',data,n)[0]==7 and data[n+44:n+76].split(b'\0',1)[0]==user.encode()
                      for n in range(0,len(data),size))
        if found: raise ValueError('login history exists for '+user+'; it may be a human account, so it will not be removed')
    path=LOG/'lastlog'
    if path.exists() or path.is_symlink():
        if path.is_symlink() or not path.is_file(): raise ValueError('login history lastlog path is unsafe')
        width={'x86_64':4,'aarch64':8,'arm64':8}.get(platform.machine())
        if not width: raise ValueError('login history lastlog format is unknown on this host')
        size=width+32+256
        if path.stat().st_size%size: raise ValueError('login history lastlog size is invalid')
        with path.open('rb') as stream:
            stream.seek(uid*size); timestamp=stream.read(width)
        if timestamp and int.from_bytes(timestamp,sys.byteorder):
            raise ValueError('login history exists for '+user+'; it may be a human account, so it will not be removed')


def command(args, **identity):
    run=subprocess.run(args,text=True,capture_output=True,timeout=10,**identity)
    if run.returncode: raise ValueError('cannot verify service process ownership; local systemd inspection failed')
    return run.stdout.strip()


def processes(user, uid):
    owned=[]
    for path in PROC.iterdir():
        if not path.name.isdigit(): continue
        try:
            text=(path/'status').read_text()
            ids=next(line.split()[1:] for line in text.splitlines() if line.startswith('Uid:'))
            if str(uid) not in ids: continue
            groups=[line.split(':',2)[2] for line in (path/'cgroup').read_text().splitlines() if line.startswith('0::')]
            if len(groups)!=1: raise ValueError('cannot verify process ownership without its unified systemd cgroup')
            owned.append((path,groups[0]))
        except FileNotFoundError: continue # process exited during observation
    if not owned: return
    if user=='brrdfeeder':
        expected='/system.slice/brrdfeeder-engine.service'
        actual=command(['systemctl','show','brrdfeeder-engine.service','-p','ControlGroup','--value'])
        infrastructure={}
    else:
        base=f'/user.slice/user-{uid}.slice/user@{uid}.service'
        expected=base+'/app.slice/brrdhouse.service'
        # A read-only audit must not open a PAM session through runuser (which
        # can itself write login/audit records during --dry-run).
        actual=command(['systemctl','--user','show','brrdhouse.service','-p','ControlGroup','--value'],
                       user=user,group=user,extra_groups=[],
                       env=dict(os.environ,XDG_RUNTIME_DIR=f'/run/user/{uid}',
                                DBUS_SESSION_BUS_ADDRESS=f'unix:path=/run/user/{uid}/bus'))
        infrastructure={
            base+'/init.scope':{'/usr/lib/systemd/systemd','/lib/systemd/systemd','/usr/lib/systemd/systemd-executor'},
            base+'/app.slice/dbus.service':{'/usr/bin/dbus-daemon','/usr/bin/dbus-broker','/usr/bin/dbus-broker-launch'},
            base+'/session.slice/dbus.service':{'/usr/bin/dbus-daemon','/usr/bin/dbus-broker','/usr/bin/dbus-broker-launch'},
        }
    if actual and actual!=expected: raise ValueError('unexpected package service cgroup; cannot safely recognise its processes')
    for path,group in owned:
        if actual==expected and (group==expected or group.startswith(expected+'/')): continue
        try:
            if group in infrastructure and str((path/'exe').resolve(strict=True)) in infrastructure[group]: continue
            # Exit races do not turn a completed process into a false refusal.
            if not path.exists(): continue
        except FileNotFoundError:
            if not path.exists(): continue
        raise ValueError('unrelated process '+path.name+' runs as '+user+' outside BRRDfeeder services; stop or review it before removal')


def main():
    if len(sys.argv)!=3 or sys.argv[1] not in ('brrdfeeder','brrdhouse') or not re.fullmatch(r'[0-9]+',sys.argv[2]):
        raise ValueError('unexpected account identity')
    user,uid=sys.argv[1],int(sys.argv[2])
    if uid<100: raise ValueError('UID is below the service-account safety floor')
    history(user,uid)
    processes(user,uid)
    print('account audit: no recorded login history; no unrelated process for '+user)


if __name__=='__main__':
    try: main()
    except Exception as error:
        print('REFUSED: '+(str(error) if type(error) is ValueError else 'cannot read/validate login history or process ownership ('+type(error).__name__+')'),file=sys.stderr)
        sys.exit(1)
