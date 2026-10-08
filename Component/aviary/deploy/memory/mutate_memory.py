#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Run compiling behavioral negative controls on isolated source copies."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
rows = []


def run(name, command, cwd, env=None):
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, text=True, timeout=180)
    output = result.stdout + result.stderr
    # A syntax/compiler/runtime setup failure is never a killed mutant.
    killed = result.returncode != 0 and any(marker in output for marker in ('AssertionError:', '--- FAIL:', 'test result: FAILED'))
    rows.append({'control': name, 'behavioral_failure': killed})
    if not killed:
        raise SystemExit(name + ' survived or failed outside an assertion:\n' + output[-4000:])


with tempfile.TemporaryDirectory(prefix='brrd-memory-mutants-') as folder:
    scratch = Path(folder)
    for name, old, new in (
        ('cap-is-enforced', 'MemoryMax=256M', 'MemoryMax=infinity'),
        ('payload-stays-in-service-cgroup', 'PodmanArgs=--cgroups=split', 'PodmanArgs=--cgroups=enabled'),
        ('oom-kills-service-group', 'OOMPolicy=kill', 'OOMPolicy=continue'),
        ('restart-loop-is-bounded', 'StartLimitBurst=3', 'StartLimitBurst=0'),
    ):
        project = scratch / name
        target = project / 'Component/aviary/deploy/memory'
        target.mkdir(parents=True)
        for source in (HERE / 'host-memory.py', HERE / 'test_memory.py',
                       HERE.parent / 'quadlet/brrdfeeder-engine.container',
                       ROOT / 'Component/aviary/tools/check-quadlet-cidfile.py',
                       ROOT / 'Component/brrdfeeder/install/brrdfeeder-install.sh'):
            dest = project / source.relative_to(ROOT)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, dest)
        unit = project / 'Component/aviary/deploy/quadlet/brrdfeeder-engine.container'
        source = unit.read_text()
        assert source.count(old) == 1, name
        unit.write_text(source.replace(old, new))
        run(name, ['python3', '-m', 'unittest', 'test_memory.DeploymentContracts.test_generated_unit_caps_payload_and_restart_policy'], target)

    for name, old, new in (
        ('bytes-not-decimal-kb', 'int(value) * 1024', 'int(value) * 1000'),
        ('normal-exit-is-not-oom', "if result != 'oom-kill':", "if False:"),
        ('one-event-per-invocation', "if doc['last_invocation'] == invocation:", 'if False:'),
        ('console-uid-is-required', 'if process.stat().st_uid != uid:', 'if False:'),
        ('journal-symlink-is-not-usage', '(Path(directory) / name).lstat()', '(Path(directory) / name).stat()'),
    ):
        target = scratch / name / 'Component/aviary/deploy/memory'
        target.mkdir(parents=True)
        source = (HERE / 'host-memory.py').read_text()
        assert source.count(old) == 1, name
        (target / 'host-memory.py').write_text(source.replace(old, new))
        shutil.copyfile(HERE / 'test_memory.py', target / 'test_memory.py')
        run(name, ['python3', '-m', 'unittest', 'test_memory.MemoryContracts'], target)

    for name, old, new in (
        ('console-warns', 'm.Events != nil && *m.Events > 0', 'm.Events != nil && false'),
        ('console-expires-host-memory', 'seconds-*host.Sampled > 90', 'seconds-*host.Sampled > 9000'),
    ):
        target = scratch / name
        shutil.copytree(ROOT / 'Component/brrdhouse', target, ignore=shutil.ignore_patterns('.git', '__pycache__'))
        source = (target / 'memory.go').read_text()
        assert source.count(old) == 1, name
        (target / 'memory.go').write_text(source.replace(old, new))
        run(name, ['go', 'test', '-run', 'TestMemoryEventIsVisible|TestHostMemoryRemainsVisibleWithoutEngine', '-count=1', './...'], target)

    locked = tomllib.loads((ROOT / 'Component/aviary/Cargo.lock').read_text())
    versions = {p['name']: p['version'] for p in locked['package'] if p['name'] in ('serde', 'serde_json', 'libc')}
    for name, old, new in (
        ('rust-bytes', '.checked_mul(1024)', '.checked_mul(1000)'),
        ('rust-snapshot-expires', '> 90', '> 9000'),
        ('rust-boot-bound', 'snapshot.boot_id != boot.trim()', 'false'),
    ):
        target = scratch / name
        target.mkdir()
        source = (ROOT / 'Component/aviary/engine/src/memory.rs').read_text()
        assert source.count(old) == 1, name
        (target / 'lib.rs').write_text(source.replace(old, new))
        (target / 'Cargo.toml').write_text('[package]\nname="memory-negative-control"\nversion="0.0.0"\nedition="2021"\n[lib]\npath="lib.rs"\n[dependencies]\n' +
            '\n'.join(f'{key} = {{ version = "={value}"' + (', features = ["derive"]' if key == 'serde' else '') + ' }' for key, value in versions.items()) + '\n')
        run(name, ['cargo', 'test', '--offline', '--target-dir', str(scratch / 'rust-target')], target)
print(json.dumps(rows, indent=2))
