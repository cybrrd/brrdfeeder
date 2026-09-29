#!/usr/bin/env python3
"""Offline Rust/Go receipt handoff; mutants use disposable source trees."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

root=Path(__file__).resolve().parents[4]
out=Path(os.environ.get('D44_EVIDENCE_DIR',str(Path(__file__).resolve().parent/'evidence')))
out.mkdir(parents=True,exist_ok=True)
aviary=root/'Component/aviary'
env=dict(os.environ,D44_EVIDENCE=str(out),D44_OUTCOME=str(out/'quarantine.json'),
         CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR',str(aviary/'target')))

def run(name,args,cwd,expected=0):
    if args[0] == 'cargo':
        # include_str! files differ across disposable worktrees. Force this
        # package to compile; keep downloaded dependencies and their builds.
        subprocess.run(['cargo','clean','-p','engine'],cwd=cwd,env=env,check=True,
                       stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
    p=subprocess.run(args,cwd=cwd,env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
    (out/(name+'.log')).write_text('$ '+' '.join(args)+'\n'+p.stdout+f'\nexit_code={p.returncode}\n')
    print(name,p.returncode,flush=True)
    if p.returncode!=expected:raise RuntimeError(name+' unexpected result')
    if expected and ('test result: FAILED' not in p.stdout or 'could not compile' in p.stdout):raise RuntimeError(name+' did not fail a test assertion')

run('go-handoff',['go','test','-count=1','-v','./...'],aviary/'release-system')
assert (out/'quarantine.json').exists()
run('rust-baseline',['cargo','test','--offline','--locked','-p','engine','--','--nocapture'],aviary)
mutations=[
 ('red-emission','engine/src/upward.rs','"quarantined" => Some(Distress::UpdateQuarantined)','"quarantined" => None','d44_quarantine_enters_existing_red_lane_and_durable_spool'),
 ('early-reconcile','engine/src/blue_policy.rs','if state_path.parent()?.join("update_transaction.json").exists() {','if false && state_path.parent()?.join("update_transaction.json").exists() {','d44_host_health_gate_owns_reconcile_and_mailbox'),
]
receipts=[]
with tempfile.TemporaryDirectory(prefix='d44-rust-mutant-') as tmp:
    work=Path(tmp)/'Component/aviary'
    shutil.copytree(aviary,work,ignore=shutil.ignore_patterns('target','__pycache__'))
    shutil.copytree(root/'Component/brrdfeeder',work.parent/'brrdfeeder')
    for name,file,before,after,test in mutations:
        path=work/file; original=path.read_text()
        assert original.count(before)==1,name
        try:
            path.write_text(original.replace(before,after))
            run('mutation-'+name,['cargo','test','--offline','--locked','-p','engine',test,'--','--nocapture'],work,101)
        finally:path.write_text(original)
        receipts.append(dict(control=name,expected_failure=101,source_sha256=hashlib.sha256(original.encode()).hexdigest()))
run('rust-restored',['cargo','test','--offline','--locked','-p','engine','--','--nocapture'],aviary)
(out/'rust-mutations.json').write_text(json.dumps(receipts,indent=2)+'\n')
