#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline Bluetooth ownership fixtures; never execute host systemctl or HCI."""
import contextlib
import importlib.util
import io
import json
import errno
import os
from pathlib import Path
import subprocess
import struct
import tempfile
import unittest
from unittest.mock import patch, sentinel
import yaml

ROOT=Path(__file__).resolve().parents[5]
INSTALL=ROOT/'Component/brrdfeeder/install'
spec=importlib.util.spec_from_file_location('ble',INSTALL/'bluetooth-state.py')
ble=importlib.util.module_from_spec(spec); spec.loader.exec_module(ble)


class Systemd:
    def __init__(self, enabled='enabled', active=True):
        self.enabled=enabled; self.active=active; self.calls=[]; self.fail=None
    def __call__(self,*args):
        self.calls.append(args)
        if self.fail and args==self.fail: raise ValueError('seeded systemctl failure')
        if args[0]=='show' and args[1]==ble.ENGINE:
            return 'loaded\n' if args[2]=='--property=LoadState' else 'inactive\n'
        if args[0]=='show':
            load='masked' if self.enabled.startswith('masked') else ('not-found' if self.enabled=='absent' else 'loaded')
            return f'LoadState={load}\nUnitFileState={self.enabled}\nActiveState={"active" if self.active else "inactive"}\n'
        runtime=args[0]=='--runtime'
        command=args[1] if runtime else args[0]
        if args[-1]==ble.ENGINE: return ''
        if command=='stop': self.active=False
        elif command=='start': self.active=True
        elif command=='disable': self.enabled='disabled'
        elif command=='enable': self.enabled='enabled-runtime' if runtime else 'enabled'
        elif command=='mask':
            if not self.enabled.startswith('masked'): self.under_mask=self.enabled
            self.enabled='masked-runtime' if runtime else 'masked'
        elif command=='unmask' and self.enabled.startswith('masked'):
            self.enabled=getattr(self,'under_mask','disabled')
        return ''


