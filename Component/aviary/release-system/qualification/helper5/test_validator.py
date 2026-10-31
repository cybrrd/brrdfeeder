#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Synthetic refusal controls only; no native or production access."""
import base64
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import validate as v
from binding import HASH_FIELDS, TEXT_FIELDS, validate as binding_validate

class Evidence(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name);self.count=0
        b={k:v.sha(k.encode()) for k in HASH_FIELDS}
        b.update({k:'synthetic-'+k for k in TEXT_FIELDS})
        b.update(from_build=4,to_build=5,baseline_version='0.9.0',source_commit='a'*40,baseline_source_commit='b'*40)
        self.b=b
        binary='/usr/local/libexec/brrdfeeder-release';receipt='/etc/brrdfeeder/.updater-helper.sha256'
        self.binary=binary;self.receipt=receipt;self.floor='/var/lib/brrdfeeder-updater/host-state.json'
        b['baseline_helper_sha256']=v.sha(b'old')
        def item(raw,mode=0o600): return dict(data=base64.b64encode(raw).decode(),sha256=v.sha(raw),mode=mode,uid=0,gid=0)
        pre={binary:item(b'old',0o755),receipt:item((b['baseline_helper_sha256']+'  '+binary+'\n').encode()),self.floor:None,
             binary+'.previous':None,binary+'.previous.sha256':None,binary+'-launch':item(b'launcher',0o755)}
        self.pre=self.put(pre)
        self.regraw=json.dumps(dict(binding=b,preimages_sha256=self.pre['sha256'],protected_sha256='d'*64)).encode()
        self.regsha=v.sha(self.regraw)
        events=[]
        for thread in (1,2):
            for repeat in range(3):
                for fd,isdir,ret in ((3,True,0),(4,False,0),(-1,False,-9)):
                    entry=dict(syscall=82,reason='syscall-entry',ptid=[1,thread,0],fd=fd,path='/fixture' if fd>=0 else None,is_directory=isdir)
                    events.extend([entry,dict(entry,reason='syscall-return',return_value=ret)])
        calibration=dict(status='PASS',native_arm64=True,mode='calibrate',registers=['$x0','$x0'],syscall=82,threads=2,pairs=18,pending=0)
        self.e=dict(registration_sha256=self.regsha,host=b['hostname'],node_id=b['node_id'],from_build=4,to_build=5,binding=b,
                    preimages=self.pre,calibration=self.put(calibration),calibration_events=self.put(events),rows={})
        health=self.put(dict(host=b['hostname'],node_id=b['node_id'],observed_at=1000,radio=True,gps=True,clock=True,engine=True,console=True,broker=True))
        final=self.put(dict(helper_sha256=b['baseline_helper_sha256'],build=4,effective_floor=4,journal=None,six_preimages_sha256=self.pre['sha256'],floor_preimage_exact=True))
        for name in v.ROWS:
            row=dict(host=b['hostname'],node_id=b['node_id'],registration_sha256=self.regsha,preimages_sha256=self.pre['sha256'],protected_before_sha256='d'*64,protected_after_sha256='d'*64,
                     start=1000,end=1000,health_before=health,health_after=health,final_state=final,final_preimages=self.pre)
            self.e['rows'][name]=row
            if name=='normal':
                row.update(normal_trace=self.put(b'execve("fixture", [], []) = 0\n'),idempotent_trace=self.put(b'execve("fixture", [], []) = 0\n'))
                installed=self.put(dict(helper_sha256=b['candidate_sha256'],build=5,floor=5,journal=None,metadata={'inode':123,'mtime_ns':987}))
                row.update(installed_first=installed,installed_second=installed)
            elif name.startswith('reboot-'):
                row.update(boot_before='a'*8+'-'+('a'*4+'-')*3+'a'*12,boot_after='b'*8+'-'+('b'*4+'-')*3+'b'*12)
                interrupted=name=='reboot-interrupted'
                row['boot_service']=self.put(dict(boot_id=row['boot_after'],result='success',exit_status=0,started_monotonic=10,invocation_id='fixture',restored_locally=interrupted,build=4 if interrupted else 5,helper_sha256=b['baseline_helper_sha256' if interrupted else 'candidate_sha256']))
            elif name=='handback': row.update(timers_restored=True,normal_cycle_observed=True)
            else:
                phase,route=name.split('-');row.update(phase=phase,route=route)
                path='/usr/local/libexec' if phase=='switched' else '/etc/brrdfeeder' if phase=='receipted' else '/var/lib/brrdfeeder-updater'
                entry=dict(syscall=82,reason='syscall-entry',ptid=[2,2,0],fd=3,path=path,is_directory=True)
                row['events']=self.put([entry,dict(entry,reason='syscall-return',return_value=0,phase=phase)])
                rawfloor=b'' if phase not in ('floored','committed') else b'{"next_poll":"0001-01-01T00:00:00Z","seen":{"sequence":0,"hash":""},"attempted":{},"build_seq":5}'
                row['floor_at_stop']=self.put(rawfloor)
                snap={binary:{'sha256':b['baseline_helper_sha256' if phase=='prepared' else 'candidate_sha256']},receipt:{'sha256':v.sha((b['baseline_helper_sha256' if phase in ('prepared','switched') else 'candidate_sha256']+'  '+binary+'\n').encode())},self.floor:{'sha256':v.sha(rawfloor)} if rawfloor else None,'/var/lib/brrdfeeder-updater/host-transaction.json':None if phase=='committed' else {'sha256':'e'*64}}
                row['at_stop']=self.put(dict(phase=phase,all_threads_stopped=True,syscall_return=0,entry=entry,state=snap))
                row['gdb_result']=self.put(dict(native_arm64=True,killed=True,phase=phase,seen=list(v.PHASES[:v.PHASES.index(phase)+1])))
                row['recovery_build']=5 if phase=='committed' and route=='launcher' else 4

    def put(self,value):
        self.count+=1;name=str(self.count)+'.fixture'
        raw=value if isinstance(value,bytes) else json.dumps(value).encode()
        (self.root/name).write_bytes(raw);return dict(path=name,sha256=v.sha(raw))
    def doc(self,ref): return json.loads((self.root/ref['path']).read_bytes())
    def check(self): return v.validate(self.e,self.regraw,self.regsha,self.root)
    def test_complete_fixture_requires_review(self): self.assertEqual(self.check(),'EVIDENCE_COMPLETE_REVIEW_REQUIRED')
    def test_unbound_template(self):
        for key in HASH_FIELDS+TEXT_FIELDS:
            b=dict(self.b);b[key]=None
            with self.subTest(key=key),self.assertRaises(ValueError): binding_validate(b)
    def test_identity_refusals(self):
        for key,value in [('host','wrong'),('node_id','wrong'),('from_build',2),('to_build',4),('registration_sha256','0'*64)]:
            with self.subTest(key=key):
                old=self.e[key];self.e[key]=value
                with self.assertRaises(ValueError): self.check()
                self.e[key]=old
        for key in HASH_FIELDS+('source_commit','baseline_source_commit'):
            old=self.e['binding'];self.e['binding']=dict(old);self.e['binding'][key]='0'*len(old[key])
            with self.subTest(key=key),self.assertRaises(ValueError): self.check()
            self.e['binding']=old
    def test_missing_phase(self):
        del self.e['rows']['floored-preimages']
        with self.assertRaises(ValueError): self.check()
    def test_row_refusals(self):
        for row,key,value in [('switched-launcher','route','preimages'),('prepared-launcher','phase','committed'),('normal','protected_after_sha256','0'*64),('normal','preimages_sha256','0'*64),('reboot-interrupted','boot_after',self.e['rows']['reboot-interrupted']['boot_before']),('handback','timers_restored',False)]:
            with self.subTest(row=row,key=key):
                old=self.e['rows'][row][key];self.e['rows'][row][key]=value
                with self.assertRaises(ValueError): self.check()
                self.e['rows'][row][key]=old
    def test_capture_refusals(self):
        cases=[('normal','health_after','observed_at',100),('normal','installed_first','floor',4),('normal','final_state','floor_preimage_exact',False),('reboot-committed','boot_service','build',4),('floored-launcher','at_stop','syscall_return',-5),('switched-preimages','gdb_result','native_arm64',False)]
        for row,key,field,value in cases:
            with self.subTest(row=row,key=key,field=field):
                old=self.e['rows'][row][key];doc=self.doc(old);doc[field]=value;self.e['rows'][row][key]=self.put(doc)
                with self.assertRaises(ValueError): self.check()
                self.e['rows'][row][key]=old
    def test_network_missing_or_present(self):
        for raw in (b'',b'execve("fixture") = 0\nsocket(AF_INET6, SOCK_STREAM, 0) = 3'):
            self.e['rows']['normal']['normal_trace']=self.put(raw)
            with self.assertRaises(ValueError): self.check()
    def test_bad_syscall_pair(self):
        row=self.e['rows']['switched-launcher'];events=self.doc(row['events']);events[1]['ptid']=[2,3,0];row['events']=self.put(events)
        with self.assertRaises(ValueError): self.check()
    def test_wrong_phase_floor(self):
        row=self.e['rows']['floored-preimages'];row['floor_at_stop']=self.put(b'{"build_seq":4}')
        with self.assertRaises(ValueError): self.check()
    def test_edited_capture(self):
        (self.root/self.e['calibration']['path']).write_text('{}')
        with self.assertRaises(ValueError): self.check()

if __name__=='__main__': unittest.main()
