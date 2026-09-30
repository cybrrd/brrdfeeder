#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Lint the entire public repository, or its exact export in the private mirror."""
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[5]
assert subprocess.check_output(['reuse','--version'],text=True).splitlines()[0]=='reuse, version 6.2.0'
scope=ROOT/'governance/github-migration/public-cut/public-scope-paths.txt'
if scope.exists():
    # Private CYB1 contains unrelated products and private evidence. Apply the
    # exact publication manifest, not blanket licensing to the whole monorepo.
    paths=subprocess.check_output(['git','ls-files','-z','--cached','--others',
        '--exclude-standard','--',*scope.read_text().splitlines()],cwd=ROOT).decode().split('\0')
    with tempfile.TemporaryDirectory(prefix='public-reuse-') as folder:
        root=Path(folder)
        for name in filter(None,paths):
            target=root/name
            target.parent.mkdir(parents=True,exist_ok=True)
            shutil.copyfile(ROOT/name,target)
        raise SystemExit(subprocess.call(['reuse','--root',str(root),'lint']))
raise SystemExit(subprocess.call(['reuse','--root',str(ROOT),'lint']))
