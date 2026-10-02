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

if __name__=='__main__': unittest.main(verbosity=2)
