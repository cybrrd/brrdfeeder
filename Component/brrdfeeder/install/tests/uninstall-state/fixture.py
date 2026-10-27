# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Complete uninstaller with real files/accounts in a disposable offline OS.

Only systemd, loginctl, Podman and optional mount observations are simulated.
rm/find/userdel wrappers observe secret permissions/order, then run the real tool.
"""
import base64
import binascii
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest

INSTALL = Path(__file__).resolve().parents[2]
S = Path('/var/lib/brrdfeeder')
P = Path('/var/lib/brrdfeeder-updater')
T = Path('/var/lib/brrdfeeder-status')
E = Path('/etc/brrdfeeder')
TREES = (S, P, T, E)
UID, GID = 999, 985
KEY_RAW = bytes([20 << 3]) + bytes(range(32))
PUBLIC_KEY = base64.b32encode(KEY_RAW + binascii.crc_hqx(KEY_RAW, 0).to_bytes(2, 'little')).decode()
RENEW = '.renew-' + PUBLIC_KEY


def guard():
    assert os.geteuid() == 0 and Path('/run/.containerenv').exists(), 'disposable container only'
    assert os.environ.get('BRRD_UNINSTALL_STATE_CONTAINER') == '1', 'explicit fixture opt-in required'
    assert not any(line.split()[0] != 'lo' for line in Path('/proc/net/route').read_text().splitlines()[1:]), 'network must be disabled'


def command(*args, **kwargs):
    return subprocess.run(args, capture_output=True, text=True, timeout=45, **kwargs)


def directory(path, mode, uid=0, gid=0):
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(mode); os.chown(path, uid, gid)


def file(path, mode=0o600, uid=0, gid=0, data='{}\n'):
    path.write_text(data); path.chmod(mode); os.chown(path, uid, gid)
    return path


class Fixture(unittest.TestCase):
    def setUp(self):
        guard()
        self.original_env = dict(os.environ)
        self.temp = tempfile.TemporaryDirectory(prefix='uninstall-state-')
        self.work = Path(self.temp.name)
        self.bin = self.work/'bin'; self.bin.mkdir()
        self.calls = self.work/'calls'
        self.helper = (INSTALL/'uninstall.sh').read_text()
        self.clean()
        self.stubs()

    def tearDown(self):
        self.clean()
        os.environ.clear(); os.environ.update(self.original_env)
        self.temp.cleanup()

    def clean(self):
        guard()
        # Canonical fixture roots only. The guard runs before every destructive
        # fixture reset; the host repository is mounted read-only by the runner.
        for user in ('brrdfeeder', 'brrdhouse'):
            command('/usr/sbin/userdel', user); command('/usr/sbin/groupdel', user)
        for path in TREES:
            if path.is_symlink(): path.unlink()
            elif path.exists(): shutil.rmtree(path)
        if hasattr(self, 'calls'): self.calls.unlink(missing_ok=True)

    def stubs(self):
        for name in ('systemctl', 'loginctl', 'podman'):
            text='''#!/usr/bin/python3
import os,sys
with open(os.environ['FIXTURE_CALLS'],'a') as f: f.write(os.path.basename(sys.argv[0])+' '+' '.join(sys.argv[1:])+'\\n')
if os.path.basename(sys.argv[0]) == 'systemctl':
    if sys.argv[1:2] == ['show']: print('not-found')
    if sys.argv[1:2] == ['is-failed']: sys.exit(1)
if os.path.basename(sys.argv[0]) == 'podman' and sys.argv[1:3] == ['container','exists']: sys.exit(1)
'''
            file(self.bin/name, 0o755, data=text)
        for name, real in [('rm','/usr/bin/rm'), ('find','/usr/bin/find'), ('userdel','/usr/sbin/userdel')]:
            text='''#!/usr/bin/python3
import os,stat,sys
from pathlib import Path
name=os.path.basename(sys.argv[0])
if name != 'find' or '-delete' in sys.argv:
    p=Path('/var/lib/brrdfeeder/credentials')
    if p.exists():
        assert stat.S_IMODE(p.lstat().st_mode)==0o700
        assert (p.lstat().st_uid,p.lstat().st_gid)==(999,985)
        for child in p.iterdir():
            assert stat.S_ISREG(child.lstat().st_mode)
            assert stat.S_IMODE(child.lstat().st_mode)==0o600
    if name=='userdel' and 'brrdfeeder' in sys.argv: assert not p.exists(), 'secrets must disappear before account deletion'
    with open(os.environ['FIXTURE_CALLS'],'a') as f: f.write('secret-order-ok '+name+' '+' '.join(sys.argv[1:])+'\\n')
'''+f'os.execv({real!r},[{real!r},*sys.argv[1:]])\n'
            file(self.bin/name, 0o755, data=text)
        os.environ['PATH'] = str(self.bin)+':/usr/sbin:/usr/bin:/sbin:/bin'
        os.environ['FIXTURE_CALLS'] = str(self.calls)

    def seed(self, version='0.9.0', optional=False, crash=False):
        self.clean()
        for args in [('groupadd','-g',str(GID),'brrdfeeder'),
                     ('useradd','--system','-u',str(UID),'-g',str(GID),'--no-create-home',
                      '--home-dir',str(S),'--shell','/usr/sbin/nologin','brrdfeeder')]:
            result=command(*args); self.assertEqual(result.returncode,0,result.stderr)
        directory(S,0o750,UID,GID); directory(P,0o700)
        directory(T,0o755,UID,GID); directory(E,0o755)
        directory(E/'secrets',0o750,0,GID)
        file(E/'.installer-created-brrdfeeder',data=f'v1:brrdfeeder:{UID}:{GID}:{S}:/usr/sbin/nologin\n')
        config='node:\n  status_file: '+str(S if optional else T)+'/status.json\ncapture:\n  interface: fixture0\n'
        if optional: config+='  savefile: {}\n'
        file(E/'config.yaml',0o644,data=config)
        file(E/'updater.json')
        file(E/'secrets/brrdfeeder.creds',0o640,0,GID,'synthetic bootstrap credential\n')
        file(E/'secrets/oauth_refresh.token',data='synthetic refresh token\n')
        for name in ('policy_state.json','release_attempt.json','release_currency.json','update_transaction.json','update_outcome.json'):
            file(S/name,0o644)
        for name in ('state.json','attempt.json','pending_update.json','lock','launch.lock','host-state.json','host-transaction.json'):
            file(P/name)
        directory(S/'upward',0o700,UID,GID)
        for lane in ('operational','red'):
            for suffix in ('json','tmp','lock'): file(S/'upward'/f'{lane}.{suffix}',0o600,UID,GID)
        for name in ('status.json','startup.json','status.json.tmp.101.0','.startup.json-abcd_123'):
            file(T/name,0o644,UID,GID)
        if version=='0.9.0':
            directory(S/'credentials',0o700,UID,GID)
            for name in ('brrdfeeder.creds','previous.creds','.renew.lock',RENEW):
                file(S/'credentials'/name,0o600,UID,GID,'synthetic private artifact\n')
        if optional:
            file(S/'status.json',0o644,UID,GID)
            file(S/'status.json.tmp.101.0',0o600,UID,GID)
            file(S/'status.json.tmp.102.1',0o644,UID,GID)
            directory(S/'capture',0o755,UID,GID)
            for slot in range(16): file(S/'capture'/f'capture-{slot:02}.pcap',0o600,UID,GID,'synthetic capture\n')
        if crash:
            file(S/'.policy_state.json.tmp',0o644,UID,GID)
            file(S/'.update-123',0o644)
            file(P/'.update-123')
            file(P/'.pending-987654')
            os.link(P/'pending_update.json',P/'.pending-123456')
            file(P/'.host-recover-abcd_123')
            file(E/'.host-recover-abcd_123')
            file(E/'.config.yaml-abcd_123',0o600)
            file(E/'.config.yaml-abcd_124',0o644)
            file(E/'.update-0')
            file(E/'.update-4294967295',0o644)
        return self

    def snapshot(self):
        entries=[]
        for root in TREES:
            for base, directories, files in os.walk(root,followlinks=False):
                for path in [Path(base),*(Path(base)/p for p in directories+files)]:
                    info=path.lstat()
                    value=(os.readlink(path) if stat.S_ISLNK(info.st_mode) else
                           path.read_bytes() if stat.S_ISREG(info.st_mode) else '')
                    entries.append((str(path),info.st_mode,info.st_uid,info.st_gid,info.st_ino,info.st_nlink,value))
        return sorted(set(entries)),command('getent','passwd','brrdfeeder').stdout,command('getent','group','brrdfeeder').stdout

    def uninstall(self,dry=False):
        path=self.work/'uninstall.sh'; path.write_text(self.helper)
        return command('bash',str(path),str(int(dry)),'0','','confirm','1')

    def positive(self):
        before=self.snapshot()
        result=self.uninstall(dry=True)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertEqual(self.snapshot(),before)
        self.assertFalse(self.calls.exists(),'dry-run called an effector')
        result=self.uninstall()
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        for path in TREES: self.assertFalse(path.exists(),str(path))
        self.assertNotEqual(command('getent','passwd','brrdfeeder').returncode,0)
        calls=self.calls.read_text()
        self.assertIn('secret-order-ok userdel brrdfeeder',calls)
        self.assertLess(calls.index('systemctl show brrdfeeder-engine.service'),calls.index('secret-order-ok'))

    def refused(self,reason):
        before=self.snapshot()
        for dry in (True,False):
            result=self.uninstall(dry)
            self.assertNotEqual(result.returncode,0,result.stdout+result.stderr)
            self.assertIn(reason,result.stdout+result.stderr)
            self.assertEqual(self.snapshot(),before)
            self.assertFalse(self.calls.exists(),'refusal reached an effector')
