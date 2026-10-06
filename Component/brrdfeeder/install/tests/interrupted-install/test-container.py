#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Real passwd/filesystem recovery; only inside a disposable offline container.

Execute the shipped pre-flight through identity setup (before apt/network), and
the complete uninstaller. Only systemd/udev observations are inert fixtures.
"""
import os
from pathlib import Path
import subprocess
import unittest
import io
import importlib.util
import stat
from unittest.mock import patch

assert os.geteuid()==0 and Path('/run/.containerenv').exists()
assert os.environ.get('BRRD_INTERRUPTED_CONTAINER')=='1'
ROOT=Path('/repo/Component/brrdfeeder/install')
COMMAND=Path('/usr/local/sbin/brrdfeeder')
RECEIPT=Path('/etc/brrdfeeder/.installer-created-brrdfeeder')

def run(*args):
    return subprocess.run(args,text=True,capture_output=True,timeout=30)

class Recovery(unittest.TestCase):
    def setUp(self):
        for user in ('brrdfeeder','brrdhouse'):
            run('userdel',user); run('groupdel',user)
        for path in (COMMAND,RECEIPT): path.unlink(missing_ok=True)
        RECEIPT.parent.mkdir(exist_ok=True)
        run('groupadd','-g','985','brrdfeeder')
        result=run('useradd','--system','-u','999','-g','985','--no-create-home',
                   '--home-dir','/var/lib/brrdfeeder','--shell','/usr/sbin/nologin','brrdfeeder')
        self.assertEqual(result.returncode,0,result.stderr)
        COMMAND.write_bytes(b''); COMMAND.chmod(0o755)
        RECEIPT.write_bytes(b''); RECEIPT.chmod(0o600)
        stub=Path('/tmp/interrupted-stubs'); stub.mkdir(exist_ok=True)
        for name in ('systemctl','udevadm','loginctl'):
            p=stub/name
            p.write_text('#!/bin/sh\nif [ "$1" = show ]; then echo not-found; fi\nexit 0\n')
            p.chmod(0o755)
        os.environ['PATH']=str(stub)+':/usr/sbin:/usr/bin:/sbin:/bin'
    def install(self):
        source=(ROOT/'brrdfeeder-install.sh').read_text()
        script=Path('/tmp/interrupted-preflight.sh')
        script.write_text(source.split('# --- Bootstrap deps for enrollment',1)[0]+
                          '\nprintf "RECOVERY_PREFLIGHT_PASSED\\n"\n')
        return run('bash',str(script),'--image','ghcr.io/cybrrd/brrdfeeder@sha256:'+'a'*64,
                   '--console-image','ghcr.io/cybrrd/brrdhouse@sha256:'+'b'*64,
                   '--console-listen','127.0.0.1:8080')
    def uninstall(self):
        return run('bash',str(ROOT/'uninstall.sh'),'0','0','','confirm','1')
    def test_exact_leftovers_install_then_uninstall(self):
        result=self.install()
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertIn('RECOVERY_PREFLIGHT_PASSED',result.stdout)
        self.assertEqual(RECEIPT.read_text(),'v1:brrdfeeder:999:985:/var/lib/brrdfeeder:/usr/sbin/nologin\n')
        self.assertEqual(RECEIPT.stat().st_mode & 0o777,0o600)
        self.assertGreater(COMMAND.stat().st_size,0)
        self.clean_uninstall()
    def clean_uninstall(self):
        result=self.uninstall()
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertFalse(COMMAND.exists()); self.assertFalse(RECEIPT.exists())
        self.assertNotEqual(run('getent','passwd','brrdfeeder').returncode,0)
    def test_exact_leftovers_uninstall_directly(self):
        self.clean_uninstall()
    def test_atomic_temp_residue_uninstalls(self):
        temporary=RECEIPT.parent/'.brrdfeeder-atomic-abcdefgh'
        temporary.write_text('partial'); temporary.chmod(0o600)
        self.clean_uninstall()
        self.assertFalse(temporary.exists())
    def residue(self):
        home=Path('/var/lib/brrdfeeder')
        for relative in ('', '.config', '.config/systemd', '.config/systemd/user',
                         '.local', '.local/share', '.local/share/containers', '.cache'):
            path=home/relative
            path.mkdir(mode=0o700, exist_ok=True)
            os.chown(path,999,985)
        return home
    def test_empty_service_home_residue_upgrade_uninstalls(self):
        home=self.residue()
        self.clean_uninstall()
        self.assertFalse(home.exists())
    def test_unknown_residue_file_refuses_actionably_without_deleting(self):
        home=self.residue()
        unknown=home/'.config/operator-data'
        unknown.write_text('keep me')
        result=self.uninstall()
        self.assertNotEqual(result.returncode,0)
        self.assertIn('regular',result.stdout+result.stderr)
        self.assertIn('owner=',result.stdout+result.stderr)
        self.assertIn('emptiness=',result.stdout+result.stderr)
        self.assertIn('sudo stat --',result.stdout+result.stderr)
        self.assertIn('sudo brrdfeeder uninstall',result.stdout+result.stderr)
        self.assertEqual(unknown.read_text(),'keep me')
        unknown.unlink()
        self.clean_uninstall()
    def test_negative_controls(self):
        for bad in ('foreign-command','human-shell','human-home','unlocked','privileged','mismatched-receipt'):
            with self.subTest(bad=bad):
                self.setUp()
                if bad=='foreign-command': COMMAND.write_text('#!/bin/sh\necho foreign\n')
                if bad=='human-shell': run('usermod','-s','/bin/bash','brrdfeeder')
                if bad=='human-home': run('usermod','-d','/home/human','brrdfeeder')
                if bad=='unlocked': run('usermod','-p','hashed-password','brrdfeeder')
                if bad=='privileged': run('usermod','-aG','sudo','brrdfeeder')
                if bad=='mismatched-receipt': RECEIPT.write_text('v1:foreign:999:985:/var/lib/brrdfeeder:/usr/sbin/nologin\n')
                original=(COMMAND.read_bytes(),RECEIPT.read_bytes())
                for action in (self.install,self.uninstall):
                    result=action()
                    self.assertNotEqual(result.returncode,0,result.stdout+result.stderr)
                    self.assertEqual((COMMAND.read_bytes(),RECEIPT.read_bytes()),original)
                    self.assertEqual(run('getent','passwd','brrdfeeder').returncode,0)

class Durability(unittest.TestCase):
    def test_failed_config_transform_preserves_original(self):
        code=(ROOT/'brrdfeeder-install.sh').read_text().split("3<<'ATOMIC_INSTALL_PY'\n",1)[1].split('\nATOMIC_INSTALL_PY',1)[0]
        directory=Path('/etc/brrdfeeder'); directory.mkdir(exist_ok=True)
        target=directory/'atomic-transform'; target.write_bytes(b'old'); target.chmod(0o600)
        try:
            with patch('sys.argv',['atomic','0600','root','root',str(target),str(target),'false']):
                with self.assertRaises(subprocess.CalledProcessError): exec(compile(code,'atomic-install','exec'),{})
            self.assertEqual(target.read_bytes(),b'old')
            with patch('sys.argv',['atomic','0600','root','root',str(target),str(target),'sed','s/old/new/']):
                exec(compile(code,'atomic-install','exec'),{})
            self.assertEqual(target.read_bytes(),b'new')
        finally: target.unlink()

    def test_publish_order_and_pre_rename_failure(self):
        code=(ROOT/'brrdfeeder-install.sh').read_text().split("3<<'ATOMIC_INSTALL_PY'\n",1)[1].split('\nATOMIC_INSTALL_PY',1)[0]
        directory=Path('/etc/brrdfeeder'); directory.mkdir(exist_ok=True)
        target=directory/'atomic-test'; target.write_bytes(b'old'); target.chmod(0o600)
        original_sync=os.fsync; original_replace=os.replace
        for fail in (True,False):
            calls=[]
            def sync(fd):
                kind='dir' if stat.S_ISDIR(os.fstat(fd).st_mode) else 'file'
                calls.append('fsync-'+kind)
                if fail and kind=='file': raise OSError('seeded power loss before rename')
                original_sync(fd)
            def replace(src,dst):
                self.assertEqual(Path(src).parent,target.parent)
                self.assertEqual(Path(src).stat().st_mode & 0o777,0o600)
                self.assertEqual(Path(src).read_bytes(),b'new')
                calls.append('rename'); original_replace(src,dst)
            with patch('sys.argv',['atomic','0600','root','root',str(target)]), \
                 patch('sys.stdin',io.TextIOWrapper(io.BytesIO(b'new'))), \
                 patch('os.fsync',sync),patch('os.replace',replace):
                if fail:
                    with self.assertRaisesRegex(OSError,'seeded'): exec(compile(code,'atomic-install','exec'),{})
                    self.assertEqual(target.read_bytes(),b'old')
                    self.assertEqual(calls,['fsync-file'])
                else:
                    exec(compile(code,'atomic-install','exec'),{})
                    self.assertEqual(target.read_bytes(),b'new')
                    self.assertEqual(calls,['fsync-file','rename','fsync-dir'])
            self.assertFalse(list(directory.glob('.brrdfeeder-atomic-*')))
        target.unlink()

class RerunPins(unittest.TestCase):
    def test_one_liner_preserves_installed_pins_but_explicit_change_refuses(self):
        source=(ROOT/'brrdfeeder-install.sh').read_text()
        block=source.split('# Never resolve a registry tag here',1)[1].split('# Service identity',1)[0]
        engine='ghcr.io/cybrrd/brrdfeeder@sha256:'
        console='ghcr.io/cybrrd/brrdhouse@sha256:'
        quadlet=Path('/tmp/installed-engine'); quadlet.write_text('Image='+engine+'b'*64+'\n')
        console_quadlet=Path('/tmp/installed-console'); console_quadlet.write_text('Image='+console+'d'*64+'\n')
        script='''set -euo pipefail
fatal() { echo "$*"; exit 1; }; say() { echo "$*"; }
INSTALL_INTERFACE=; INSTALL_LATITUDE=; INSTALL_LONGITUDE=
CONSOLE_LISTEN=127.0.0.1:8080
CONFIG_PATH=/tmp/no-config
'''+f'''
IMAGE_REPOSITORY=ghcr.io/cybrrd/brrdfeeder
CONSOLE_REPOSITORY=ghcr.io/cybrrd/brrdhouse
QUADLET_FILE={quadlet}
CONSOLE_QUADLET_FILE={console_quadlet}
REQUESTED_IMAGE={engine+'a'*64}
REQUESTED_CONSOLE_IMAGE={console+'c'*64}
BOOT_PREPARE=$1
# Never resolve a registry tag here'''+block+'''
printf 'RETAINED %s %s\\n' "$CONTAINER_IMAGE" "$CONSOLE_IMAGE"
'''
        for bootstrap,expected in [('1',0),('0',1)]:
            result=run('bash','-c',script,'pins',bootstrap)
            self.assertEqual(result.returncode,expected,result.stdout+result.stderr)
            if bootstrap=='1': self.assertIn('RETAINED '+engine+'b'*64+' '+console+'d'*64,result.stdout)

class Upgrade(unittest.TestCase):
    def test_full_one_liner_host_repair_from_0822_a728(self):
        # Reuse the real-installer OS fixture. Only hardware and external
        # services are simulated; execute the shipped embedded BLE helper.
        os.environ['BRRD_NAMING_CONTAINER']='1'
        spec=importlib.util.spec_from_file_location('naming',ROOT/'tests/brrdfeeder-naming/container-tests.py')
        naming=importlib.util.module_from_spec(spec); spec.loader.exec_module(naming)
        fixture=naming.InstallerRerun('test_rerun_keeps_current_new_dropin')
        fixture.setUp(); self.addCleanup(fixture.tearDown)
        fixture.seed_installed_node()
        engine=Path('/etc/containers/systemd/brrdfeeder-engine.container')
        engine.parent.mkdir(parents=True,exist_ok=True)
        console=Path('/etc/brrdfeeder/brrdhouse.container')
        engine.write_text('# Installed by brrdfeeder-install.sh.\nImage=ghcr.io/cybrrd/brrdfeeder@sha256:'+'b'*64+'\n')
        console.write_text('Image=ghcr.io/cybrrd/brrdhouse@sha256:'+'d'*64+'\n')
        adapter=Path('/tmp/upgrade-usb/1-2'); adapter.mkdir(parents=True,exist_ok=True)
        (adapter/'idVendor').write_text('0bda'); (adapter/'idProduct').write_text('a728')
        uname=fixture.bin/'uname'; uname.write_text('#!/bin/sh\necho aarch64\n'); uname.chmod(0o755)
        # No engine container remains after this fixture's simulated stop.
        podman=fixture.bin/'podman'
        stub=podman.read_text().split('\n',1)
        podman.write_text(stub[0]+"\nimport sys\nif sys.argv[1:3] == ['container','exists']: sys.exit(1)\n"+stub[1])
        shim=fixture.bin/'python3'
        shim.write_text('''#!/usr/bin/python3
import importlib.util,importlib.machinery,json,os,sys
from pathlib import Path
if sys.argv[1:2]!=['/usr/local/libexec/brrdfeeder-bluetooth']:
    os.execv('/usr/bin/python3',['python3',*sys.argv[1:]])
def load(name,path):
    s=importlib.util.spec_from_loader(name,importlib.machinery.SourceFileLoader(name,str(path)))
    m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m
m=load('ble_shipped',sys.argv[1])
f=load('ble_fixture','/repo/Component/brrdfeeder/install/tests/pi-native-p0/test-ble.py')
m.USB=Path('/tmp/upgrade-usb')
m.ctl=f.Systemd()
m.ctl.engine='active'; m.ctl.pid='42'; m.ctl.stop_state='failed'
def down(adapter):
    assert adapter=={'usb_id':'0bda:a728'}
    assert m.ctl.enabled=='masked' and not m.ctl.active
m.controller_down=down
sys.argv=sys.argv[1:]
m.main()
Path('/tmp/upgrade-bluetooth-state').write_text(json.dumps([m.ctl.enabled,m.ctl.active]))
'''); shim.chmod(0o755)
        fixture.env['BRRDFEEDER_BOOTSTRAP_PREPARE']='1'
        result=fixture.rerun()
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        import json, yaml
        doc=yaml.safe_load(Path('/etc/brrdfeeder/config.yaml').read_bytes())
        self.assertEqual(doc['sensors']['rid_ble'],
                         {'enabled':True,'unblock_rfkill':True,'adapter':{'usb_id':'0bda:a728'}})
        self.assertEqual(json.loads(Path('/etc/brrdfeeder/.bluetooth-prior.json').read_bytes()),
                         {'version':1,'unit':'bluetooth.service','enabled':'enabled','active':True})
        self.assertIn('Image=ghcr.io/cybrrd/brrdfeeder@sha256:'+'b'*64,engine.read_text())
        self.assertIn('Image=ghcr.io/cybrrd/brrdhouse@sha256:'+'d'*64,console.read_text())

if __name__=='__main__': unittest.main(verbosity=2)
