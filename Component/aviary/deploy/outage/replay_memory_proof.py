#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Local capped replay proof. No network, production node, build or image pull.

Usage: replay_memory_proof.py ENGINE_TEST_BINARY NEW_OUTPUT_DIRECTORY
Requires user systemd, rootless Podman, cached TEST_OS_IMAGE and an ext4 output
location. The ignored Rust fixture seeds 256 legal 1-MiB records, retries across
three fresh processes, then drains all records with 128 MiB anonymous overhead
and 16 MiB tmpfs reserved. Podman/conmon stay inside the 256-MiB service cgroup.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

TEST = 'outage_spool::tests::replay_memory_fixture'
IMAGE = os.environ.get('TEST_OS_IMAGE', 'sha256:5c43fb9e961bcdd08c51d2858c6ad3523f66ebbdb9f2fd8effc9cd16e4b86338')
LIMIT = 256 * 1024 * 1024


def run(args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def memory_group():
    lines = Path('/proc/self/cgroup').read_text().splitlines()
    relative = next(line[3:] for line in lines if line.startswith('0::'))
    return Path('/sys/fs/cgroup') / relative.lstrip('/')


def worker(binary, output):
    group = memory_group()
    assert int((group / 'memory.max').read_text()) == LIMIT
    assert int((group / 'memory.swap.max').read_text()) == 0
    samples = []
    phases = []
    observed = set()
    for index, mode in enumerate(['retry', 'retry', 'retry', 'drain']):
        args = ['podman', 'run', '--rm', '--pull=never', '--network=none',
                '--cgroups=disabled', '--read-only', '--security-opt=no-new-privileges',
                '--cap-drop=all', '--tmpfs=/replay-reserve:rw,size=16m,mode=1777',
                '-v', str(binary) + ':/engine-test:ro',
                '-v', str(output) + ':/fixture:rw',
                '-v', '/lib/x86_64-linux-gnu:/hostlib:ro',
                '-e', 'B06_REPLAY_ROOT=/fixture', '-e', 'B06_REPLAY_MODE=' + mode,
                '-e', 'B06_REPLAY_TMPFS=/replay-reserve',
                '--entrypoint=/hostlib/ld-linux-x86-64.so.2', IMAGE,
                '--library-path', '/hostlib', '/engine-test', TEST,
                '--exact', '--ignored', '--nocapture', '--test-threads=1']
        with (output / f'{index}-{mode}.log').open('w') as log:
            process = subprocess.Popen(args, stdout=log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 120
            try:
                while process.poll() is None:
                    assert time.monotonic() < deadline, 'capped replay timed out'
                    samples.append(int((group / 'memory.current').read_text()))
                    for pid in (group / 'cgroup.procs').read_text().split():
                        try:
                            observed.add(Path('/proc', pid, 'comm').read_text().strip())
                        except FileNotFoundError:
                            pass
                    time.sleep(.02)
            finally:
                if process.poll() is None:
                    process.kill()
                code = process.wait()
        phases.append({'mode': mode, 'exit': code})
        assert code == 0, f'{mode} failed; see {index}-{mode}.log'
    events = dict(line.split() for line in (group / 'memory.events').read_text().splitlines())
    peak = int((group / 'memory.peak').read_text())
    result = {'scope': 'local synthetic replay, not native Pi qualification',
              'memory_max': LIMIT, 'memory_swap_max': 0,
              'cgroup': str(group), 'memory_peak': peak,
              'sampled_memory_peak': max(samples), 'events': events,
              'reserved_anon_bytes': 128 * 1024 * 1024,
              'reserved_tmpfs_bytes': 16 * 1024 * 1024,
              'replay_batch_bytes': 4 * 1024 * 1024,
              'observed_cgroup_process_names': sorted(observed),
              'real_podman_conmon_in_same_cgroup': {'podman', 'conmon'} <= observed,
              'phases': phases, 'pass': False}
    assert result['real_podman_conmon_in_same_cgroup'], result
    assert int(events['oom']) == 0 and int(events['oom_kill']) == 0, result
    # Demand measured headroom, not merely survival just beneath MemoryMax.
    result['pass'] = peak < 224 * 1024 * 1024
    (output / 'memory-result.json').write_text(json.dumps(result, indent=2) + '\n')
    assert result['pass'], result
    print(json.dumps(result, indent=2), flush=True)


def proof(binary, output):
    assert binary.is_file()
    output.mkdir(parents=True, exist_ok=False)
    filesystem = run(['stat', '-f', '-c', '%T', str(output)], capture_output=True).stdout.strip()
    assert filesystem not in ('tmpfs', 'ramfs'), 'backlog must be on real disk, not charged tmpfs'
    with (output / 'seed.log').open('w') as log:
        run([str(binary), TEST, '--exact', '--ignored', '--nocapture'],
            env=dict(os.environ, B06_REPLAY_ROOT=str(output), B06_REPLAY_MODE='seed'),
            stdout=log, stderr=subprocess.STDOUT, timeout=120)
    unit = f'b06-replay-memory-{os.getpid()}.service'
    try:
        run(['systemd-run', '--user', '--wait', '--pipe', '--unit=' + unit,
             '--property=MemoryAccounting=yes', '--property=MemoryMax=256M',
             '--property=MemorySwapMax=0', '--property=OOMPolicy=kill',
             '--property=KillMode=control-group',
             '--setenv=TEST_OS_IMAGE=' + IMAGE,
             sys.executable, str(Path(__file__).resolve()), 'worker', str(binary), str(output)],
            timeout=600)
    finally:
        subprocess.run(['systemctl', '--user', 'stop', unit], capture_output=True)
        subprocess.run(['systemctl', '--user', 'reset-failed', unit], capture_output=True)


if __name__ == '__main__':
    if len(sys.argv) == 4 and sys.argv[1] == 'worker':
        worker(Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve())
    elif len(sys.argv) == 3:
        proof(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve())
    else:
        raise SystemExit(__doc__)
