#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Real logger/PTY and read-only account-auditor tests. No host removal."""
from pathlib import Path
import importlib.util
import os
import pty
import select
import sqlite3
import struct
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[5]
INSTALL=ROOT/'Component/brrdfeeder/install'
HELPER=(INSTALL/'uninstall.sh').read_text()

def load(name, path):
    spec=importlib.util.spec_from_file_location(name,path)
    module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    return module

def function(name):
    lines=HELPER.splitlines(); start=next(i for i,l in enumerate(lines) if l.startswith(name+'() {'))
    end=start if lines[start].endswith('}') else next(i for i in range(start+1,len(lines)) if lines[i]=='}')
    return '\n'.join(lines[start:end+1])

class Account(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.mod=load('account',INSTALL/'uninstall-account.py')
        for attr in ['LOG','WTMPDB','LASTLOG2','PROC']:
            path=self.root/attr; path.mkdir(); setattr(self.mod,attr,path)

    def test_no_retained_history_and_no_processes(self):
        self.mod.history('brrdfeeder',999); self.mod.processes('brrdfeeder',999)

    def test_binary_history_each_supported_layout_and_other_user(self):
        for arch,size in [('x86_64',384),('aarch64',400)]:
            with self.subTest(arch=arch), patch.object(self.mod.platform,'machine',return_value=arch):
                record=bytearray(size); struct.pack_into('@h',record,0,7)
                record[44:49]=b'jdoe\0'; (self.mod.LOG/'wtmp').write_bytes(record)
                self.mod.history('brrdfeeder',999)
                record[44:54]=b'brrdfeeder'; (self.mod.LOG/'wtmp').write_bytes(record)
                with self.assertRaisesRegex(ValueError,'login history exists'): self.mod.history('brrdfeeder',999)

    def test_sqlite_history_both_schemas(self):
        for directory,name,ddl,insert in [
            (self.mod.WTMPDB,'wtmp.db','CREATE TABLE wtmp(User TEXT)','INSERT INTO wtmp VALUES(?)'),
            (self.mod.LASTLOG2,'lastlog2.db','CREATE TABLE Lastlog2(Name TEXT, Time INTEGER)','INSERT INTO Lastlog2 VALUES(?,1)')]:
            with self.subTest(name=name):
                path=directory/name
                with sqlite3.connect(path) as db: db.execute(ddl); db.execute(insert,('jdoe',))
                before=path.read_bytes(); self.mod.history('brrdfeeder',999)
                self.assertEqual(before,path.read_bytes())
                with sqlite3.connect(path) as db: db.execute(insert,('brrdfeeder',))
                with self.assertRaisesRegex(ValueError,'login history exists'): self.mod.history('brrdfeeder',999)
                path.unlink()

    def test_unreadable_format_and_active_transaction_refuse(self):
        path=self.mod.LOG/'wtmp'; path.write_bytes(b'unknown')
        with self.assertRaisesRegex(ValueError,'binary format'): self.mod.history('brrdfeeder',999)
        path.write_bytes(b''); (self.mod.LOG/'wtmp-wal').write_bytes(b'pending')
        with self.assertRaisesRegex(ValueError,'being updated'): self.mod.history('brrdfeeder',999)

    def test_lastlog(self):
        path=self.mod.LOG/'lastlog'
        with patch.object(self.mod.platform,'machine',return_value='aarch64'):
            with path.open('wb') as stream:
                stream.truncate(1000*296); stream.seek(999*296); stream.write((123).to_bytes(8,sys.byteorder))
            with self.assertRaisesRegex(ValueError,'login history exists'): self.mod.history('brrdfeeder',999)

    def process(self,group):
        path=self.mod.PROC/'31415'; path.mkdir(exist_ok=True)
        (path/'status').write_text('Uid:\t999\t999\t999\t999\n')
        (path/'cgroup').write_text('0::'+group+'\n')

    def test_only_verified_package_processes(self):
        expected='/system.slice/brrdfeeder-engine.service'
        with patch.object(self.mod,'command',return_value=expected):
            self.process(expected+'/libpod-payload'); self.mod.processes('brrdfeeder',999)
            self.process('/system.slice/unrelated.service')
            with self.assertRaisesRegex(ValueError,'unrelated process'): self.mod.processes('brrdfeeder',999)
        with patch.object(self.mod,'command',return_value=''):
            self.process(expected)
            with self.assertRaisesRegex(ValueError,'unrelated process'): self.mod.processes('brrdfeeder',999)

class Profile(unittest.TestCase):
    def check(self,field='',value='',adopt=0):
        # Execute the actual shell identity-validation loop, replacing only OS
        # observations. Separate tests above execute the real history/proc code.
        body=HELPER.split('declare -A uid=() gid=()\n',1)[1].split('\n  fi\ndone\n',1)[0]+'\n  fi\ndone\n'
        fixture=r'''set -euo pipefail
stage=confirm; adopt=$3
declare -A uid=() gid=()
log() { echo "$*"; }
notice() { echo "$*"; }
die() { echo "REFUSED: $*"; exit 1; }
absent() { :; }
safe_tree() { :; }; safe_file() { :; }; safe_path() { :; }
account_uid=999; account_home=/var/lib/brrdfeeder; account_shell=/usr/sbin/nologin
account_groups=brrdfeeder; account_password=L; audit_result=0
if [[ -n $1 ]]; then printf -v "$1" %s "$2"; fi
getent() {
 case "$1:${2:-}" in
   passwd:brrdfeeder|passwd:) printf 'brrdfeeder:x:%s:999::%s:%s\n' "$account_uid" "$account_home" "$account_shell";;
   group:999) echo 'brrdfeeder:x:999:';;
   *) return 2;;
 esac
}
id() { if [[ $1 == -G ]]; then echo 999; else echo "$account_groups"; fi; }
passwd() { echo "brrdfeeder $account_password"; }
audit_legacy_account() { if (( audit_result )); then echo 'REFUSED: seeded login history / unrelated process'; return 1; fi; }
'''
        return subprocess.run(['bash','-c',fixture+body,'profile',field,value,str(adopt)],capture_output=True,text=True)

    def test_automatic_legacy_recognition(self):
        result=self.check(); self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertIn('recognised brrdfeeder as a BRRDfeeder service account',result.stdout)

    def test_each_bad_profile_refuses_even_with_expert_flag(self):
        for field,value,reason in [('account_uid','99','UID is below'),('account_home','/home/jdoe','home is not'),
            ('account_shell','/bin/bash','login shell'),('account_password','P','password is not locked'),
            ('account_groups','brrdfeeder sudo','privileged group sudo'),
            ('account_groups','brrdfeeder adm','privileged group adm'),
            ('account_groups','brrdfeeder wheel','privileged group wheel'),('audit_result','1','seeded login history')]:
            for adopt in [0,1]:
                with self.subTest(field=field,value=value,adopt=adopt):
                    result=self.check(field,value,adopt)
                    self.assertEqual(result.returncode,1,result.stdout+result.stderr)
                    self.assertIn(reason,result.stdout)
                    self.assertNotIn('recognised brrdfeeder as',result.stdout)

class Confirmation(unittest.TestCase):
    def run_fixture(self,answer=None,yes=False,dry=False):
        with tempfile.TemporaryDirectory() as tmp:
            directory=Path(tmp); child=directory/'child.sh'; applied=directory/'applied'
            code='set -euo pipefail\n'+(INSTALL/'log-events.sh').read_text()+'\n'
            code+='\n'.join(function(n) for n in ['log','notice','die','confirm_removal'])
            code+=f'\nstage=confirm; dry={int(dry)}; yes={int(yes)}\n'
            code+='for ((n=0;n<80;n++)); do log "WOULD: fixture-item-$n"; done\nconfirm_removal\n'
            code+=f'if (( ! dry )); then touch {applied}; fi\n'
            child.write_text(code)
            runner=directory/'runner.py'
            runner.write_text(f'''import importlib.util,sys
from pathlib import Path
spec=importlib.util.spec_from_file_location('logger',{str(INSTALL/'install-log.py')!r})
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
m.LOG_DIR=Path({str(directory/'logs')!r});m.CREDS=Path({str(directory/'absent')!r})
m.environment=lambda **kw: 'offline fixture'
sys.exit(m.supervise({str(child)!r},['--uninstall','--no-verbose']))
''')
            if answer is None:
                result=subprocess.run([sys.executable,str(runner)],text=True,capture_output=True,start_new_session=True,timeout=10)
                rc=result.returncode; terminal=result.stdout+result.stderr
            else:
                pid,fd=pty.fork()
                if pid==0: os.execv(sys.executable,[sys.executable,str(runner)])
                data=b''; sent=False; deadline=time.monotonic()+15
                try:
                    while time.monotonic()<deadline:
                        ready,_,_=select.select([fd],[],[],0.1)
                        if ready:
                            try: chunk=os.read(fd,65536)
                            except OSError: break
                            if not chunk: break
                            data+=chunk
                            if b'Remove BRRDfeeder from this Pi?' in data and not sent:
                                os.write(fd,(answer+'\n').encode()); sent=True
                    else: os.kill(pid,9); self.fail('confirmation timed out')
                finally: os.close(fd)
                _,status=os.waitpid(pid,0); rc=os.waitstatus_to_exitcode(status); terminal=data.decode()
            logs=list((directory/'logs').glob('uninstall-*.log'))
            self.assertEqual(len(logs),1,terminal)
            log=logs[0].read_text()
            self.assertEqual(log.split('==== RESULT ====')[0].count('WOULD: fixture-item-'),80)
            self.assertNotIn('WOULD:',terminal)
            summary=terminal.split('BRRDfeeder removal plan:',1)[1].split('Remove BRRDfeeder from this Pi?',1)[0]
            # The seven plan lines; failure summary is counted separately.
            self.assertLessEqual(len([l for l in summary.splitlines() if l.startswith('  ')]),11)
            return rc,terminal,log,applied.exists()

    def test_no_terminal_refuses_before_mutation(self):
        rc,out,log,applied=self.run_fixture(); self.assertEqual(rc,1,out)
        self.assertIn('No terminal',out); self.assertIn('--yes',out); self.assertFalse(applied)
        self.assertIn('result=FAILED',log)

    def test_one_prompt_yes_and_cancel(self):
        for answer,expected in [('y',0),('n',1),('',1)]:
            with self.subTest(answer=answer):
                rc,out,log,applied=self.run_fixture(answer=answer)
                self.assertEqual(rc,expected,out); self.assertEqual(applied,expected==0)
                self.assertEqual(out.count('Remove BRRDfeeder from this Pi?'),1)

    def test_yes_and_dry_need_no_terminal(self):
        for yes,dry in [(True,False),(False,True)]:
            with self.subTest(yes=yes,dry=dry):
                rc,out,log,applied=self.run_fixture(yes=yes,dry=dry)
                self.assertEqual(rc,0,out); self.assertEqual(applied,not dry)
                self.assertNotIn('Remove BRRDfeeder from this Pi?',out)

if __name__=='__main__': unittest.main(verbosity=2)
