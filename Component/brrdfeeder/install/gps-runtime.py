#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""BRRDfeeder GPS runtime transport: exact device grant, bounded hotplug recovery.

Never opens a serial port or discovers/claims adapters. A stable character inode
survives USB removal between ExecStartPre and Podman's device stat. /dev/null is
the absent transport: termios rejects it, so it cannot masquerade as live GPS.
"""
import fcntl
import datetime
import grp
import json
import os
import re
from pathlib import Path
import stat
import subprocess
import sys
import time
import yaml

ROOT = Path('/run/brrdfeeder-gps')
DEVICE = Path('/dev/cybrrd_gps')
CONFIG = Path('/etc/brrdfeeder/config.yaml')
UNIT = 'brrdfeeder-engine.service'
STATUS = Path('/var/lib/brrdfeeder-status')


def status():
    try:
        record = json.loads((STATUS/'startup.json').read_text())
        stamp = datetime.datetime.fromisoformat(record['written_at'])
        age = (datetime.datetime.now(datetime.timezone.utc)-stamp).total_seconds()
        if not 0 <= age <= 15:
            print('GPS startup status is stale; current GPS state unknown.')
            return
        state = record.get('state')
        if state == 'gps-missing':
            ids = record.get('usb_adapter_ids', [])
            ids = [s for s in ids[:8] if isinstance(s, str) and re.fullmatch(r'[0-9a-f]{4}:[0-9a-f]{4}', s)]
            print('No GPS device found; serial adapter IDs seen: '+(', '.join(ids) or 'none')+'.')
        elif state == 'gps-waiting':
            gps = record.get('gps', {})
            def number(key):
                value = gps.get(key)
                return str(value) if type(value) in (int, float) and 0 <= value <= 999 else 'unknown'
            print('GPS device present, no fix yet; satellites='+number('satellites_used')+' HDOP='+number('hdop')+'.')
        else:
            print('GPS first-fix startup is waiting; inspect the local console for details.')
    except FileNotFoundError:
        try:
            record = json.loads((STATUS/'status.json').read_text())
            stamp = datetime.datetime.fromisoformat(record['written_at'].replace('Z', '+00:00'))
            age = (datetime.datetime.now(datetime.timezone.utc)-stamp).total_seconds()
            if record.get('heartbeat', {}).get('os_clock_trusted') is False:
                print('Last engine report: saved position preserved; clock untrusted, report freshness unverified.'+
                      (' GPS not live.' if record.get('heartbeat', {}).get('gps', {}).get('state') != 'healthy' else ''))
            elif not 0 <= age <= 3*record['status_interval_secs']:
                print('Engine status is stale; current GPS state unknown.')
            elif record.get('heartbeat', {}).get('gps', {}).get('state') != 'healthy':
                print('Engine running on saved position: position preserved, GPS not live.')
            else:
                print('GPS reports a live fix; see the console for position and time trust.')
        except (OSError, ValueError, KeyError, TypeError):
            print('GPS status not available yet.')
    except (OSError, ValueError, KeyError, TypeError):
        print('GPS status is invalid; current GPS state unknown.')


def identity():
    try:
        value = DEVICE.stat()
        if stat.S_ISCHR(value.st_mode):
            return [value.st_dev, value.st_ino, value.st_rdev, value.st_ctime_ns]
    except OSError:
        pass
    return None


def prepare(state, current):
    temporary = ROOT/'device.new'
    temporary.unlink(missing_ok=True)
    if current is None:
        # Unlike a USB symlink, /dev/null cannot disappear on receiver removal.
        os.symlink('/dev/null', temporary)
    else:
        os.mknod(temporary, stat.S_IFCHR | 0o660, current[2])
        os.chown(temporary, 0, grp.getgrnam('dialout').gr_gid)
        os.chmod(temporary, 0o660)
    os.replace(temporary, ROOT/'device')
    state['mapped'] = current
    state['attempted'] = current


def should_restart(state, current, now):
    # Removal never restarts. Each new present identity gets at most one attempt;
    # flapping is bounded to three attempts per ten minutes and one per minute.
    attempts = [t for t in state.get('restarts', []) if 0 <= now-t < 600]
    state['restarts'] = attempts
    if current is None or current == state.get('mapped') or current == state.get('attempted'):
        return False
    if len(attempts) >= 3 or (attempts and now-attempts[-1] < 60):
        return False
    state['attempted'] = current
    attempts.append(now)
    return True


def save(state):
    temporary = ROOT/'state.new'
    with open(temporary, 'w', encoding='utf-8') as output:
        json.dump(state, output)
    os.replace(temporary, ROOT/'state.json')


def main():
    global DEVICE
    if sys.argv[1:] == ['status']:
        status()
        return
    os.umask(0o077)
    config = yaml.safe_load(CONFIG.read_text())
    device = config.get('sensors', {}).get('gps', {}).get('device', '/dev/cybrrd_gps')
    if not isinstance(device, str) or not device.startswith('/dev/'):
        raise ValueError('GPS device must be under /dev')
    DEVICE = Path(device)
    ROOT.mkdir(mode=0o700, exist_ok=True)
    info = ROOT.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o077:
        raise ValueError('unsafe GPS runtime directory')
    with open(ROOT/'lock', 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        path = ROOT/'state.json'
        state = json.loads(path.read_text()) if path.exists() else {}
        current = identity()
        if sys.argv[1:] == ['prepare']:
            prepare(state, current)
            save(state)
            print('GPS transport: '+('device present' if current else 'absent; position preserved, GPS not live'), flush=True)
        elif sys.argv[1:] == ['check']:
            # Never interrupt a first-fix ExecStartPre, intentional stop, update,
            # or failed service. --no-block avoids waiting on prepare's lock.
            active = subprocess.run(['systemctl', 'is-active', '--quiet', UNIT], timeout=5)
            if active.returncode == 0 and should_restart(state, current, time.monotonic()):
                save(state)  # Persist attempt before calling the restart effector.
                print('GPS appeared/replaced: requesting one bounded engine restart to attach the exact device', flush=True)
                subprocess.run(['systemctl', 'try-restart', '--no-block', UNIT], check=True, timeout=10)
        else:
            raise ValueError('expected prepare or check')


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print('GPS runtime refused ('+type(error).__name__+'); inspect configuration/runtime ownership.', flush=True)
        raise SystemExit(1)
