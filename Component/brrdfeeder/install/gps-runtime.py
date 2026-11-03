#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""BRRDfeeder GPS runtime: safe discovery, exact grant, bounded recovery.

Native/previously confirmed mappings win. When they are absent, one CP2102N tty
may be receive-only NMEA-qualified with the install-time ownership and restore
contract. The exact character device is then passed through the existing stable
transport; no broad CP210x udev mapping or configuration rewrite is needed.
"""
import fcntl
import datetime
import grp
import json
import os
import re
import select
from pathlib import Path
import stat
import subprocess
import sys
import termios
import time
import yaml

ROOT = Path('/run/brrdfeeder-gps')
DEVICE = Path('/dev/cybrrd_gps')
CONFIG = Path('/etc/brrdfeeder/config.yaml')
UNIT = 'brrdfeeder-engine.service'
STATUS = Path('/var/lib/brrdfeeder-status')
SYS_TTY = Path('/sys/class/tty')
DEV_ROOT = Path('/dev')
PROC = Path('/proc')
HOTPLUG_ID = '10c4:ea60'
UBLOX_IDS = {'1546:'+product for product in ('01a5', '01a6', '01a7', '01a8', '01a9')}
PROBE_SECONDS = 5.0
PROBE_RETRY_SECONDS = 60.0


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


def device_identity(path):
    try:
        value = path.stat()
        if stat.S_ISCHR(value.st_mode):
            return [value.st_dev, value.st_ino, value.st_rdev, value.st_ctime_ns]
    except OSError:
        pass
    return None


def identity():
    return device_identity(DEVICE)


def tty_inventory():
    """Return [(usb_id, tty_path)] from tty sysfs metadata without opening ttys."""
    found = []
    try:
        entries = sorted(SYS_TTY.iterdir())
    except OSError:
        return found
    for entry in entries:
        try:
            parent = (entry/'device').resolve(strict=True)
        except OSError:
            continue
        while parent != parent.parent:
            try:
                vendor = (parent/'idVendor').read_text().strip().lower()
                product = (parent/'idProduct').read_text().strip().lower()
            except (FileNotFoundError, NotADirectoryError):
                parent = parent.parent
                continue
            except OSError:
                break
            usb_id = vendor+':'+product
            if re.fullmatch(r'[0-9a-f]{4}:[0-9a-f]{4}', usb_id):
                found.append((usb_id, DEV_ROOT/entry.name))
            break
    return found


def held_elsewhere(rdev):
    """Fail closed if another visible process holds any alias of this tty."""
    try:
        for process in PROC.iterdir():
            if not process.name.isdigit() or int(process.name) == os.getpid():
                continue
            try:
                for entry in (process/'fd').iterdir():
                    try:
                        info = entry.stat()
                    except FileNotFoundError:
                        continue
                    if stat.S_ISCHR(info.st_mode) and info.st_rdev == rdev:
                        return 'busy'
            except FileNotFoundError:
                continue
    except OSError:
        return 'ownership-unknown'
    return ''


def valid_nmea(sentence):
    if not sentence.startswith(('$GP', '$GN')):
        return False
    body, separator, checksum = sentence.strip().rpartition('*')
    if not separator or len(checksum) != 2:
        return False
    value = 0
    for char in body[1:]:
        value ^= ord(char)
    return ('%02X' % value) == checksum.upper()


def confirm(device):
    """Receive-only NMEA confirmation; restore failure overrides all outcomes."""
    fd, original = None, None
    try:
        info = os.stat(device)
        if not stat.S_ISCHR(info.st_mode):
            return 'no-nmea'
        ownership = held_elsewhere(info.st_rdev)
        if ownership:
            return ownership
        fd = os.open(device, os.O_RDONLY | os.O_NOCTTY | os.O_NONBLOCK)
        if os.fstat(fd).st_rdev != info.st_rdev:
            return 'no-nmea'
        ownership = held_elsewhere(info.st_rdev)
        if ownership:
            return ownership
        original = termios.tcgetattr(fd)
        attrs = termios.tcgetattr(fd)
        attrs[0] = attrs[1] = attrs[3] = 0
        attrs[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        attrs[4] = attrs[5] = termios.B9600
        attrs[6][termios.VMIN], attrs[6][termios.VTIME] = 0, 0
        termios.tcsetattr(fd, termios.TCSANOW, attrs)
        buffer, seen, deadline = '', 0, time.monotonic()+PROBE_SECONDS
        while time.monotonic() < deadline and seen < 2:
            ready, _, _ = select.select([fd], [], [], max(0.0, deadline-time.monotonic()))
            if not ready:
                continue
            try:
                chunk = os.read(fd, 256)
            except BlockingIOError:
                continue
            if not chunk:
                break
            for char in chunk.decode('ascii', 'replace'):
                if char == '$':
                    if valid_nmea(buffer):
                        seen += 1
                    buffer = '$'
                elif char in '\r\n':
                    if valid_nmea(buffer):
                        seen += 1
                    buffer = ''
                elif buffer:
                    buffer += char
                    if len(buffer) > 128:
                        buffer = ''
        return 'ok' if seen >= 2 else 'no-nmea'
    except (OSError, termios.error, KeyboardInterrupt):
        return 'no-nmea'
    finally:
        if fd is not None:
            restore_failed = False
            try:
                if original is not None:
                    termios.tcsetattr(fd, termios.TCSANOW, original)
                    restore_failed = termios.tcgetattr(fd) != original
            except (OSError, termios.error):
                restore_failed = True
            finally:
                os.close(fd)
            if restore_failed:
                return 'restore-failed'


def record_probe(state, identity_value, outcome, now):
    state['probe'] = {'identity': identity_value, 'outcome': outcome, 'at': now}


def discover(state, now=None):
    """Resolve native GPS first, otherwise safely qualify one late CP2102N."""
    now = time.monotonic() if now is None else now
    native = identity()
    if native is not None:
        return native
    if DEVICE != DEV_ROOT/'cybrrd_gps':
        # A custom configured path is an explicit operator binding. Never
        # substitute an automatically discovered bridge for it.
        return None
    inventory = tty_inventory()
    if any(usb_id in UBLOX_IDS for usb_id, _ in inventory):
        # udev owns native u-blox mapping; a following timer tick sees its link.
        return None
    candidates = [path for usb_id, path in inventory if usb_id == HOTPLUG_ID]
    if not candidates:
        return None
    if len(candidates) != 1:
        record_probe(state, None, 'ambiguous', now)
        print('GPS hotplug refused: multiple CP2102N candidates; no port opened', flush=True)
        return None
    candidate = candidates[0]
    candidate_identity = device_identity(candidate)
    if candidate_identity is None:
        return None
    qualified = state.get('qualified', {})
    if (qualified.get('usb_id') == HOTPLUG_ID and
            qualified.get('path') == str(candidate) and
            qualified.get('identity') == candidate_identity):
        ownership = held_elsewhere(candidate_identity[2])
        if ownership:
            record_probe(state, candidate_identity, ownership, now)
            return None
        return candidate_identity
    previous = state.get('probe', {})
    if previous.get('identity') == candidate_identity:
        outcome = previous.get('outcome')
        elapsed = now-previous.get('at', now)
        if outcome == 'restore-failed' or (outcome == 'no-nmea' and 0 <= elapsed < PROBE_RETRY_SECONDS):
            return None
    outcome = confirm(candidate)
    # Do not authorize an identity that changed during the bounded read.
    if outcome == 'ok' and device_identity(candidate) != candidate_identity:
        outcome = 'changed'
    record_probe(state, candidate_identity, outcome, now)
    if outcome != 'ok':
        print('GPS hotplug candidate '+HOTPLUG_ID+' '+outcome+'; no mapping or restart', flush=True)
        return None
    state['qualified'] = {'usb_id': HOTPLUG_ID, 'path': str(candidate),
                          'identity': candidate_identity}
    print('GPS hotplug candidate '+HOTPLUG_ID+' NMEA-confirmed; exact device qualified', flush=True)
    return candidate_identity


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
        before = json.dumps(state, sort_keys=True)
        current = discover(state)
        if sys.argv[1:] == ['prepare']:
            prepare(state, current)
            save(state)
            print('GPS transport: '+('device present' if current else 'absent; position preserved, GPS not live'), flush=True)
        elif sys.argv[1:] == ['check']:
            # Never interrupt a first-fix ExecStartPre, intentional stop, update,
            # or failed service. --no-block avoids waiting on prepare's lock.
            active = subprocess.run(['systemctl', 'is-active', '--quiet', UNIT], timeout=5)
            restart = active.returncode == 0 and should_restart(state, current, time.monotonic())
            if restart or json.dumps(state, sort_keys=True) != before:
                save(state)  # Persist qualification/attempt before the effector.
            if restart:
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
