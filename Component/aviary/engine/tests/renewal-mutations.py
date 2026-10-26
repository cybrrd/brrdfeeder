#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Mutate production guards in a private workspace copy; require assertion failures."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT=Path(__file__).resolve().parents[2]
OUT=Path(sys.argv[1]).resolve() if len(sys.argv)>1 else Path(tempfile.mkdtemp(prefix='renewal-engine-receipts-'))
OUT.mkdir(parents=True,exist_ok=True)
NEGATIVE='credential_renewal::tests::rejected_jwts_leave_file_and_live_connection_unchanged'
cases=[
 ('issuer','issuer','if trust.is_some_and(','if false && trust.is_some_and('),
 ('account','account','if trust.is_some_and(','if false && trust.is_some_and('),
 ('signature','signature','.map_err(|_| "invalid_signature")?;','.ok();'),
 ('subject','subject','if c.sub != key.public_key()','if false && c.sub != key.public_key()'),
 ('metadata','metadata','|| res.node_id != node','|| false'),
 ('missing_expiry','missing_expiry','    exp: i64,','    #[serde(default)]\n    exp: i64,'),
 ('nonadvancing','nonadvancing','if c.exp <= old.exp','if false && c.exp <= old.exp'),
 ('clock','clock','if !trusted {','if false && !trusted {'),
 ('private_directory','', 'm.mode() & 0o077 != 0','false'),
 ('private_file_mode','', '.mode(0o600)', '.mode(0o644)'),
 ('rotation_signal','metadata','self.rotation.send_modify(|n| *n=n.saturating_add(1));','let _ = &self.rotation;'),
 ('rotation_queue','', '_ = rotation.changed() => continue,','_ = rotation.changed() => { buffer.clear(); continue; },'),
]
selected_tests={
 'private_directory':'credential_renewal::tests::canonical_signature_and_storage_failclosed',
 'private_file_mode':'credential_renewal::tests::verified_atomic_swap_and_permissions',
 'rotation_queue':'credential_renewal::tests::rotation_rereads_file_and_retains_offline_queue_in_order',
}
env=dict(os.environ)
# Sharing dependency artifacts is safe; never point the copied source at a live node.
env['CARGO_TARGET_DIR']=str(ROOT/'target')
rows=[]
with tempfile.TemporaryDirectory(prefix='engine-renewal-mutations-') as td:
 work=Path(td)/'aviary';shutil.copytree(ROOT,work,ignore=shutil.ignore_patterns('target','.git','evidence'))
 installer=Path(td)/'brrdfeeder/install';installer.mkdir(parents=True)
 shutil.copy2(ROOT.parent/'brrdfeeder/install/brrdfeeder-install.sh',installer/'brrdfeeder-install.sh')
 source=work/'engine/src/credential_renewal.rs';original=source.read_text()
 command=['cargo','+1.88.0','test','--offline','--locked','-p','engine','--bin','engine',NEGATIVE,'--','--exact','--nocapture']
 r=subprocess.run(command,cwd=work,env=env,capture_output=True,text=True,timeout=300)
 (OUT/'baseline.log').write_text(r.stdout+r.stderr)
 if r.returncode:sys.exit('baseline failed')
 for mode in ['expired','lifetime','stale_iat','future_iat','nbf']:
  # Remove this whole conjunction of time checks, retain all identity/signature checks.
  start=original.index('    if c.exp <= old.exp');end=original.index(' // RENEW MUTATE expiry',start)
  block=original[start:end];cases.append((mode,mode,block,'    if false '+block[block.index('{'):]))
 for name,mode,old,new in cases:
  target=work/'engine/src/nats_publisher.rs' if name.startswith('rotation_') else source
  baseline=target.read_text()
  if old not in baseline:sys.exit('mutation anchor drift: '+name)
  target.write_text(baseline.replace(old,new))
  test=selected_tests.get(name,NEGATIVE);args=[test if arg==NEGATIVE else arg for arg in command]
  try:r=subprocess.run(args,cwd=work,env=dict(env,RENEW_NEGATIVE=mode),capture_output=True,text=True,timeout=300)
  finally:target.write_text(baseline)
  output=r.stdout+r.stderr;(OUT/(name+'.log')).write_text(output)
  killed=r.returncode!=0 and "panicked at" in output and 'test result: FAILED' in output and 'could not compile' not in output
  rows.append(dict(guard=name,test=test,case=mode,killed=killed,exit=r.returncode))
  print(name+': '+('KILLED (assertion)' if killed else 'FAILED mutation evidence'),flush=True)
 (OUT/'summary.json').write_text(json.dumps(rows,indent=2)+'\n')
print('receipts: '+str(OUT));sys.exit(0 if all(r['killed'] for r in rows) else 1)