class BLE(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name); self.config=self.root/'config.yaml'; self.receipt=self.root/'.bluetooth-prior.json'
        self.usb=self.root/'usb'; self.usb.mkdir()
        # A real render of the shipped template; measured position below is a FIXTURE.
        text=(INSTALL/'brrdfeeder-install.sh').read_text()
        self.template=text.split("<<'CFGEOF'\n",1)[1].split('\nCFGEOF',1)[0]
        self.config.write_text(self.template.replace('EDIT-ME-wlanX','wlan1').replace('EDIT-ME-node','fixture'))
        self.config.chmod(0o644)
        self.systemd=Systemd()
        self.log=io.StringIO()
        for p in [patch.object(ble,'CONFIG',self.config),patch.object(ble,'RECEIPT',self.receipt),patch.object(ble,'USB',self.usb),
                  patch.object(ble,'controller_down'),
                  patch.object(ble,'ctl',self.systemd),patch.object(ble,'safe'),contextlib.redirect_stdout(self.log)]:
            p.__enter__(); self.addCleanup(p.__exit__,None,None,None)
        # Ownership checks are a separate adversarial test: this fixture is NOT root.
    def adapter(self,name='1-2',identity='0bda:876e'):
        d=self.usb/name; d.mkdir(); v,p=identity.split(':'); (d/'idVendor').write_text(v); (d/'idProduct').write_text(p)
    def explicit(self,value):
        doc=yaml.safe_load(self.config.read_bytes()); doc.setdefault('sensors',{})['rid_ble']=value
        self.config.write_text(yaml.safe_dump(doc,sort_keys=False))
    def test_rendered_config_present_absent_and_explicit(self):
        ble.apply(); self.assertFalse(self.receipt.exists()); self.assertFalse(self.systemd.calls)
        self.assertNotIn('rid_ble',yaml.safe_load(self.config.read_bytes())['sensors'])
        self.adapter(); ble.apply()
        doc=yaml.safe_load(self.config.read_bytes())
        self.assertIn('rid_ble',doc['sensors'], 'present supported adapter must get a BLE configuration')
        self.assertEqual(doc['sensors']['rid_ble'],{'enabled':True,'unblock_rfkill':True,'adapter':{'usb_id':'0bda:876e'}})
        self.assertEqual(self.systemd.enabled,'masked'); self.assertFalse(self.systemd.active)
        self.assertEqual(self.receipt.stat().st_mode&0o777,0o600)
        original=self.receipt.read_bytes(); ble.apply(); self.assertEqual(self.receipt.read_bytes(),original)
        out=os.environ.get('P0_EVIDENCE')
        if out:
            doc['node']['id']='fixture'; doc['node']['location']={'latitude':41.,'longitude':-87.,'elevation_meters':200.}
            Path(out,'ble-rendered-config.yaml').write_text(yaml.safe_dump(doc,sort_keys=False))
            Path(out,'ble-ownership.log').write_text(self.log.getvalue())
        ble.restore(); self.assertEqual((self.systemd.enabled,self.systemd.active),('enabled',True)); self.assertFalse(self.receipt.exists())
        self.explicit({'enabled':False}); before=self.config.read_bytes(); self.systemd.calls=[]
        ble.apply(); self.assertEqual(before,self.config.read_bytes()); self.assertFalse(self.systemd.calls)
    def test_explicit_identity_and_duplicate_adapter(self):
        self.adapter(); self.adapter('1-3')
        with self.assertRaisesRegex(ValueError,'multiple'): ble.apply()
        self.assertFalse(self.receipt.exists()); self.assertFalse(self.systemd.calls)
        self.explicit({'enabled':True,'unblock_rfkill':False,'adapter':{'bd_addr':'AA:BB:CC:DD:EE:FF'}})
        before=self.config.read_bytes(); ble.apply(); self.assertEqual(self.config.read_bytes(),before)
    def test_dry_run_no_mutation_and_no_receipt(self):
        self.adapter(); before=self.config.read_bytes(); ble.apply(dry=True)
        self.assertEqual(self.config.read_bytes(),before); self.assertFalse(self.receipt.exists())
        self.assertTrue(all(c[0]=='show' for c in self.systemd.calls))
        ble.apply(); before=self.receipt.read_bytes(); self.systemd.calls=[]; ble.restore(dry=True)
        self.assertEqual(self.receipt.read_bytes(),before); self.assertFalse(self.systemd.calls)
        self.assertIn('WOULD restore',self.log.getvalue())
    def test_failed_mask_receipt_survives_retry(self):
        self.adapter(); self.systemd.fail=('mask',ble.UNIT)
        with self.assertRaisesRegex(ValueError,'seeded'): ble.apply()
        prior=self.receipt.read_bytes(); self.assertTrue(json.loads(prior)['active'])
        self.systemd.fail=None; ble.apply(); self.assertEqual(self.receipt.read_bytes(),prior)
        self.systemd.fail=('start',ble.UNIT)
        with self.assertRaises(ValueError): ble.restore()
        self.assertTrue(self.receipt.exists())
        self.systemd.fail=None; ble.restore(); self.assertFalse(self.receipt.exists())
    def test_state_matrix_and_missing_receipt(self):
        self.adapter()
        for state in ble.STATES:
            for active in [False,True]:
                if state in ('absent','masked','masked-runtime') and active: continue
                with self.subTest(state=state,active=active):
                    self.systemd.enabled=state; self.systemd.active=active
                    # Already-masked state implies a loaded disabled vendor unit underneath.
                    self.systemd.under_mask='disabled'
                    try:
                        ble.apply()
                        try: ble.restore()
                        except ValueError as error: self.fail('valid saved Bluetooth state was not restored: '+str(error))
                        self.assertEqual((self.systemd.enabled,self.systemd.active),(state,active))
                    finally:
                        # Independent cases even when a deliberate restore mutant
                        # retains its fixture receipt. Never touches a real host.
                        self.receipt.unlink(missing_ok=True)
        self.systemd.calls=[]; ble.restore(); self.assertFalse(self.systemd.calls)
        self.assertIn('nothing to do',self.log.getvalue())
    def test_receipt_and_config_refuse_before_mutation(self):
        self.adapter()
        for value in [{}, {'version':1,'unit':'sshd.service','enabled':'enabled','active':True},
                      {'version':1,'unit':ble.UNIT,'enabled':'masked','active':True}]:
            self.receipt.write_text(json.dumps(value)); self.systemd.calls=[]
            with self.assertRaises(ValueError): ble.apply()
            self.assertFalse(self.systemd.calls)
        self.receipt.unlink()
        for value in [{'enabled':'true'}, {'enabled':True,'adapter':{'usb_id':'x; echo BAD'}},
                      {'enabled':True,'adapter':{'usb_id':'0bda:876e','bd_addr':'AA:BB:CC:DD:EE:FF'}},
                      {'enabled':True,'adapter':{'usb_id':'0bda:876e'},'quiet_window_s':0}, {'unknown':1}]:
            self.explicit(value); self.systemd.calls=[]
            with self.assertRaises(ValueError): ble.apply()
            self.assertFalse(self.systemd.calls)
    def test_order_and_embedding(self):
        self.adapter()
        calls=[]
        original_atomic=ble.atomic
        def atomic(path,*args): calls.append(('write',path)); original_atomic(path,*args)
        original_ctl=self.systemd
        def ctl(*args): calls.append(args); return original_ctl(*args)
        with patch.object(ble,'ctl',ctl),patch.object(ble,'atomic',atomic),patch.object(ble,'controller_down',side_effect=lambda _:calls.append(('DOWN',))): ble.apply()
        self.assertLess(calls.index(('stop',ble.ENGINE)),calls.index(('write',self.config)))
        self.assertLess(calls.index(('write',self.receipt)),calls.index(('stop',ble.UNIT)))
        self.assertLess(calls.index(('mask',ble.UNIT)),calls.index(('DOWN',)))
        install=(INSTALL/'brrdfeeder-install.sh').read_text(); uninstall=(INSTALL/'uninstall.sh').read_text()
        embedded=install.split("<<'BLUETOOTH_EOF'\n",1)[1].split('\nBLUETOOTH_EOF',1)[0]+'\n'
        self.assertEqual(embedded,(INSTALL/'bluetooth-state.py').read_text())
        self.assertLess(uninstall.index('stop_unit system brrdfeeder-engine.service'),uninstall.index('act python3 /usr/local/libexec/brrdfeeder-bluetooth restore'))
        self.assertLess(uninstall.index('brrdfeeder-bluetooth restore'),uninstall.index('for file in /etc/systemd/system/brrdfeeder-updater.path'))


