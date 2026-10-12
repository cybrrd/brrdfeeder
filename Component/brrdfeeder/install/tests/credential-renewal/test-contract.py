#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Host dispatch with stubbed effectors. Never calls Podman, OAuth or flock."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

INSTALL=Path(__file__).resolve().parents[2]/'brrdfeeder-install.sh'
ROOT=INSTALL.parents[3]
TEXT=INSTALL.read_text()
BLOCK=TEXT[TEXT.index('if [[ ${1:-} == renew ]]'):TEXT.index('case "${1:-}" in')]

def dispatch(uid=0,sync='yes',clock_exit=0,exchange=0,signal=0,block=BLOCK):
 with tempfile.TemporaryDirectory(prefix='owner-renew-test-') as td:
  p=Path(td);calls=p/'calls'
  # Bash EUID is readonly; substitute only the source of the test UID.
  script=block.replace('$EUID','${FIXTURE_UID}')+'\nexit 99\n'
  (p/'command.sh').write_text(script)
  for name,body in {
   'hostname':'echo lab\n',
   'timedatectl':'echo "$FIXTURE_SYNC"; exit "$FIXTURE_CLOCK_EXIT"\n',
   'podman':'''python3 -c 'import json,os,sys; f=open(os.environ["FIXTURE_CALLS"],"a");f.write(json.dumps(sys.argv[1:])+"\\n");f.close();sys.exit(int(os.environ["FIXTURE_EXCHANGE"] if sys.argv[1]=="exec" else os.environ["FIXTURE_SIGNAL"]))' "$@"\n''',
  }.items():
   q=p/name;q.write_text('#!/usr/bin/env bash\n'+body);q.chmod(0o755)
  env=dict(os.environ,PATH=str(p)+':'+os.environ['PATH'],FIXTURE_UID=str(uid),FIXTURE_SYNC=sync,FIXTURE_CLOCK_EXIT=str(clock_exit),FIXTURE_EXCHANGE=str(exchange),FIXTURE_SIGNAL=str(signal),FIXTURE_CALLS=str(calls))
  r=subprocess.run(['bash',str(p/'command.sh'),'renew'],env=env,capture_output=True,text=True)
  return r,[json.loads(x) for x in calls.read_text().splitlines()] if calls.exists() else []

class Renewal(unittest.TestCase):
 def test_root_and_clock_before_authority(self):
  for options in [dict(uid=1000),dict(sync='no')]:
   result,calls=dispatch(**options);self.assertNotEqual(result.returncode,0);self.assertEqual(calls,[])
 def test_owner_clock_unknown_or_error_fails_closed(self):
  for options in [dict(sync=''),dict(sync='unknown'),dict(sync='yes',clock_exit=1),dict(sync='',clock_exit=127)]:
   with self.subTest(options=options):
    result,calls=dispatch(**options)
    self.assertNotEqual(result.returncode,0);self.assertEqual(calls,[])
    self.assertIn('Clock is not synchronized',result.stderr)
 def test_owner_clock_guard_mutation_is_detected(self):
  guard='RENEW_CLOCK_SYNC=$(timedatectl show -p NTPSynchronized --value) && [[ $RENEW_CLOCK_SYNC == yes ]]'
  self.assertIn(guard,BLOCK)
  mutated=BLOCK.replace(guard,'true')
  result,calls=dispatch(sync='no',block=mutated)
  # The unchanged fail-closed assertion MUST fail with the guard removed.
  with self.assertRaises(AssertionError):self.assertEqual(calls,[])
  self.assertEqual(result.returncode,0)
 def test_owner_helper_clock_contract_is_explicit(self):
  engine=(ROOT/'Component/aviary/engine/src/credential_renewal.rs').read_text()
  doc=(ROOT/'docs/node-renewal/README.md').read_text()
  self.assertIn('OWNER_CLOCK_HOST_GATE',engine)
  self.assertIn('not a supported standalone recovery interface',doc)
  self.assertIn('host-command update',doc)
 def test_image_device_flow_then_explicit_signal(self):
  result,calls=dispatch();self.assertEqual(result.returncode,0,result.stderr)
  self.assertEqual(calls,[['exec','-i','brrdfeeder-engine','/usr/local/libexec/brrdfeeder-policy','renew-owner','lab'],['kill','--signal','USR1','brrdfeeder-engine']])
  self.assertIn('reconnect requested',result.stdout)
 def test_failures_do_not_claim_reconnect(self):
  r,calls=dispatch(exchange=4);self.assertEqual(r.returncode,4);self.assertEqual(len(calls),1)
  r,calls=dispatch(signal=5);self.assertNotEqual(r.returncode,0);self.assertNotIn('reconnect requested',r.stdout)
 def test_no_reinstall_or_identity_rewrite(self):
  for prohibited in ['uninstall','rm ','config.yaml','curl ','Authorization:','systemctl restart']:
   self.assertNotIn(prohibited,BLOCK)
  engine=(ROOT/'Component/aviary/engine/src/credential_renewal.rs').read_text()
  self.assertIn('"renew_node_id":node',engine.replace(' ',''))
  self.assertIn('device_authorization',engine);self.assertIn('urn:ietf:params:oauth:grant-type:device_code',engine)
 def test_ro_bootstrap_and_narrow_rw_mount(self):
  self.assertIn('Volume=${SECRETS_DIR}:/etc/brrdfeeder/secrets:ro',TEXT)
  self.assertIn('Volume=${STATE_DIR}/credentials:/var/lib/brrdfeeder/credentials:rw',TEXT)
  self.assertIn('install -d -m 0700 -o "$TARGET_UID" -g "$TARGET_GID" "$STATE_DIR/credentials"',TEXT)
  self.assertIn('if [[ ! -e "$STATE_DIR/credentials/brrdfeeder.creds" ]]',TEXT)
 def test_root_guard_mutation(self):
  mutated=BLOCK.replace('$EUID -eq 0','1 -eq 1')
  _,calls=dispatch(uid=1000,block=mutated)
  self.assertTrue(calls,'root guard mutant was not exercised')
 def test_real_private_directory_preserves_rotated_file(self):
  setup=TEXT.split('# --- dedicated credential state (renewal v1) ---\n')[1].split('# --- end dedicated credential state ---')[0]
  with tempfile.TemporaryDirectory(prefix='renewal-install-state-') as td:
   root=Path(td);state=root/'state';state.mkdir();bootstrap=root/'bootstrap.creds';bootstrap.write_text('synthetic bootstrap')
   env=dict(os.environ,STATE_DIR=str(state),CREDS_PATH=str(bootstrap),TARGET_UID=str(os.getuid()),TARGET_GID=str(os.getgid()))
   def provision():return subprocess.run(['bash','-euc','run() { "$@"; }; fatal() { echo "$*" >&2; exit 1; };\n'+setup],env=env,capture_output=True,text=True)
   r=provision();self.assertEqual(r.returncode,0,r.stderr);directory=state/'credentials';active=directory/'brrdfeeder.creds'
   self.assertEqual(directory.stat().st_mode&0o777,0o700);self.assertEqual(active.stat().st_mode&0o777,0o600);self.assertEqual(active.stat().st_uid,os.getuid())
   active.write_text('rotated');bootstrap.write_text('new bootstrap');self.assertEqual(provision().returncode,0);self.assertEqual(active.read_text(),'rotated')
   active.unlink();active.symlink_to(bootstrap);self.assertNotEqual(provision().returncode,0);self.assertEqual(bootstrap.read_text(),'new bootstrap')

if __name__=='__main__':unittest.main()
