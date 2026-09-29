#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Stable host supervisor, intentionally NOT replaced by updater.json.

No containers, network, keys, imports outside the standard library or shell.
The retained binary's local checksum is a corruption guard; its authority is
the prior pinned install/signed update. Root is the trusted state owner.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile

LIMIT = 32 << 20
REPLY = b"brrdfeeder-release self-test v1\n"


def trusted(path, owner, limit=LIMIT):
    path = Path(path)
    for parent in path.parents:
        st = parent.lstat()
        if not stat.S_ISDIR(st.st_mode) or st.st_uid != owner or st.st_mode & 0o022:
            raise RuntimeError(f"unsafe host path parent: {parent}")
        if parent == Path("/"):
            break
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode) or st.st_uid != owner or st.st_mode & 0o022 or st.st_size > limit:
            raise RuntimeError(f"unsafe host file: {path}")
        with os.fdopen(fd, "rb", closefd=False) as f:
            raw = f.read(limit + 1)
        if len(raw) > limit:
            raise RuntimeError("host file exceeds limit")
        return raw
    finally:
        os.close(fd)


def sync_dir(path):
    fd = os.open(path, os.O_DIRECTORY | os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic(path, raw, mode):
    fd, tmp = tempfile.mkstemp(prefix=".host-recover-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as f:
            os.fchmod(f.fileno(), mode)
            f.write(raw)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
        sync_dir(path.parent)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def starts(binary):
    try:
        r = subprocess.run([str(binary), "self-test"], stdout=subprocess.PIPE,
                           stderr=subprocess.DEVNULL, timeout=5, check=False)
        return r.returncode == 0 and r.stdout == REPLY
    except (OSError, subprocess.TimeoutExpired):
        return False


def recover(binary, private, receipt, owner=0, read=trusted):
    tx = private / "host-transaction.json"
    previous = Path(str(binary) + ".previous")
    journal = None
    try:
        journal = json.loads(read(tx, owner, 4096))
    except FileNotFoundError:
        pass
    # Never invoke an unconfirmed candidate after a kill/reboot. A completed
    # candidate is also checked on every invocation so loader failures fall back.
    try:
        read(binary, owner)
        present = True
    except FileNotFoundError:
        present = False
    if journal is None and present and starts(binary):
        return
    old = read(previous, owner)
    expected = read(Path(str(previous) + ".sha256"), owner, 65).decode().strip()
    digest = hashlib.sha256(old).hexdigest()
    if digest != expected or (journal is not None and journal.get("previous_sha256") != digest):
        raise RuntimeError("retained updater checksum mismatch; refuse recovery")
    if not starts(previous):
        raise RuntimeError("retained updater cannot start; package pins left untouched")
    atomic(binary, old, 0o755)
    atomic(receipt, (digest + "  " + str(binary) + "\n").encode(), 0o600)
    if journal is not None:
        tx.unlink()
        sync_dir(private)
    print("host-updater: restored previous executable locally; no network", file=sys.stderr)


def main(args):
    if os.geteuid() != 0:
        raise RuntimeError("host supervisor requires root")
    private = Path("/var/lib/brrdfeeder-updater")
    st = private.lstat()
    if not stat.S_ISDIR(st.st_mode) or st.st_uid != 0 or st.st_mode & 0o077:
        raise RuntimeError("unsafe private updater directory")
    lock = os.open(private / "launch.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        binary = Path("/usr/local/libexec/brrdfeeder-release")
        recover(binary, private, Path("/etc/brrdfeeder/.updater-helper.sha256"))
        return subprocess.call([str(binary), *args])
    finally:
        os.close(lock)


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"host-updater: {exc}", file=sys.stderr)
        sys.exit(1)
