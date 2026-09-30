#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline rework suites + assertion-failing mutations; never writes live sources."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

root=Path(__file__).resolve().parents[4]
out=Path(os.environ.get('D44_EVIDENCE_DIR',str(Path(__file__).resolve().parent/'evidence')));out.mkdir(parents=True,exist_ok=True)
release=root/'Component/aviary/release-system'
def run(name,path,args,expected=0):
 p=subprocess.run(args,cwd=path,env=dict(os.environ,D44_EVIDENCE=str(out),GOPROXY='off',PYTHONDONTWRITEBYTECODE='1'),text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 (out/(name+'.log')).write_text(p.stdout+f'\nexit_code={p.returncode}\n')
 print(name,p.returncode,flush=True)
 assert p.returncode==expected,(name,p.stdout[-2500:])
 if expected:assert '--- FAIL:' in p.stdout and 'build failed' not in p.stdout
for name,path in [('release',release)]:run(name,path,['go','test','-race','-count=1','-v','./...'])
mutants=[
 ('wire-field-order',release,'manifest.go','m.ConsoleVersion, m.ConsoleBuild','m.ConsoleBuild, m.ConsoleVersion','TestReleaseCanonicalWireBytes'),
 ('mailbox-private',release,'storage.go','filepath.Join(u.cfg.PrivateDir, "pending_update.json")','filepath.Join(u.cfg.StateDir, "pending_update.json")','TestPendingIsPrivateAndBadSlotsRecover'),
 ('mailbox-cleanup',release,'updater.go','staged, err := u.preparePending()','staged, err := false, error(nil)','TestPendingIsPrivateAndBadSlotsRecover'),
 ('local-before-poll',release,'updater.go','return u.apply()','return u.poll()','TestStagedAppliesWithoutManifestNetwork'),
 ('clock',release,'updater.go','if e != nil || s.Heartbeat.Trusted == nil || !*s.Heartbeat.Trusted {','if false && (e != nil || s.Heartbeat.Trusted == nil || !*s.Heartbeat.Trusted) {','TestClockTrustPrecedesExpiryDecisions'),
 ('boot-quarantine',release,'package.go','tx.Phase = "recovery_wait"','u.state.Quarantined[target] = true; tx.Phase = "recovery_wait"','TestBootReadinessNeverQuarantinesGoodPair'),
 ('boot-readiness',release,'package.go','return u.recoverWithReadiness(true)','return u.recoverWithReadiness(false)','TestBootReadinessNeverQuarantinesGoodPair'),
 ('signer-echo',release,'publisher.go','if !bytes.Equal(signed.canonical(), m.canonical()) {','if false {','TestRemotePublisherRefusesUnsafeReplies'),
]
receipts=[]
for name,path,file,before,after,test in mutants:
 with tempfile.TemporaryDirectory(prefix='d44-rework-mutant-') as tmp:
  work=Path(tmp)/'source';shutil.copytree(path,work)
  p=work/file;s=p.read_text();assert before in s,(name,before);p.write_text(s.replace(before,after))
  run('mutation-'+name,work,['go','test','-count=1','-v','-run','^'+test+'$'],1)
  receipts.append(dict(control=name,test=test,assertion_failure=True))
for name,path in [('release-restored',release)]:run(name,path,['go','test','-race','-count=1','-v','./...'])
(out/'mutations.json').write_text(json.dumps(receipts,indent=2)+'\n')
