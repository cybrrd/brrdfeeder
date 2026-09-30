#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Behavioural journald drop-in compatibility tests for the BRRDfeeder naming
packet (2026-09-25). ONLY inside a disposable, network-disabled, root OS
container (same discipline as d42/D12): real filesystem and passwd, synthetic
package/systemd commands via PATH stubs.

Cases (packet section B):
  uninstall: legacy-only, new-only, both, foreign-at-new, foreign-at-legacy
  install rerun (main installer, apply): legacy migrated to the new name
  install rerun (aviary kit, apply): same migration rule
Foreign files must never be removed; markerless legacy files must be refused.
"""
import os
import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest

assert os.environ.get('BRRD_NAMING_CONTAINER')=='1', 'disposable container only'
assert os.geteuid()==0 and Path('/run/.containerenv').exists(), 'not a container'
ROOT=Path(os.environ.get('NAMING_ROOT','/repo'))
OUT=Path(os.environ.get('NAMING_EVIDENCE','/tmp/naming-evidence')); OUT.mkdir(parents=True,exist_ok=True)
INSTALL=ROOT/'Component/brrdfeeder/install'
UNINSTALL=INSTALL/'uninstall.sh'
INSTALLER=INSTALL/'brrdfeeder-install.sh'
KITDIR=ROOT/'Component/aviary/deploy/bootstrap'
JDIR=Path('/etc/systemd/journald.conf.d')
LEGACY=JDIR/'99-brrdfeeder-open.conf'
NEW=JDIR/'99-brrdfeeder.conf'
LEGACY_CONTENT='''# BRRDfeeder Open tier — protect MicroSD card from journald write wear.
# Installed by brrdfeeder-install.sh when node.storage_class is "ephemeral".
# Forces journald to RAM (/run/log/journal). System logs are NOT persistent.
[Journal]
Storage=volatile
RuntimeMaxUse=200M
SystemMaxUse=0
'''
NEW_CONTENT='''# BRRDfeeder — protect MicroSD card from journald write wear.
# Installed by brrdfeeder-install.sh when node.storage_class is "ephemeral".
# Forces journald to RAM (/run/log/journal). System logs are NOT persistent.
[Journal]
Storage=volatile
RuntimeMaxUse=200M
SystemMaxUse=0
'''
FOREIGN_CONTENT='''# drop-in maintained by the site administrator
[Journal]
Storage=persistent
'''
SYSTEMCTL_STUB='''#!/bin/sh
if [ "$1 $2" = 'restart systemd-journald' ]; then
  if [ -f /etc/systemd/journald.conf.d/99-brrdfeeder-open.conf ]; then state=legacy; else state=migrated; fi
  echo "$state" >> /tmp/naming-journald-restarts
fi
case "$1" in
  is-active|is-enabled) exit 1 ;;
  *) exit 0 ;;
esac
'''

def run(args,env=None,stdin=None):
    result=subprocess.run(args,text=True,capture_output=True,env=env,input=stdin,timeout=180)
    return result

class JournaldFixture(unittest.TestCase):
    def setUp(self):
        Path('/tmp/naming-journald-restarts').unlink(missing_ok=True)
        # Remove only the inert helper created by the prior naming case; its
        # receipt lives in /etc/brrdfeeder, also reset below. No host execution.
        Path('/usr/local/libexec/brrdfeeder-release').unlink(missing_ok=True)
        if JDIR.exists():
            for entry in JDIR.iterdir(): entry.unlink()
        else: JDIR.mkdir(parents=True)
        # Class isolation: the installer cases leave accounts/trees behind.
        for user in ('brrdhouse','brrdfeeder','operator'):
            subprocess.run(['userdel','--force',user],capture_output=True)
            subprocess.run(['groupdel',user],capture_output=True)
        subprocess.run(['rm','-rf','/etc/brrdfeeder','/var/lib/brrdfeeder','/var/lib/brrdhouse',
                        '/var/lib/brrdfeeder-status','/run/brrdfeeder-identity','/usr/local/sbin/brrdfeeder'],
                       capture_output=True)
    def seed(self,path,content):
        path.write_text(content); path.chmod(0o644)
    def record(self,name,result):
        (OUT/name).write_text(f'argv: {result.args}\nrc: {result.returncode}\n--- stdout ---\n{result.stdout}\n--- stderr ---\n{result.stderr}\n')
        for marker in ('BRRDfeeder ' + 'Open tier', 'P' + 'ack-canonical', '#185 ' + 'Drop 2'):
            self.assertNotIn(marker, result.stdout + result.stderr)

class UninstallPlan(JournaldFixture):
    def plan(self):
        return run(['bash',str(UNINSTALL),'1','0','','plan','1'])
    def test_legacy_only_node_plans_legacy_removal(self):
        self.seed(LEGACY,LEGACY_CONTENT)
        result=self.plan(); self.record('uninstall-plan-legacy-only.log',result)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertIn(f'WOULD: rm -- {LEGACY}',result.stdout)
        self.assertIn(f'nothing to do: {NEW} absent',result.stdout)
    def test_new_only_node_plans_new_removal(self):
        self.seed(NEW,NEW_CONTENT)
        result=self.plan(); self.record('uninstall-plan-new-only.log',result)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertIn(f'WOULD: rm -- {NEW}',result.stdout)
        self.assertIn(f'nothing to do: {LEGACY} absent',result.stdout)
    def test_both_present_plans_both(self):
        self.seed(LEGACY,LEGACY_CONTENT); self.seed(NEW,NEW_CONTENT)
        result=self.plan(); self.record('uninstall-plan-both.log',result)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertIn(f'WOULD: rm -- {LEGACY}',result.stdout)
        self.assertIn(f'WOULD: rm -- {NEW}',result.stdout)
    def test_foreign_file_at_new_name_is_refused_and_survives(self):
        self.seed(NEW,FOREIGN_CONTENT)
        result=self.plan(); self.record('uninstall-plan-foreign-new.log',result)
        self.assertNotEqual(result.returncode,0,'uninstall touched a foreign drop-in')
        self.assertIn(f'REFUSED: unrecognised package file: {NEW}',result.stderr+result.stdout)
        self.assertEqual(NEW.read_text(),FOREIGN_CONTENT)
    def test_foreign_file_at_legacy_name_is_refused_and_survives(self):
        self.seed(LEGACY,FOREIGN_CONTENT)
        result=self.plan(); self.record('uninstall-plan-foreign-legacy.log',result)
        self.assertNotEqual(result.returncode,0,'uninstall touched a foreign drop-in')
        self.assertIn(f'REFUSED: unrecognised package file: {LEGACY}',result.stderr+result.stdout)
        self.assertEqual(LEGACY.read_text(),FOREIGN_CONTENT)

class UninstallApply(JournaldFixture):
    def apply(self):
        stubs=tempfile.mkdtemp(prefix='naming-stub-')
        Path(stubs,'systemctl').write_text(SYSTEMCTL_STUB); Path(stubs,'systemctl').chmod(0o755)
        Path(stubs,'udevadm').write_text('#!/bin/sh\nexit 0\n'); Path(stubs,'udevadm').chmod(0o755)
        env=dict(os.environ,PATH=stubs+':'+os.environ['PATH'])
        return run(['bash',str(UNINSTALL),'0','0','','confirm','1'],env=env)
    def test_apply_removes_both_dropins(self):
        self.seed(LEGACY,LEGACY_CONTENT); self.seed(NEW,NEW_CONTENT)
        result=self.apply(); self.record('uninstall-apply-both.log',result)
        self.assertEqual(result.returncode,0,result.stdout[-2000:]+result.stderr[-2000:])
        self.assertFalse(LEGACY.exists(),'legacy drop-in survived apply')
        self.assertFalse(NEW.exists(),'new drop-in survived apply')
    def test_apply_removes_legacy_only(self):
        self.seed(LEGACY,LEGACY_CONTENT)
        result=self.apply(); self.record('uninstall-apply-legacy.log',result)
        self.assertEqual(result.returncode,0,result.stdout[-2000:]+result.stderr[-2000:])
        self.assertFalse(LEGACY.exists())
    def test_apply_removes_new_only(self):
        self.seed(NEW,NEW_CONTENT)
        result=self.apply(); self.record('uninstall-apply-new.log',result)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertFalse(NEW.exists())
    def test_apply_refuses_foreign_legacy_and_keeps_it(self):
        self.seed(LEGACY,FOREIGN_CONTENT)
        result=self.apply(); self.record('uninstall-apply-foreign-legacy.log',result)
        self.assertNotEqual(result.returncode,0)
        self.assertIn('unrecognised package file',result.stdout+result.stderr)
        self.assertEqual(LEGACY.read_text(),FOREIGN_CONTENT)
    def test_apply_refuses_foreign_and_keeps_it(self):
        self.seed(NEW,FOREIGN_CONTENT)
        result=self.apply(); self.record('uninstall-apply-foreign.log',result)
        self.assertNotEqual(result.returncode,0,'apply removed a foreign drop-in')
        self.assertIn('unrecognised package file',result.stdout+result.stderr)
        self.assertEqual(NEW.read_text(),FOREIGN_CONTENT)

PIN='ghcr.io/cybrrd/brrdfeeder@sha256:'+'a'*64

class InstallerRerun(JournaldFixture):
    """Full installer, apply mode, with the d42 stub recipe."""
    def setUp(self):
        super().setUp()
        subprocess.run(['userdel','--force','brrdhouse'],capture_output=True)
        subprocess.run(['userdel','--force','brrdfeeder'],capture_output=True)
        subprocess.run(['groupdel','brrdhouse'],capture_output=True)
        subprocess.run(['groupdel','brrdfeeder'],capture_output=True)
        subprocess.run(['rm','-rf','/etc/brrdfeeder','/var/lib/brrdfeeder','/var/lib/brrdhouse',
                        '/var/lib/brrdfeeder-status','/run/brrdfeeder-identity','/etc/containers/systemd',
                        '/var/lib/systemd/linger/brrdfeeder','/var/lib/systemd/linger/brrdhouse',
                        '/etc/udev/rules.d/99-cybrrd-brrdfeeder.rules','/usr/local/sbin/brrdfeeder',
                        '/usr/local/libexec/brrdfeeder-image-identity','/usr/local/libexec/brrdfeeder-provision-status',
                        '/usr/local/libexec/brrdfeeder-gps-seed','/usr/local/libexec/brrdfeeder-bluetooth',
                        '/etc/systemd/system/brrdfeeder-updater.path','/etc/systemd/system/brrdfeeder-updater.service',
                        '/etc/chrony/conf.d/10-pack.conf'],capture_output=True)
        Path('/etc/containers').mkdir(exist_ok=True); Path('/etc/systemd/system').mkdir(parents=True,exist_ok=True)
        Path('/etc/udev/rules.d').mkdir(parents=True,exist_ok=True); Path('/etc/chrony/conf.d').mkdir(parents=True,exist_ok=True)
        subprocess.run(['useradd','--system','--user-group','--no-create-home',
                        '--home-dir','/var/lib/brrdfeeder','--shell','/usr/sbin/nologin','brrdfeeder'],check=True)
        self.tmp=tempfile.mkdtemp(prefix='naming-')
        # Stub directory must be traversable by the unprivileged console user
        # (runuser execs these); mkdtemp's 0700 would give rc=126.
        import stat as stat_module
        os.chmod(self.tmp,0o755)
        self.bin=Path(self.tmp)
        stub='''#!/usr/bin/python3
import sys
from pathlib import Path
name=Path(sys.argv[0]).name; args=sys.argv[1:]
if name=='curl' and any(a.startswith('http://127.0.0.1:8080') for a in args): sys.exit(0)
if name in ['curl','jq','apt-get']: sys.exit(99)
if name=='podman':
    if args[:2]==['image','exists']: sys.exit(0)
    if args[:1]==['pull']: sys.exit(0)
    if args[:2]==['image','inspect']:
        print('123' if any('build_seq' in a for a in args) else args[2]); sys.exit(0)
    if args[:1]==['ps']: print('brrdfeeder-engine\\nbrrdhouse'); sys.exit(0)
    sys.exit(0)
if name=='lsusb': sys.exit(1)
if name=='chronyc': print('Reference ID : ABCDEF00'); sys.exit(0)
if name=='journalctl': print('backhaul connected\\nGPS reached Healthy\\nCapture loop active\\n[rid_ble] state=Healthy detail=fixture'); sys.exit(0)
sys.exit(0)
'''
        for command in ['curl','jq','apt-get','podman','systemctl','udevadm','lsusb','chronyd','chronyc','journalctl','sleep','ip','sudo','loginctl']:
            path=self.bin/command; path.write_text(stub); path.chmod(0o755)
        systemctl=self.bin/'systemctl'  # D12 semantics: units inactive on a fresh node
        systemctl.write_text(SYSTEMCTL_STUB.replace('is-active|is-enabled) exit 1',
            'is-active) exit 0 ;;\n  is-enabled) exit 1').replace('#!/bin/sh\n',
            '#!/bin/sh\nif [ "$1" = show ]; then exec python3 /repo/Component/brrdfeeder/install/tests/brrdfeeder-naming/naming-readiness.py; fi\n'))
        systemctl.chmod(0o755)
        self.env=dict(os.environ,PATH=str(self.bin)+':'+os.environ['PATH'],SUDO_USER='root')
    def seed_installed_node(self):
        """Config with ephemeral storage class + inert creds: an already-enrolled
        legacy node being re-run by the operator."""
        CFG=Path('/etc/brrdfeeder/config.yaml')
        CFG.parent.mkdir(parents=True,exist_ok=True)
        text=INSTALLER.read_text().split("<<'CFGEOF'\n",1)[1].split('\nCFGEOF',1)[0]+'\n'
        CFG.write_text(text.replace('EDIT-ME-wlanX','wlan-test').replace('EDIT-ME-latitude','41.5').replace('EDIT-ME-longitude','-87.5'))
        creds=Path('/etc/brrdfeeder/secrets/brrdfeeder.creds')
        creds.parent.mkdir(parents=True,exist_ok=True)
        creds.write_text('NAMING INERT FIXTURE - NOT A CREDENTIAL\n'); creds.chmod(0o600)
        # Self-Update rerun preserves a previously installed host helper after
        # verifying its ownership receipt. Use an INERT helper in this naming
        # fixture; crypto/install behavior has its independent release suites.
        helper=Path('/usr/local/libexec/brrdfeeder-release')
        helper.parent.mkdir(parents=True,exist_ok=True)
        helper.write_text('#!/bin/sh\ncase "$1" in self-test|install) exit 0;; *) exit 99;; esac\n')
        helper.chmod(0o755)
        receipt=CFG.parent/'.updater-helper.sha256'
        receipt.write_text(hashlib.sha256(helper.read_bytes()).hexdigest()+'  '+str(helper)+'\n')
        receipt.chmod(0o600)
    def rerun(self):
        return run(['bash',str(INSTALLER),'--image',PIN,'--console-image',
                    'ghcr.io/cybrrd/brrdhouse@sha256:'+'c'*64,
                    '--console-listen','127.0.0.1:8080'],env=self.env)
    def test_rerun_migrates_legacy_dropin(self):
        self.seed_installed_node(); self.seed(LEGACY,LEGACY_CONTENT)
        result=self.rerun(); self.record('installer-rerun-migrate.log',result)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertFalse(LEGACY.exists(),'legacy drop-in not migrated away: \n'+result.stdout[-3000:])
        self.assertTrue(NEW.exists(),'new drop-in not written:\n'+result.stdout[-3000:])
        self.assertEqual(NEW.read_text(),NEW_CONTENT)
        self.assertEqual(Path('/tmp/naming-journald-restarts').read_text().splitlines(),['migrated'])
    def test_rerun_keeps_current_new_dropin(self):
        self.seed_installed_node(); self.seed(NEW,NEW_CONTENT)
        result=self.rerun(); self.record('installer-rerun-current.log',result)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertEqual(NEW.read_text(),NEW_CONTENT)
        self.assertFalse(LEGACY.exists())
    def test_rerun_migrates_both_and_restarts_after_removal(self):
        self.seed_installed_node(); self.seed(NEW,NEW_CONTENT); self.seed(LEGACY,LEGACY_CONTENT)
        result=self.rerun(); self.record('installer-rerun-both.log',result)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertFalse(LEGACY.exists())
        self.assertEqual(NEW.read_text(),NEW_CONTENT)
        self.assertEqual(Path('/tmp/naming-journald-restarts').read_text().splitlines(),['migrated'])
    def test_rerun_refuses_foreign_file_at_new_name(self):
        self.seed_installed_node(); self.seed(NEW,FOREIGN_CONTENT); self.seed(LEGACY,LEGACY_CONTENT)
        result=self.rerun(); self.record('installer-rerun-foreign-new.log',result)
        self.assertNotEqual(result.returncode,0)
        self.assertIn('unrecognised journald drop-in',result.stdout+result.stderr)
        self.assertEqual(NEW.read_text(),FOREIGN_CONTENT)
        self.assertEqual(LEGACY.read_text(),LEGACY_CONTENT)
    def test_rerun_refuses_foreign_file_at_legacy_name(self):
        self.seed_installed_node(); self.seed(LEGACY,FOREIGN_CONTENT)
        result=self.rerun(); self.record('installer-rerun-foreign-legacy.log',result)
        self.assertNotEqual(result.returncode,0,'installer removed or ignored a foreign legacy-name file')
        self.assertIn('unrecognised legacy journald drop-in',result.stdout+result.stderr)
        self.assertEqual(LEGACY.read_text(),FOREIGN_CONTENT)

class KitRerun(JournaldFixture):
    """Aviary bootstrap-kit installer, apply mode (D12-style PATH stubs)."""
    def setUp(self):
        super().setUp()
        subprocess.run(['userdel','--force','operator'],capture_output=True)
        subprocess.run(['useradd','-m','operator'],check=True)
        subprocess.run(['usermod','-a','-G','dialout','operator'],check=True)
        self.kit=tempfile.mkdtemp(prefix='kit-')
        subprocess.run(['cp',*[str(p) for p in KITDIR.glob('*.sh')],self.kit],check=True)
        Path('/home/operator/brrdfeeder-src/engine/target/release').mkdir(parents=True,exist_ok=True)
        subprocess.run(['install','-D','-m','0755','/bin/true','/home/operator/brrdfeeder-src/engine/target/release/engine'],check=True)
        template=(KITDIR/'config.yaml.mobile.template').read_text()
        Path('/home/operator/config.yaml').write_text(template)
        self.tmp=tempfile.mkdtemp(prefix='kit-stub-')
        for command in ['systemctl','udevadm','ip','mount','loginctl','sudo']:
            path=Path(self.tmp,command); path.write_text(SYSTEMCTL_STUB if command=='systemctl' else '#!/bin/sh\nexit 0\n'); path.chmod(0o755)
        self.env=dict(os.environ,PATH=self.tmp+':'+os.environ['PATH'],BRRDFEEDER_LEGACY_USER='operator')
    def kit_run(self):
        return run(['bash',str(Path(self.kit,'brrdfeeder-install.sh'))],env=self.env)
    def test_kit_rerun_migrates_legacy_dropin(self):
        self.seed(LEGACY,LEGACY_CONTENT)
        result=self.kit_run(); self.record('kit-rerun-migrate.log',result)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertFalse(LEGACY.exists(),'kit left the legacy drop-in in place:\n'+result.stdout[-3000:])
        self.assertTrue(NEW.exists(),'kit did not write the new drop-in:\n'+result.stdout[-3000:])
        self.assertEqual(NEW.read_text(),NEW_CONTENT)
        self.assertEqual(Path('/tmp/naming-journald-restarts').read_text().splitlines(),['migrated'])
    def test_kit_new_only(self):
        self.seed(NEW,NEW_CONTENT)
        result=self.kit_run(); self.record('kit-rerun-current.log',result)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertEqual(NEW.read_text(),NEW_CONTENT)
        self.assertFalse(LEGACY.exists())
    def test_kit_both_restart_after_removal(self):
        self.seed(NEW,NEW_CONTENT); self.seed(LEGACY,LEGACY_CONTENT)
        result=self.kit_run(); self.record('kit-rerun-both.log',result)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertEqual(NEW.read_text(),NEW_CONTENT)
        self.assertFalse(LEGACY.exists())
        self.assertEqual(Path('/tmp/naming-journald-restarts').read_text().splitlines(),['migrated'])
    def test_kit_refuses_foreign_new(self):
        self.seed(NEW,FOREIGN_CONTENT); self.seed(LEGACY,LEGACY_CONTENT)
        result=self.kit_run(); self.record('kit-rerun-foreign-new.log',result)
        self.assertNotEqual(result.returncode,0)
        self.assertIn('unrecognised journald drop-in',result.stdout+result.stderr)
        self.assertEqual(NEW.read_text(),FOREIGN_CONTENT)
        self.assertEqual(LEGACY.read_text(),LEGACY_CONTENT)
    def test_kit_refuses_foreign_file_at_legacy_name(self):
        self.seed(LEGACY,FOREIGN_CONTENT)
        result=self.kit_run(); self.record('kit-rerun-foreign-legacy.log',result)
        self.assertNotEqual(result.returncode,0,'kit touched a foreign legacy-name file')
        self.assertIn('unrecognised legacy journald drop-in',result.stdout+result.stderr)
        self.assertEqual(LEGACY.read_text(),FOREIGN_CONTENT)

if __name__=='__main__':
    unittest.main(verbosity=2)
