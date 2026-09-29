#!/usr/bin/env python3
"""Two ignored engine tests, local loopback broker only, cached image/no build."""
import os
from pathlib import Path
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[4]
image=os.environ.get('TEST_NATS_IMAGE','docker.io/library/nats@sha256:e4bf19f15fd3218814a4e3c9e0064e1334bd8aa20d5984b9f1a0afd084f8cc00')
with tempfile.TemporaryDirectory(prefix='self-update-broker-') as tmp:
    binary=Path(tmp)/'nats-server'
    cid=subprocess.check_output(['podman','create','--pull=never','--network=none',image],text=True).strip()
    try:
        subprocess.run(['podman','cp',cid+':/usr/local/bin/nats-server',str(binary)],check=True)
    finally:
        subprocess.run(['podman','rm',cid],check=True)
    binary.chmod(0o700)
    print('Cached fixture image:',image,flush=True)
    subprocess.run([str(binary),'--version'],check=True)
    # The tests generate ephemeral passwords, ports, streams and storage under
    # their Scratch directories. Both broker configs bind ONLY 127.0.0.1.
    result=subprocess.run(['cargo','test','--offline','--locked','-p','engine','upward::tests::broker_',
                           '--','--ignored','--nocapture','--test-threads=1'],
                          cwd=ROOT/'Component/aviary',env=dict(os.environ,D40_NATS_SERVER=str(binary)),timeout=180)
    raise SystemExit(result.returncode)
