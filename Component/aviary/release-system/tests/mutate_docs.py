#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Documentation regression controls, not runtime/native update proof."""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

source = Path(__file__).resolve().parents[1]
cases = [
    ('private-path', '[build-host.sh](build-host.sh)', 'governance/reviews/absent/build-host.sh',
     'test_public_readme_has_no_absent_private_paths', 'public README references absent private proof paths'),
    ('wrong-compiler', 'Go 1.27.0', 'Go 0.0.0',
     'test_standalone_compiler_matches_build_script', 'README does not name the standalone script compiler'),
    ('publisher-boundary', 'low-level signer client', 'complete promotion authority',
     'test_signer_client_is_not_approval_or_deployment', 'publisher boundary is undocumented'),
    ('unconditional-join', 'Classify the installed baseline first', 'Always uninstall every installed baseline',
     'test_join_is_conditional', 'README unconditionally directs uninstall'),
]
results = []
for name, before, after, test, message in cases:
    with tempfile.TemporaryDirectory(prefix='pr6-doc-mutant-') as directory:
        root = Path(directory)
        (root / 'tests').mkdir()
        for file in ('README.md', 'PROOF.md', 'build-host.sh'):
            shutil.copyfile(source / file, root / file)
        shutil.copyfile(source / 'tests/test_docs.py', root / 'tests/test_docs.py')
        path = root / 'README.md'
        text = path.read_text()
        assert text.count(before) == 1, 'stale documentation mutation: ' + name
        path.write_text(text.replace(before, after))
        run = subprocess.run(['python3', 'tests/test_docs.py', 'Docs.' + test], cwd=root,
                             stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        killed = run.returncode == 1 and 'FAIL:' in run.stdout and message in run.stdout
        results.append(dict(control=name, assertion_failure=killed))
        if not killed:
            print(run.stdout)
print(json.dumps(results, indent=2))
raise SystemExit(not all(r['assertion_failure'] for r in results))
