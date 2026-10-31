#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Exercise restoration decisions in an in-memory filesystem; no host writes."""
import base64
import copy
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

HERE=Path(__file__).resolve().parent

def item(raw,mode=0o600):
    import hashlib
    return dict(data=base64.b64encode(raw).decode(),sha256=hashlib.sha256(raw).hexdigest(),mode=mode,uid=0,gid=0)

class Restore(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name);(self.root/'launcher5.py').write_bytes(b'new launcher')
        binding=types.ModuleType('binding')
        binding.load_binding=lambda: dict(candidate_sha256=item(b'new')['sha256'],baseline_helper_sha256=item(b'old')['sha256'])
        binding.guard=lambda:None
        spec=importlib.util.spec_from_file_location('preimages',HERE/'helper-preimages.py')
        m=importlib.util.module_from_spec(spec)
        with patch.dict(sys.modules,binding=binding): spec.loader.exec_module(m)
        self.m=m;m.__file__=str(self.root/'helper-preimages.py')
        self.prior={str(p):None for p in m.PATHS}
        self.prior.update({str(m.BINARY):item(b'old',0o755),str(m.RECEIPT):item((item(b'old')['sha256']+'  '+str(m.BINARY)+'\n').encode()),str(m.LAUNCH):item(b'old launcher',0o755)})
        self.current={Path(p):copy.deepcopy(x) for p,x in self.prior.items()}
        self.current[m.BINARY]=item(b'new',0o755)
        self.current[m.RECEIPT]=item((m.PIN+'  '+str(m.BINARY)+'\n').encode())
        self.current[m.FLOOR]=item(b'{"next_poll":"0001-01-01T00:00:00Z","seen":{"sequence":0,"hash":""},"attempted":{},"build_seq":5}')
        self.current[m.LAUNCH]=item(b'new launcher',0o755)
        self.current[m.PATHS[3]]=item(b'old',0o755)
        self.current[m.PATHS[4]]=item((item(b'old')['sha256']+'\n').encode())
        self.journal=dict(previous_sha256=item(b'old')['sha256'],candidate_sha256=m.PIN,local_upgrade=True,previous_host_state='',previous_host_state_sha256=item(b'')['sha256'])
        self.current[m.TX]=item(json.dumps(self.journal).encode())
        self.writes=[]
    def run_restore(self,wrong_registration=False):
        m=self.m;raw=json.dumps(self.prior).encode();manifest=item(raw)
        self.current[self.root/'preimages.json']=manifest
        def replace(path,value): self.writes.append(path);self.current[path]=value
        with patch.object(m,'guard',lambda:None),patch.object(m.os,'geteuid',lambda:0),patch.object(m.platform,'machine',lambda:'aarch64'),patch.object(m,'secure_parents',lambda p:None),patch.object(m,'read',lambda p:self.current.get(p)),patch.object(m,'replace',replace),patch.object(sys,'argv',['helper-preimages.py','restore',str(self.root),'--manifest-sha256','0'*64 if wrong_registration else manifest['sha256']]):
            m.main()
    def test_exact_restore_old_launcher_last(self):
        self.run_restore()
        self.assertEqual(self.writes,self.m.PATHS[:-1]+[self.m.TX,self.m.LAUNCH])
        for p in self.m.PATHS:self.assertEqual(self.current[p],self.prior[str(p)])
        self.assertIsNone(self.current[self.m.FLOOR])
    def test_committed_restore(self):
        self.current[self.m.TX]=None;self.run_restore()
        self.assertEqual(self.current[self.m.BINARY],self.prior[str(self.m.BINARY)])
    def test_refuses_everything_before_write(self):
        cases=['registration','binary','floor','journal','journal-floor','late-preimage','launcher','receipt','previous']
        for case in cases:
            with self.subTest(case=case):
                original=copy.deepcopy(self.current);prior=copy.deepcopy(self.prior)
                if case=='binary':self.current[self.m.BINARY]=item(b'unrelated update',0o755)
                elif case=='floor':self.current[self.m.FLOOR]=item(b'{"build_seq":6}')
                elif case in ('journal','journal-floor'):
                    j=dict(self.journal);j['candidate_sha256' if case=='journal' else 'previous_host_state_sha256']='0'*64
                    self.current[self.m.TX]=item(json.dumps(j).encode())
                elif case=='late-preimage':self.prior[str(self.m.LAUNCH)]['sha256']='0'*64
                elif case=='launcher':self.current[self.m.LAUNCH]=item(b'unrelated launcher',0o755)
                elif case=='receipt':self.current[self.m.RECEIPT]=item(b'corrupt receipt')
                elif case=='previous':self.current[self.m.PATHS[3]]=item(b'unrelated preimage',0o755)
                with self.assertRaises(RuntimeError):self.run_restore(case=='registration')
                self.assertEqual(self.writes,[])
                self.current=original;self.prior=prior

if __name__=='__main__':unittest.main()
