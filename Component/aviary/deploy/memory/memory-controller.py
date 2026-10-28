#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Pi memory-controller repair and read-only enforcement check. Never reboots."""
import fcntl
import os
from pathlib import Path
import stat
import sys
import tempfile

CAP = 256 * 1024 * 1024


def read(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > 16384:
            raise ValueError('expected a bounded regular file: ' + str(path))
        value = stream.read(16385)
        if len(value) > 16384:
            raise ValueError('oversized file: ' + str(path))
        return value


def enabled_cmdline(raw):
    # Preserve all unrelated tokens. The boot command line must remain one line.
    text = raw.decode('ascii')
    if '\n' in text.rstrip('\n') or '\r' in text:
        raise ValueError('cmdline must contain one line')
    tokens = text.split()
    updated = []
    for token in tokens:
        if token.startswith('cgroup_disable='):
            controllers = token.split('=', 1)[1].split(',')
            if 'memory' in controllers:
                controllers = [item for item in controllers if item != 'memory']
                if controllers:
                    updated.append('cgroup_disable=' + ','.join(controllers))
                continue
        updated.append(token)
    return (' '.join(updated) + '\n').encode('ascii') if updated != tokens else raw


def replace(path, raw):
    # Caller holds the boot-directory lock; atomic replace + directory fsync.
    fd, name = tempfile.mkstemp(prefix='.brrdfeeder-memory-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            os.fchmod(stream.fileno(), stat.S_IMODE(path.stat(follow_symlinks=False).st_mode))
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def edit(root, restore=False):
    cmdline = root / 'boot/firmware/cmdline.txt'
    backup = cmdline.with_name('cmdline.txt.brrdfeeder-memory.bak')
    if not restore:
        try:
            model = read(root / 'sys/firmware/devicetree/base/model')
        except FileNotFoundError:
            return 'not a Raspberry Pi; boot command line unchanged'
        if not model.startswith(b'Raspberry Pi'):
            return 'not a Raspberry Pi; boot command line unchanged'
    lock_fd = os.open(cmdline.parent / '.brrdfeeder-memory.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(lock_fd, 'rb') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        original = read(cmdline)
        if restore:
            saved = read(backup)
            if original == saved:
                return 'original cmdline already restored; reboot required if not yet rebooted'
            if original != enabled_cmdline(saved):
                raise ValueError('cmdline changed since repair; backup retained, manual reconciliation required')
            replace(cmdline, saved)
            return 'original cmdline restored byte-for-byte; REBOOT REQUIRED'
        updated = enabled_cmdline(original)
        if updated == original:
            return 'memory is not disabled in boot cmdline; running-controller check still required'
        try:
            saved = read(backup)
        except FileNotFoundError:
            fd = os.open(backup, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            with os.fdopen(fd, 'wb') as stream:
                stream.write(original)
                stream.flush()
                os.fsync(stream.fileno())
            directory = os.open(cmdline.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        else:
            if saved != original:
                raise ValueError('prior backup differs; refusing to overwrite it')
        replace(cmdline, updated)
        return 'removed cgroup_disable=memory; backup=' + str(backup) + '; REBOOT REQUIRED (operator action)'


def check(root):
    base = root / 'sys/fs/cgroup'
    controllers = read(base / 'cgroup.controllers').decode('ascii').split()
    if 'memory' not in controllers:
        raise ValueError('memory controller absent; MemoryMax is NOT enforced; reboot after boot-cmdline repair')
    actual = read(base / 'system.slice/brrdfeeder-engine.service/memory.max').strip()
    if actual != str(CAP).encode('ascii'):
        raise ValueError('engine memory.max=' + actual.decode('ascii') + '; expected 268435456; cap NOT verified')
    return 'memory controller available; engine memory.max=268435456; 256 MiB cap enforced'


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in ('enable', 'restore', 'check'):
        raise ValueError('usage: brrdfeeder-memory-controller enable|restore|check')
    command = sys.argv[1]
    if command != 'check' and os.geteuid() != 0:
        raise ValueError('boot-cmdline edits require root')
    print(check(Path('/')) if command == 'check' else edit(Path('/'), command == 'restore'))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError) as error:
        print('BRRDfeeder memory controller: ' + str(error), file=sys.stderr)
        raise SystemExit(1)
