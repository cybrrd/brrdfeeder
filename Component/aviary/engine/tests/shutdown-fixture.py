#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Execute the exact production shutdown tail with deterministic stalled tasks.

No radio, NATS, GPS, root or production unit is used. --systemd additionally
proves an isolated transient USER unit with Restart=always ends inactive/success.
The temporary Cargo project and all build output are removed on exit.
"""
import argparse
import os
from pathlib import Path
import signal
import re
import subprocess
import tempfile
import time

p=argparse.ArgumentParser(); p.add_argument('--systemd',action='store_true'); args=p.parse_args()
source=(Path(__file__).resolve().parents[1]/'src/main.rs').read_text()
tail=source[source.index('    let mut sigterm = tokio::signal::unix::signal('):]
assert tail.rstrip().endswith('}')
tail,count=re.subn(r'(?m)^(    (?:let intentional_stop = )?tokio::select! \{)',
                  r'    println!("READY");\n\1',tail,count=1)
assert count == 1
prefix=r'''
struct Cancel;
impl Cancel { fn cancel(&self) {} }
struct Status(bool);
impl Status { async fn shutdown(self) -> std::io::Result<()> {
    if self.0 { std::future::pending::<()>().await; } Ok(())
} }
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stage = std::env::var("STALL").unwrap_or_default();
    let crash = stage == "crash";
    let capture_future = async { if !crash { std::future::pending::<()>().await; } };
    tokio::pin!(capture_future);
    let gps_cancel = Cancel; let upward_cancel = Cancel;
    let heartbeat_task = tokio::spawn(std::future::pending::<()>());
    let upward_task = if stage == "upward" { Some(tokio::spawn(std::future::pending::<()>())) } else { None };
    let status_cleanup = Some(Status(stage == "status"));
    let ble_keeper = if stage == "ble" { Some(tokio::spawn(std::future::pending::<()>())) } else { None };
'''
with tempfile.TemporaryDirectory(prefix='brrd-shutdown-') as folder:
    root=Path(folder); (root/'src').mkdir()
    (root/'Cargo.toml').write_text('[package]\nname="shutdown-fixture"\nversion="0.0.0"\nedition="2021"\n[dependencies]\ntokio={version="1",features=["full"]}\n')
    (root/'src/main.rs').write_text(prefix+tail)
    env=dict(os.environ,CARGO_TARGET_DIR=str(root/'target'))
    subprocess.run(['cargo','build','--offline','--manifest-path',str(root/'Cargo.toml')],env=env,check=True)
    binary=root/'target/debug/shutdown-fixture'
    errors=[]
    for stage,sig,want in [('upward',signal.SIGTERM,0),('status',signal.SIGTERM,0),
                           ('ble',signal.SIGTERM,0),('upward',signal.SIGINT,0),
                           ('clean',signal.SIGTERM,0),('crash',None,1)]:
        proc=subprocess.Popen([str(binary)],env=dict(env,STALL=stage),stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        assert proc.stdout.readline().strip()=='READY'
        started=time.monotonic()
        if sig: proc.send_signal(sig)
        try: out,err=proc.communicate(timeout=8)
        except subprocess.TimeoutExpired:
            proc.kill(); proc.communicate(); raise
        elapsed=time.monotonic()-started
        print(f'{stage} signal={sig} exit={proc.returncode} expected={want} elapsed={elapsed:.3f}s\n{err}',flush=True)
        if proc.returncode!=want: errors.append(stage)
        if stage in ('upward','status','ble'): assert 4.8 <= elapsed < 7
    if args.systemd:
        unit=f'codex0824-shutdown-{os.getpid()}.service'
        try:
            subprocess.run(['systemd-run','--user','--unit='+unit,'--property=Restart=always',
                            '--property=TimeoutStopSec=10','--setenv=STALL=upward',str(binary)],check=True)
            time.sleep(1)
            subprocess.run(['systemctl','--user','stop',unit],check=True,timeout=12)
            result=subprocess.check_output(['systemctl','--user','show',unit,
                '--property=ActiveState,Result,ExecMainStatus'],text=True)
            print('SYSTEMD '+result,flush=True)
            if not all(x in result.splitlines() for x in ['ActiveState=inactive','Result=success','ExecMainStatus=0']):
                errors.append('systemd')
        finally:
            # Successful transient units may already have been garbage-collected.
            subprocess.run(['systemctl','--user','stop',unit],check=False,capture_output=True)
            subprocess.run(['systemctl','--user','reset-failed',unit],check=False,capture_output=True)
    assert not errors, 'shutdown regressions: '+repr(errors)