class Safety(unittest.TestCase):
    def test_restore_helper_permissions_guard_executes(self):
        # Execute the shipped validation loop with inert stat/diagnostic functions.
        source=(INSTALL/'uninstall.sh').read_text()
        guard=source.split('  for helper_path in ',1)[1].split('\n  python3 /usr/local/libexec/brrdfeeder-bluetooth check',1)[0]
        script='''set -euo pipefail
die() { echo "$*"; exit 3; }
stat() { if [[ $2 == %u ]]; then echo "$FIXTURE_UID"; else echo "$FIXTURE_MODE"; fi; }
for helper_path in '''+guard+'\n'
        for uid,mode,rc in [(0,'755',0),(1000,'755',3),(0,'777',3),(0,'775',3),(0,'700',0)]:
            result=subprocess.run(['bash','-c',script],env=dict(os.environ,FIXTURE_UID=str(uid),FIXTURE_MODE=mode),capture_output=True,text=True)
            self.assertEqual(result.returncode,rc,result.stderr+result.stdout)
    def test_real_path_validation_refuses_symlinks_and_nonroot(self):
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp)/'state'; p.write_text('{}'); link=Path(tmp)/'link'; link.symlink_to(p)
            for path in [p,link]:
                with self.assertRaises(ValueError): ble.safe(path,0o600)
    def test_unknown_service_state_refuses(self):
        with patch.object(ble,'ctl',return_value='LoadState=loaded\nUnitFileState=alias\nActiveState=active\n'):
            with self.assertRaises(ValueError): ble.service_state()


