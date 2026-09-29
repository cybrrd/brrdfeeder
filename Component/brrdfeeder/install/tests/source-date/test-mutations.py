#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Prove helper contract tests reject lost epoch plumbing (not real image proof)."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

repo = Path(__file__).resolve().parents[5]
helper = repo / 'Component/aviary/tools/build-arm64.sh'
mutations = {
    'caller-epoch-not-overridden': ('export SOURCE_DATE_EPOCH="$epoch"', ': # mutant: trust caller epoch', 'ambient-past'),
    'epoch-not-passed-to-build': ('  --build-arg "SOURCE_DATE_EPOCH=$epoch" \\\n', '', 'source-date-changes'),
    'wrong-account-day-accepted': ('[[ $(<"$build_tmp/shadow-lastchg.txt") == "$((epoch / 86400))" ]] || die \'image account date is not the source commit day\'',
                                   ': # mutant: no account-date check', 'wrong-shadow-day'),
}
results = []
for name, (before, after, diagnostic) in mutations.items():
    with tempfile.TemporaryDirectory(prefix='source-date-mutant-') as directory:
        root = Path(directory)
        tools = root / 'Component/aviary/tools'
        engine = tools.parent / 'engine'
        tools.mkdir(parents=True)
        engine.mkdir()
        shutil.copyfile(helper.with_name('test-build-arm64.py'), tools / 'test-build-arm64.py')
        shutil.copyfile(helper.parent.parent / 'engine/Containerfile', engine / 'Containerfile')
        original = helper.read_text()
        assert original.count(before) == 1, name
        (tools / helper.name).write_text(original.replace(before, after))
        result = subprocess.run([sys.executable, str(tools / 'test-build-arm64.py')], capture_output=True, text=True)
        assert result.returncode == 1 and 'FAIL:' in result.stderr and diagnostic in result.stderr, result.stderr
        results.append(dict(mutant=name, suite_exit=result.returncode, killed=True, diagnostic=result.stderr))
print(json.dumps(results, indent=2))
