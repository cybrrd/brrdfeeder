#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline mutation and helper-5 strict-reader proofs. No host writes or network."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

release=Path(__file__).resolve().parents[1]
out=Path(os.environ['POLICY_EVIDENCE_DIR']);out.mkdir(parents=True,exist_ok=True)
goroot=subprocess.check_output(['go','env','GOROOT'],cwd=release,text=True).strip()
env=dict(os.environ,PATH=goroot+'/bin:'+os.environ['PATH'],GOTOOLCHAIN='local',GOPROXY='off',GOSUMDB='off',PYTHONDONTWRITEBYTECODE='1')
def run(name,cwd,args,expected=0,extra=None):
 p=subprocess.run(args,cwd=cwd,env=dict(env,**(extra or {})),text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 (out/(name+'.log')).write_text(p.stdout+'\nexit_code='+str(p.returncode)+'\n')
 print(name,p.returncode,flush=True)
 assert p.returncode==expected,(name,p.stdout[-3000:])
 if expected: assert '--- FAIL:' in p.stdout and 'build failed' not in p.stdout

run('policy-clean',release,['go','test','-count=1','-run','TestPolicy','./...'])
mutants=[
 ('transform','engine_policy.go','lines[intervalLine] = "StartLimitIntervalSec=0"','lines[intervalLine] = "StartLimitIntervalSec=300"','TestPolicyTransformNarrow'),
 ('same-image-noop','updater.go','if policy == nil && contains(im.RepoDigests','if (policy == nil || policy != nil) && contains(im.RepoDigests','TestPolicySignedFlowMatrix/same-poll'),
 ('policy-restart','package.go','if ec || policy != nil {','if ec {','TestPolicySignedFlowMatrix/same-staged'),
 ('capability','engine_policy.go','if err != nil || answer != policyCapability {','if err != nil || (false && answer != policyCapability) {','TestPolicyReadinessRefusesBeforeStaging'),
 ('late-input','engine_policy_transaction.go','if err != nil || meta != p.Before || !bytes.Equal(raw, tx.Quadlet) {','if err != nil || (false && (meta != p.Before || !bytes.Equal(raw, tx.Quadlet))) {','TestPolicyLatePreflightChanges'),
 ('new-invocation','package.go','if err != nil || sample.Invocation == policy.Engine.Invocation {','if err != nil || (false && sample.Invocation == policy.Engine.Invocation) {','TestPolicyFailureBudgetAndExactRollback/invocation'),
 ('budget','engine_policy_transaction.go','if b.Used {','if false && b.Used {','TestPolicyReadinessRefusesBeforeStaging/budget'),
 ('rollback-reload','engine_policy_transaction.go','if err = u.policyReload(p.Effective); err != nil {','if err = error(nil); err != nil {','TestPolicyRecoveryIsBoundedAndVerifiesEffectiveState'),
 ('recovery-bound','engine_policy_transaction.go','if p.RecoveryStarts >= 2 || u.now().Before(p.NextRecovery) {','if false && (p.RecoveryStarts >= 2 || u.now().Before(p.NextRecovery)) {','TestPolicyRecoveryIsBoundedAndVerifiesEffectiveState'),
 ('journal-version','engine_policy.go','p.Schema != 1 || p.Rule != policyRule','p.Schema != p.Schema || p.Rule != policyRule','TestPolicyConflictAndCorruptJournal/schema'),
]
receipts=[]
for name,file,before,after,test in mutants:
 with tempfile.TemporaryDirectory(prefix='policy-mutant-') as tmp:
  work=Path(tmp)/'module';shutil.copytree(release,work,ignore=shutil.ignore_patterns('__pycache__','evidence'))
  p=work/file;s=p.read_text();assert s.count(before)==1,(name,s.count(before));p.write_text(s.replace(before,after))
  run('mutation-'+name,work,['go','test','-count=1','-run','^'+test+'$'],1)
  receipts.append(dict(control=name,test=test,assertion_failure=True))
(out/'mutations.json').write_text(json.dumps(receipts,indent=2)+'\n')
fixtures=out/'strict-fixtures';fixtures.mkdir(exist_ok=True)
run('strict-export',release,['go','test','-count=1','-run','^TestPolicyExportStrictReaderFixtures$'],extra={'POLICY_STRICT_FIXTURES':str(fixtures.resolve())})
# Fixed reviewed stack base, never a mutable remote branch or a relaxed decoder.
base='15b835400eb5732f21ca663a244ad845bae7fda1'
with tempfile.TemporaryDirectory(prefix='policy-old-reader-') as tmp:
 work=Path(tmp);prefix='Component/aviary/release-system/'
 files=subprocess.check_output(['git','ls-tree','-r','--name-only',base,'--',prefix],cwd=release.parents[2],text=True).splitlines()
 for path in files:
  p=work/path.removeprefix(prefix);p.parent.mkdir(parents=True,exist_ok=True)
  p.write_bytes(subprocess.check_output(['git','show',base+':'+path],cwd=release.parents[2]))
 (work/'policy_reader_test.go').write_text('''package main
import("os";"path/filepath";"testing";"strings")
func TestPolicyOldReader(t *testing.T){
 for _,name:=range []string{"untouched.json","active.json","committed.json"}{
  u,_,_:=setup(t);raw,err:=os.ReadFile(filepath.Join(os.Getenv("POLICY_STRICT_FIXTURES"),name));if err!=nil{t.Fatal(err)}
  if err=os.WriteFile(u.statePath(),raw,0600);err!=nil{t.Fatal(err)}
  err=u.load();if name=="active.json" {if err==nil||!strings.Contains(err.Error(),"unknown field"){t.Fatal("old reader accepted new active journal",err)}}else if err!=nil {t.Fatal(name,err)}
 }
}
''')
 run('helper5-strict-reader',work,['go','test','-count=1','-v','-run','^TestPolicyOldReader$'],extra={'POLICY_STRICT_FIXTURES':str(fixtures.resolve())})
run('policy-restored',release,['go','test','-count=1','-run','TestPolicy','./...'])
