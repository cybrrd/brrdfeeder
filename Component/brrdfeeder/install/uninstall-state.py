#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Read-only validation of additional package state; embedded in uninstall.sh.

The shell checks ancestors and mounts before calling this validator. No recovery,
unlink, chmod, or credential reads happen here. A pending hardlink exception is
limited to the two writer-created names in the private updater directory.
"""
import base64
import binascii
import os
from pathlib import Path
import re
import stat
import sys

STATE = Path('/var/lib/brrdfeeder')
PRIVATE = Path('/var/lib/brrdfeeder-updater')
CONFIG = Path('/etc/brrdfeeder')


def require(condition, reason, path):
    if not condition:
        raise ValueError(reason + ': ' + str(path))


def owned_file(path, uid, gid, modes, links=(1,)):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode), 'not a regular package file', path)
    require((info.st_uid, info.st_gid) == (uid, gid), 'wrong package file owner', path)
    require(stat.S_IMODE(info.st_mode) in modes, 'wrong package file mode', path)
    require(info.st_nlink in links, 'unexpected package file hardlinks', path)
    return info


def owned_directory(path, uid, gid, modes):
    info = path.lstat()
    require(stat.S_ISDIR(info.st_mode), 'not a package directory', path)
    require((info.st_uid, info.st_gid) == (uid, gid), 'wrong package directory owner', path)
    require(stat.S_IMODE(info.st_mode) in modes, 'wrong package directory mode', path)


def go_temporary(name, prefix):
    # os.CreateTemp uses a decimal uint32 suffix. No extra path components,
    # arbitrary suffixes, or leading zeroes are writer-generated names.
    match = re.fullmatch(re.escape(prefix) + r'(0|[1-9][0-9]{0,9})', name)
    return match is not None and int(match[1]) <= 0xffffffff


def python_temporary(name, prefix):
    return re.fullmatch(re.escape(prefix) + r'[a-z0-9_]{8}', name) is not None


def renewal_temporary(name):
    if not re.fullmatch(r'\.renew-U[A-Z2-7]{55}', name):
        return False
    raw = base64.b32decode(name[len('.renew-'):])
    # NATS public user keys: prefix byte, 32-byte public key, CRC16-XMODEM (LE).
    return (len(raw) == 35 and raw[0] == 20 << 3
            and int.from_bytes(raw[-2:], 'little') == binascii.crc_hqx(raw[:-2], 0))


def configured(kind):
    import yaml
    path = CONFIG / 'config.yaml'
    info = owned_file(path, 0, 0, (0o600, 0o640, 0o644))
    require(info.st_size <= 1024 * 1024, 'config too large to identify optional state', path)
    # SafeLoader does not construct arbitrary objects. Only fixed installed
    # placements are recognized; a configured path never expands deletion roots.
    try:
        with path.open('rb') as stream:
            config = yaml.safe_load(stream) or {}
    except yaml.YAMLError:
        raise ValueError('cannot parse config to identify optional state: ' + str(path)) from None
    require(isinstance(config, dict), 'config is not a mapping', path)
    if kind == 'status':
        node = config.get('node')
        return isinstance(node, dict) and node.get('status_file') == str(STATE / 'status.json')
    capture = config.get('capture')
    savefile = capture.get('savefile') if isinstance(capture, dict) else None
    return (isinstance(savefile, dict)
            and savefile.get('directory', str(STATE / 'capture')) == str(STATE / 'capture'))


def pending(path):
    owned_directory(PRIVATE, 0, 0, (0o700,))
    info = owned_file(path, 0, 0, (0o600,), links=(1, 2))
    if info.st_nlink == 1:
        return
    target = PRIVATE / 'pending_update.json'
    if path == target:
        candidates = [p for p in PRIVATE.iterdir() if go_temporary(p.name, '.pending-')]
    else:
        candidates = [target]
    matches = []
    for candidate in candidates:
        try:
            other = candidate.lstat()
        except FileNotFoundError:
            continue
        if (other.st_dev, other.st_ino) != (info.st_dev, info.st_ino):
            continue
        owned_file(candidate, 0, 0, (0o600,), links=(2,))
        matches.append(candidate)
    require(len(matches) == 1 and matches[0] != path,
            'pending hardlinks must be the exact private staging pair', path)


def validate(path, uid, gid):
    name = path.name
    if path == STATE / 'release_attempt.json':
        owned_file(path, 0, 0, (0o644,))
    elif path == PRIVATE / 'attempt.json':
        owned_directory(PRIVATE, 0, 0, (0o700,))
        owned_file(path, 0, 0, (0o600,))
    elif path == STATE / 'credentials':
        owned_directory(path, uid, gid, (0o700,))
        for child in path.iterdir():
            require(child.name in ('brrdfeeder.creds', 'previous.creds', '.renew.lock')
                    or renewal_temporary(child.name), 'unknown renewal artifact', child)
            owned_file(child, uid, gid, (0o600,))
    elif path == STATE / 'outage':
        owned_directory(path, uid, gid, (0o700,))
        for child in path.iterdir():
            segment = re.fullmatch(r'[0-9]{20}\.seg', child.name)
            require(child.name in ('lock', 'cursor.json', 'cursor.json.tmp', 'gaps.slots')
                    or segment is not None and int(child.name[:20]) <= 0xffffffffffffffff,
                    'unknown outage spool artifact', child)
            owned_file(child, uid, gid, (0o600,))
    elif path == STATE / 'capture':
        require(configured('capture'), 'capture savefile is not configured here', path)
        # create_dir_all inherits the normal service umask; sensor files remain
        # private even with a traversable directory. No group/other writes.
        owned_directory(path, uid, gid, (0o700, 0o750, 0o755))
        for child in path.iterdir():
            require(re.fullmatch(r'capture-(0[0-9]|1[0-5])\.pcap', child.name),
                    'unknown capture artifact', child)
            owned_file(child, uid, gid, (0o600,))
    elif path.parent == STATE and (name == 'status.json' or
            re.fullmatch(r'status\.json\.tmp\.[1-9][0-9]*\.(0|[1-9][0-9]*)', name)):
        if name != 'status.json':
            pid, sequence = map(int, name.split('.')[-2:])
            require(pid <= 0xffffffff and sequence <= 0xffffffffffffffff,
                    'status temporary PID/sequence out of range', path)
        require(configured('status'), 'status is not configured here', path)
        owned_file(path, uid, gid, (0o644,) if name == 'status.json' else (0o600, 0o644))
    elif path.parent == PRIVATE and (name == 'pending_update.json' or go_temporary(name, '.pending-')):
        pending(path)
    elif path.parent in (PRIVATE, CONFIG) and python_temporary(name, '.host-recover-'):
        if path.parent == PRIVATE:
            owned_directory(PRIVATE, 0, 0, (0o700,))
        owned_file(path, 0, 0, (0o600,))
    elif path.parent == CONFIG and go_temporary(name, '.update-'):
        owned_file(path, 0, 0, (0o600, 0o644))
    elif path.parent == CONFIG and python_temporary(name, '.config.yaml-'):
        owned_file(path, 0, 0, (0o600, 0o640, 0o644))
    else:
        raise ValueError('unknown package state: ' + str(path))


if __name__ == '__main__':
    try:
        validate(Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3]))
    except (OSError, ValueError, ImportError) as error:
        print('REFUSED: ' + str(error), file=sys.stderr)
        sys.exit(1)
