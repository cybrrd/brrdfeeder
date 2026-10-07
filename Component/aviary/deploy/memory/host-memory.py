#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Read-only host observations; only stop mode writes a durable OOM receipt.

No network, Podman socket, subprocesses, journal content, or engine-writable
input. Metrics are observations, not health or release authority.
"""
import fcntl
import json
import os
from pathlib import Path
import pwd
import re
import stat
import sys
import tempfile
import time

STATE = Path('/var/lib/brrdfeeder-memory')
RUNTIME = Path('/run/brrdfeeder-memory')
PROC = Path('/proc')
JOURNAL = Path('/run/log/journal')
MAX_BYTES = 16384


def read(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError('not regular')
        raw = stream.read(MAX_BYTES + 1)
        if len(raw) > MAX_BYTES:
            raise ValueError('oversize')
        return raw.decode('ascii')


def protected_dir(path):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o022:
        raise ValueError('unprotected directory')


def atomic(path, doc, durable=False):
    protected_dir(path.parent)
    fd, name = tempfile.mkstemp(prefix='.memory-', dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as stream:
            os.fchmod(stream.fileno(), 0o644)
            json.dump(doc, stream, sort_keys=True)
            stream.write('\n')
            stream.flush()
            if durable:
                os.fsync(stream.fileno())
        os.replace(name, path)
        if durable:
            directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def counter(state):
    try:
        doc = json.loads(read(state / 'events.json'))
    except FileNotFoundError:
        return {'schema_version': 1, 'memory_cap_events': 0, 'last_invocation': None}
    if (not isinstance(doc, dict) or type(doc.get('schema_version')) is not int
            or doc.get('schema_version') != 1 or type(doc.get('memory_cap_events')) is not int
            or not 0 <= doc['memory_cap_events'] < 2**64
            or not isinstance(doc.get('last_invocation'), str)
            or not re.fullmatch('[a-f0-9]{32}', doc['last_invocation'])):
        raise ValueError('invalid counter; refusing reset')
    return doc


def record_stop(state, result, invocation):
    protected_dir(state)
    if result != 'oom-kill':
        return False
    if not re.fullmatch('[a-f0-9]{32}', invocation):
        raise ValueError('invalid invocation')
    fd = os.open(state / '.lock', os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        doc = counter(state)
        if doc['last_invocation'] == invocation:
            return False
        if doc['memory_cap_events'] == 2**64 - 1:
            raise ValueError('counter exhausted')
        doc.update(memory_cap_events=doc['memory_cap_events'] + 1, last_invocation=invocation)
        atomic(state / 'events.json', doc, durable=True)
        print('BRRDfeeder memory event: service_result=oom-kill memory_cap_events=' +
              str(doc['memory_cap_events']) + ' invocation=' + invocation, flush=True)
        return True


def kib(text, key):
    matches = [line.split() for line in text.splitlines() if line.startswith(key + ':')]
    if len(matches) != 1 or len(matches[0]) != 3 or matches[0][2] != 'kB':
        return None
    value = matches[0][1]
    return int(value) * 1024 if value.isascii() and value.isdecimal() and int(value) < 2**54 else None


def console_rss(proc, uid):
    # Exact dedicated UID + systemd unit membership; never attribute another
    # user's similarly named process. Sum only the console executable, not conmon.
    total, found = 0, False
    for index, process in enumerate(proc.iterdir()):
        if index > 65536:
            return None
        if not process.name.isdecimal():
            continue
        try:
            if process.stat().st_uid != uid:
                continue
            group = read(process / 'cgroup')
            if not any(line.startswith('0::') and 'brrdhouse.service' in line[3:].split('/') for line in group.splitlines()):
                continue
            if read(process / 'comm').strip() != 'brrdhouse':
                continue
            value = kib(read(process / 'status'), 'VmRSS')
            if value is None:
                return None
            total += value
            found = True
        except (OSError, ValueError):
            continue
    return total if found else None


def journal_bytes(root):
    # Allocated blocks, not file contents or persistent /var/log/journal usage.
    if not root.is_dir() or root.is_symlink():
        return None
    total, entries = 0, 0
    def fail(error):
        raise error
    for directory, dirs, files in os.walk(root, followlinks=False, onerror=fail):
        dirs[:] = [name for name in dirs if not (Path(directory) / name).is_symlink()]
        for name in files:
            entries += 1
            if entries > 8192:
                return None
            info = (Path(directory) / name).lstat()
            if stat.S_ISREG(info.st_mode) and (name.endswith('.journal') or name.endswith('.journal~')):
                total += info.st_blocks * 512
    return total


def sample(proc, journal, state, console_uid):
    doc = {'schema_version': 1, 'boot_id': read(proc / 'sys/kernel/random/boot_id').strip(),
           'sampled_boottime_secs': int(time.clock_gettime(time.CLOCK_BOOTTIME))}
    try:
        meminfo = read(proc / 'meminfo')
        doc['host_mem_total_bytes'] = kib(meminfo, 'MemTotal')
        doc['host_mem_available_bytes'] = kib(meminfo, 'MemAvailable')
    except (OSError, ValueError):
        pass
    for key, observe in (
        ('console_rss_bytes', lambda: console_rss(proc, console_uid) if console_uid is not None else None),
        ('volatile_journal_bytes', lambda: journal_bytes(journal)),
        ('memory_cap_events', lambda: counter(state)['memory_cap_events']),
    ):
        try:
            doc[key] = observe()
        except (OSError, ValueError):
            print('BRRDfeeder memory observation unavailable: ' + key, file=sys.stderr)
    return {key: value for key, value in doc.items() if value is not None}


def main():
    if os.geteuid() != 0 or sys.argv[1:] not in (['sample'], ['stop']):
        raise ValueError('root-only sample|stop')
    protected_dir(STATE)
    protected_dir(RUNTIME)
    if sys.argv[1] == 'stop':
        record_stop(STATE, os.environ.get('SERVICE_RESULT', ''), os.environ.get('INVOCATION_ID', ''))
    try:
        uid = pwd.getpwnam('brrdhouse').pw_uid
    except KeyError:
        uid = None
    atomic(RUNTIME / 'host.json', sample(PROC, JOURNAL, STATE, uid))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        print('BRRDfeeder memory helper failed: ' + type(error).__name__, file=sys.stderr)
        raise SystemExit(1)