class Controller(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        root=Path(self.tmp.name); self.devices=root/'devices'; self.devices.mkdir()
        self.hci=root/'bluetooth'; self.hci.mkdir(); self.calls=[]
        self.systemd=Systemd('masked',False)
        for p in [patch.object(ble,'HCI',self.hci),patch.object(ble,'DEVICES',self.devices),patch.object(ble,'ctl',self.systemd)]:
            p.__enter__(); self.addCleanup(p.__exit__,None,None,None)
        self.sock=patch.object(ble.socket,'socket').start(); self.addCleanup(patch.stopall)
        # The socket itself is mocked; some interpreter builds also omit these
        # constants. Sentinels prove the helper passes them through unchanged.
        for name in ('AF_BLUETOOTH','BTPROTO_HCI'):
            p=patch.object(ble.socket,name,getattr(sentinel,name),create=True)
            p.start(); self.addCleanup(p.stop)
        self.ioctl=patch.object(ble.fcntl,'ioctl',side_effect=self.operation).start()
        self.sock.return_value.__enter__.return_value.fileno.return_value=99
        self.add('hci7')
    def add(self,name,usb='0bda:876e'):
        device=self.devices/name; device.mkdir(); v,p=usb.split(':')
        (device/'idVendor').write_text(v); (device/'idProduct').write_text(p)
        hci=self.hci/name; hci.mkdir(); (hci/'device').symlink_to(device)
        (hci/'address').write_text('AA:BB:CC:DD:EE:FF')
    def operation(self,fd,request,arg,*rest):
        self.calls.append((fd,request))
        if request==0x400448ca: self.assertEqual(arg,7)
        elif request==0x800448d3:
            self.assertEqual(len(arg),92); self.assertEqual(struct.unpack_from('@H',arg)[0],7)
            struct.pack_into('@I',arg,16,0)
        else: self.fail('unapproved ioctl')
        return 0
    def test_only_targeted_down_then_flags_read(self):
        self.add('hci0','1234:5678')
        ble.controller_down({'usb_id':'0bda:876e'})
        self.sock.assert_called_once_with(sentinel.AF_BLUETOOTH,
                                         ble.socket.SOCK_RAW | ble.socket.SOCK_CLOEXEC,
                                         sentinel.BTPROTO_HCI)
        self.assertEqual(self.calls,[(99,0x400448ca),(99,0x800448d3)])
    def test_bad_matches_never_open_socket(self):
        for adapter in [{'usb_id':'1234:5678'},{'bd_addr':'00:11:22:33:44:55'}]:
            with self.assertRaisesRegex(ValueError,'exactly one'): ble.controller_down(adapter)
        self.add('hci8')
        with self.assertRaisesRegex(ValueError,'exactly one'): ble.controller_down({'usb_id':'0bda:876e'})
        self.sock.assert_not_called()
    def test_live_owners_never_open_socket(self):
        self.systemd.active=True
        with self.assertRaisesRegex(ValueError,'inactive'): ble.controller_down({'usb_id':'0bda:876e'})
        self.systemd.active=False; self.systemd.enabled='enabled'
        with self.assertRaisesRegex(ValueError,'inactive'): ble.controller_down({'usb_id':'0bda:876e'})
        self.systemd.enabled='masked'
        old=self.systemd
        def ctl(*args):
            if args==('show',ble.ENGINE,'--property=ActiveState','--value'): return 'active\n'
            return old(*args)
        with patch.object(ble,'ctl',ctl):
            with self.assertRaisesRegex(ValueError,'inactive'): ble.controller_down({'usb_id':'0bda:876e'})
        self.sock.assert_not_called()
    def test_still_up_read_failure_and_identity_change_refuse(self):
        original=self.operation
        def up(fd,req,arg,*rest):
            original(fd,req,arg,*rest)
            if req==0x800448d3: struct.pack_into('@I',arg,16,1)
        self.ioctl.side_effect=up
        with self.assertRaisesRegex(ValueError,'verification failed'): ble.controller_down({'usb_id':'0bda:876e'})
        self.ioctl.side_effect=OSError(errno.EPERM,'seeded')
        with self.assertRaisesRegex(ValueError,'errno=1'): ble.controller_down({'usb_id':'0bda:876e'})
        self.ioctl.reset_mock()
        with patch.object(ble,'selected_controller',side_effect=[(7,'first',1),(8,'changed',2)]):
            with self.assertRaisesRegex(ValueError,'changed before'): ble.controller_down({'usb_id':'0bda:876e'})
        self.ioctl.assert_not_called()
    def test_already_down_still_requires_read_verification(self):
        def already(fd,req,arg,*rest):
            if req==0x400448ca: raise OSError(errno.EALREADY,'already down')
            return self.operation(fd,req,arg,*rest)
        self.ioctl.side_effect=already
        ble.controller_down({'bd_addr':'AA:BB:CC:DD:EE:FF'})
        self.assertEqual(self.calls,[(99,0x800448d3)])


if __name__=='__main__': unittest.main(verbosity=2)
