#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Synthetic metadata inventory and no-effect contracts; no physical probes."""
import ast
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[5]
HERE=ROOT/'Component/brrdfeeder/devices'
spec=importlib.util.spec_from_file_location('scanner',HERE/'scan.py');scan=importlib.util.module_from_spec(spec);spec.loader.exec_module(scan)
RAW=(HERE/'supported-devices.json').read_bytes()

class Inventory(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.sys=Path(self.tmp.name)/'sys';(self.sys/'bus/usb/devices').mkdir(parents=True);(self.sys/'devices').mkdir()
    def device(self,name,identity='10c4:ea60',driver='cp210x'):
        node=self.sys/'devices'/name;node.mkdir();vid,pid=identity.split(':');(node/'idVendor').write_text(vid);(node/'idProduct').write_text(pid)
        interface=node/(name+':1.0');interface.mkdir()
        if driver:
            target=self.sys/'bus/usb/drivers'/driver;target.mkdir(parents=True,exist_ok=True);(interface/'driver').symlink_to(target)
        (self.sys/'bus/usb/devices'/name).symlink_to(node)
        return node
    def test_candidate_is_not_qualification(self):
        self.device('1-1');row=scan.scan(self.sys,RAW)['devices'][0]
        self.assertEqual(row['profiles'][0]['state'],'candidate')
        self.assertEqual((row['capability'],row['ownership'],row['health']),('unverified',)*3)
        self.assertIn('unverified',row['profiles'][0]['label'])
    def test_unknown_missing_wrong_driver_and_ambiguity(self):
        self.device('1-1');self.device('1-2');self.device('1-3','ffff:ffff');self.device('1-4','1546:01a7',None);self.device('1-5','0bda:876e','wrong')
        result=scan.scan(self.sys,RAW);rows=result['devices']
        self.assertEqual(result['ambiguous_roles'],['gps']);self.assertEqual([r['instance'] for r in rows],sorted(r['instance'] for r in rows))
        self.assertEqual(rows[2]['state'],'unsupported');self.assertEqual(rows[3]['profiles'][0]['state'],'driver-missing');self.assertEqual(rows[4]['profiles'][0]['state'],'driver-mismatch')
    def test_schema_and_duplicate_refusal(self):
        for mutation in ('schema','duplicate','driver'):
            doc=json.loads(RAW)
            if mutation=='schema':doc['schema_version']=99
            elif mutation=='duplicate':doc['profiles'].append(doc['profiles'][0])
            else:doc['profiles'][0]['drivers']=['$(unsafe)']
            with self.assertRaises(ValueError):scan.catalog(json.dumps(doc).encode())
    def test_no_devices_no_mutation(self):
        self.device('1-1');before={str(p):p.read_bytes() for p in self.sys.rglob('*') if p.is_file()}
        scan.scan(self.sys,RAW)
        self.assertEqual(before,{str(p):p.read_bytes() for p in self.sys.rglob('*') if p.is_file()})
        self.assertTrue(scan.scan(self.sys,RAW)['snapshot_only'])
    def test_missing_inventory_refused(self):
        with self.assertRaises(ValueError):scan.scan(Path(self.tmp.name)/'missing',RAW)
    def test_source_has_no_effectors(self):
        tree=ast.parse((HERE/'scan.py').read_text())
        forbidden={'open','write','write_text','write_bytes','mkdir','unlink','rename','replace','system','popen','run','Popen','ioctl','tcsetattr','socket'}
        self.assertFalse(any(isinstance(n,ast.Call) and ((isinstance(n.func,ast.Name) and n.func.id in forbidden) or (isinstance(n.func,ast.Attribute) and n.func.attr in forbidden)) for n in ast.walk(tree)))
        imports={n.names[0].name for n in ast.walk(tree) if isinstance(n,ast.Import)}
        self.assertFalse(imports & {'os','subprocess','socket','termios','fcntl'})
    def test_generated_and_dispatch_refusal(self):
        subprocess.run(['python3',str(HERE/'generate.py'),'--check'],check=True)
        installer=ROOT/'Component/brrdfeeder/install/brrdfeeder-install.sh'
        for command in ('add','remove','replace'):
            result=subprocess.run(['bash',str(installer),'devices',command],capture_output=True,text=True)
            self.assertEqual(result.returncode,2);self.assertIn('not implemented',result.stderr);self.assertNotIn('enrollment',result.stdout)
        result=subprocess.run(['bash',str(installer),'devices','--help'],capture_output=True,text=True)
        self.assertEqual(result.returncode,0);self.assertIn('scan',result.stdout)
    def test_runtime_allowlist_parity(self):
        profiles={e['id']:e for e in json.loads(RAW)['profiles']}
        text=(ROOT/'Component/brrdfeeder/install/gps-runtime.py').read_text()
        self.assertIn("HOTPLUG_ID = '"+profiles['cp210x-nmea']['usb_ids'][0]+"'",text)
        ble=(ROOT/'Component/brrdfeeder/install/bluetooth-state.py').read_text()
        for identity in profiles['realtek-hci']['usb_ids']:self.assertIn("'"+identity+"'",ble)
        for identity in profiles['ublox-usb']['usb_ids']:self.assertIn("'"+identity.split(':')[1]+"'",text)

if __name__=='__main__':unittest.main(verbosity=2)
