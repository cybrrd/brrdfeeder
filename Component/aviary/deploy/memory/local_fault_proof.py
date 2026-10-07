#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Opt-in LOCAL transient user-unit proof; never a field/native-engine soak.

Uses an already-cached fixture image, no network/pull, a capped split cgroup,
one intentional OOM followed by a 60-second 32-MiB fixture soak. Artifacts stay
under --out. Only this run's named container and transient service are removed.
"""
import argparse
import importlib.util
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True


def command(args, **kwargs):
    return subprocess.run(args, check=True, capture_output=True, text=True, timeout=20, **kwargs).stdout


def payload(root):
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    # The host controller verifies the KERNEL ancestor limit before arming.
    deadline = time.monotonic() + 20
    while not (root / 'armed').exists():
        if time.monotonic() > deadline:
            raise RuntimeError('not armed; refusing allocation')
        time.sleep(0.1)
    attempt = root / 'first-attempt'
    first = not attempt.exists() or (root / 'repeat-oom').exists()
    if first:
        attempt.write_text('intentional-capped-oom\n')
    blocks = [bytearray(1024 * 1024) for _ in range(32)]
    for block in blocks:
        block[::4096] = b'x' * 256
    if first:
        # Bounded request > 256 MiB; never run without the ancestor cap.
        print('ARMED: allocating past service MemoryMax', flush=True)
        for _ in range(400):
            block = bytearray(1024 * 1024)
            block[::4096] = b'x' * 256
            blocks.append(block)
        raise RuntimeError('survived > cap: enforcement failed')
    start = time.monotonic()
    while time.monotonic() - start < 60:
        assert sum(block[0] for block in blocks) == 32 * ord('x')
        time.sleep(0.2)
    (root / 'soak-complete').write_text(json.dumps({'seconds': time.monotonic() - start, 'rss_payload_bytes': 32 * 1024 * 1024}))
    print('SOAK_COMPLETE', flush=True)
    while True:
        time.sleep(1)  # Parent performs an intentional systemd stop.


def stop(root):
    spec = importlib.util.spec_from_file_location('memory', HERE / 'host-memory.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    result = os.environ.get('SERVICE_RESULT', '')
    invocation = os.environ.get('INVOCATION_ID', '')
    with (root / 'stops.jsonl').open('a') as stream:
        stream.write(json.dumps({'result': result, 'invocation': invocation}) + '\n')
    try:
        module.record_stop(root / 'state', result, invocation)
    finally:
        # Quadlet appends this exact cleanup AFTER the custom stop hook. The
        # first prototype omitted it and incurred an extra Podman recovery
        # failure; keep that receipt, but do not call it a clean restart proof.
        cleanup = subprocess.run(['/usr/bin/podman', 'rm', '-v', '-f', '-i', '--cidfile=' + str(root / 'container.cid')], capture_output=True, text=True, timeout=10)
        with (root / 'cleanup.log').open('a') as stream:
            stream.write(cleanup.stdout + cleanup.stderr)


def run(out, image, repeat_oom=False):
    out.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='memory-local-', dir=out)).resolve()
    state = root / 'state'
    state.mkdir(mode=0o755)
    state.chmod(0o755)
    if repeat_oom:
        (root / 'repeat-oom').write_text('three starts then start-limit-hit\n')
    unit = f'codex-brrd-memory-test-{os.getpid()}.service'
    container = unit.removesuffix('.service')
    image_id = command(['podman', 'image', 'inspect', '--format', '{{.Id}}', image]).strip()
    (root / 'input.json').write_text(json.dumps({'image_id': image_id, 'unit': unit, 'scope': 'local user-unit fixture; NOT native engine'}, indent=2))
    print('Retained proof directory: ' + str(root), flush=True)
    print('WARNING: TEST ONLY — intentional capped OOM on this local shared host; '
          + ('three OOM events' if repeat_oom else 'one OOM event')
          + '. Shared-host memory alerts may fire. Unit/container: ' + container, flush=True)
    try:
        command(['systemd-run', '--user', '--unit=' + unit,
                 '--description=TEST ONLY: intentional capped OOM; temporary BRRDfeeder memory fixture',
                 '--property=MemoryAccounting=yes', '--property=MemoryMax=256M',
                 '--property=MemorySwapMax=0', '--property=OOMPolicy=kill',
                 '--property=Delegate=yes', '--property=Restart=always',
                 '--property=Type=notify', '--property=NotifyAccess=all', '--property=KillMode=mixed',
                 '--property=RestartSec=5', '--property=StartLimitIntervalSec=300',
                 '--property=StartLimitBurst=3', '--property=TimeoutStopSec=10',
                 '--property=ExecStop=/usr/bin/podman rm -v -f -i --cidfile=' + str(root / 'container.cid'),
                 '--property=ExecStopPost=/usr/bin/python3 ' + str(Path(__file__).resolve()) + ' stop ' + str(root),
                 '/usr/bin/podman', 'run', '--rm', '--pull=never', '--network=none',
                 '--name=' + container, '--replace', '--cgroups=split', '--read-only',
                 '--cidfile=' + str(root / 'container.cid'), '--sdnotify=conmon', '-d',
                 '--pids-limit=64', '-v', str(root) + ':/proof:rw',
                 '-v', str(Path(__file__).resolve()) + ':/proof-payload.py:ro',
                 image_id, 'python3', '/proof-payload.py', 'payload', '/proof'])
        props = command(['systemctl', '--user', 'show', unit, '--property=ControlGroup,MemoryMax,MemorySwapMax,OOMPolicy,Delegate'])
        (root / 'properties.txt').write_text(props)
        values = dict(line.split('=', 1) for line in props.splitlines())
        assert values['MemoryMax'] == '268435456' and values['MemorySwapMax'] == '0' and values['OOMPolicy'] == 'kill', props
        group = Path('/sys/fs/cgroup') / values['ControlGroup'].lstrip('/')
        assert (group / 'memory.max').read_text().strip() == '268435456'
        assert (group / 'memory.swap.max').read_text().strip() == '0'
        (root / 'armed').write_text('kernel ancestor memory.max=268435456 and memory.swap.max=0\n')
        deadline = time.monotonic() + 120
        while not (root / 'soak-complete').exists():
            if repeat_oom:
                status = command(['systemctl', '--user', 'show', unit, '--property=ActiveState,NRestarts'])
                if 'ActiveState=failed' in status.splitlines() and 'NRestarts=3' in status.splitlines():
                    # systemd may retain Result=oom-kill instead of replacing
                    # it with start-limit-hit. Require the actual failed state,
                    # three receipts, the refusal log and two quiet intervals.
                    time.sleep(10)
                    break
            if time.monotonic() > deadline:
                raise RuntimeError('restart/soak did not complete')
            time.sleep(0.2)
        count = json.loads((state / 'events.json').read_text())['memory_cap_events']
        assert count == (3 if repeat_oom else 1), count
        report = {'scope': 'local user-unit fixture; NOT native engine', 'memory_cap_events': count,
                  'soak': None if repeat_oom else json.loads((root / 'soak-complete').read_text()),
                  'service': command(['systemctl', '--user', 'show', unit, '--property=NRestarts,ActiveState,Result,MemoryCurrent,MemoryPeak,InvocationID'])}
        if repeat_oom:
            assert 'ActiveState=failed' in report['service'].splitlines(), report
            assert 'NRestarts=3' in report['service'].splitlines(), report
            log = command(['journalctl', '--user-unit=' + unit, '--no-pager'])
            assert 'Start request repeated too quickly.' in log, log
        else:
            assert 'NRestarts=1' in report['service'].splitlines(), report
        (root / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2), flush=True)
    finally:
        subprocess.run(['systemctl', '--user', 'stop', unit], capture_output=True, timeout=20)
        logs = subprocess.run(['journalctl', '--user-unit=' + unit, '--no-pager', '-o', 'short-monotonic'], capture_output=True, text=True, timeout=20)
        (root / 'journal.log').write_text(logs.stdout + logs.stderr)
        subprocess.run(['podman', 'rm', '-f', container], capture_output=True, timeout=20)
        subprocess.run(['systemctl', '--user', 'reset-failed', unit], capture_output=True, timeout=20)


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] in ('payload', 'stop'):
        (payload if sys.argv[1] == 'payload' else stop)(Path(sys.argv[2]))
    else:
        parser = argparse.ArgumentParser()
        parser.add_argument('--out', type=Path, required=True)
        parser.add_argument('--image', required=True, help='already-cached offline fixture image with python3')
        parser.add_argument('--repeat-oom', action='store_true', help='prove the three-start latch instead of the post-restart soak')
        args = parser.parse_args()
        run(args.out, args.image, args.repeat_oom)
