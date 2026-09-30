#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Reject nested Cargo locks, including untracked and ignored working-tree paths."""
import argparse
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[5]
WORKSPACE = Path('Component/aviary')
LOCK = WORKSPACE / 'Cargo.lock'


def check(root):
    if not (root / LOCK).is_file():
        print(f'FAIL: missing workspace root lock {LOCK.as_posix()}', file=sys.stderr)
        return 1

    def scan_error(error):
        raise error

    offending = []
    try:
        # No git index or ignore rules: inspect newly created paths at every
        # depth. Directory symlinks are not followed (external trees/cycles).
        for directory, dirs, files in os.walk(root / WORKSPACE, onerror=scan_error, followlinks=False):
            if 'Cargo.lock' in files or 'Cargo.lock' in dirs:
                path = (Path(directory) / 'Cargo.lock').relative_to(root)
                if path != LOCK:
                    offending.append(path.as_posix())
    except OSError as error:
        print(f'FAIL: cannot inspect {WORKSPACE.as_posix()}: {error}', file=sys.stderr)
        return 1
    if offending:
        print('FAIL: nested Cargo.lock paths are not allowed:', file=sys.stderr)
        for path in sorted(offending):
            print(f'  {path}', file=sys.stderr)
        print(f'Use the workspace root lock: {LOCK.as_posix()}', file=sys.stderr)
        return 1
    print(f'PASS: only the workspace root lock exists: {LOCK.as_posix()}')
    return 0


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    args = parser.parse_args()
    raise SystemExit(check(args.root.resolve()))
