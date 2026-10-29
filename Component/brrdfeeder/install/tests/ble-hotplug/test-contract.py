#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Real helper/selection with fake sysfs, systemctl and HCI; no host effects."""
import importlib.util
import errno
import json
from pathlib import Path
import re
import unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('prior',Path(__file__).resolve().parents[1]/'pi-native-p0/test-ble.py')
prior=importlib.util.module_from_spec(spec); spec.loader.exec_module(prior)
ble=prior.ble
INSTALL=prior.INSTALL
ROOT=prior.ROOT

class Hotplug(unittest.TestCase):
    explicit=prior.BLE.explicit
    def setUp(self):
        prior.BLE.setUp(self)
        self.hci=self.root/'hci'; self.hci.mkdir()
        self.runtime=self.root/'runtime'
        for p in [patch.object(ble,'HCI',self.hci),patch.object(ble,'DEVICES',self.usb),
                  patch.object(ble,'RUNTIME',self.runtime),patch.object(ble,'CONFLICT',self.runtime/'conflict')]:
            p.start(); self.addCleanup(p.stop)
    def adapter(self,index=0,identity='0bda:876e'):
        prior.BLE.adapter(self,str(index),identity)
        h=self.hci/f'hci{index}'; h.mkdir(); (h/'device').symlink_to(self.usb/str(index))
        (h/'address').write_text(f'AA:BB:CC:DD:EE:{index:02X}')
    def test_absent_auto_then_add_preserves_bytes_and_engine(self):
        original=self.config.read_bytes(); ble.apply(hotplug=True)
        self.assertFalse(self.receipt.exists()); self.assertFalse(self.systemd.calls)
        self.adapter(); ble.apply(hotplug=True)
        self.assertEqual(self.config.read_bytes(),original)
        self.assertEqual(self.systemd.engine,'active')
        self.assertNotIn(('stop',ble.ENGINE),self.systemd.calls)
        self.assertEqual((self.systemd.enabled,self.systemd.active),('masked',False))
        ble.controller_down.assert_called_once_with({},hotplug=True)
        receipt=self.receipt.read_bytes(); self.systemd.calls=[]; ble.apply(hotplug=True)
        self.assertEqual(self.receipt.read_bytes(),receipt)
        self.assertTrue(all(c[0]=='show' for c in self.systemd.calls))
    def test_order_receipt_before_stop_mask_before_down(self):
        self.adapter(); calls=[]; atomic=ble.atomic; ctl=self.systemd
        def write(path,*args): calls.append(('write',path)); atomic(path,*args)
        def control(*args): calls.append(args); return ctl(*args)
        with patch.object(ble,'atomic',write),patch.object(ble,'ctl',control),patch.object(ble,'controller_down',side_effect=lambda *a,**k:calls.append(('down',))):
            ble.apply(hotplug=True)
        self.assertLess(calls.index(('write',self.receipt)),calls.index(('stop',ble.UNIT)))
        self.assertLess(calls.index(('mask',ble.UNIT)),calls.index(('down',)))
    def test_off_never_scans_or_claims_across_replug(self):
        self.explicit({'enabled':False}); self.adapter()
        with patch.object(ble,'selected_controller',side_effect=AssertionError('must not select')):
            for _ in range(2): ble.apply(hotplug=True)
        self.assertFalse(self.systemd.calls); self.assertFalse(self.receipt.exists())
    def test_off_restores_prior_and_restarts_existing_engine(self):
        self.adapter(); ble.apply(hotplug=True); self.explicit({'enabled':False})
        before=self.config.read_bytes(); ble.apply(hotplug=True)
        self.assertFalse(self.receipt.exists()); self.assertEqual(self.config.read_bytes(),before)
        self.assertEqual((self.systemd.enabled,self.systemd.active),('enabled',True))
        self.assertEqual(self.systemd.engine,'active')
        self.assertLess(self.systemd.calls.index(('stop',ble.ENGINE)),self.systemd.calls.index(('unmask',ble.UNIT)))
    def test_unsupported_ambiguous_and_pinned_absence_refuse(self):
        self.adapter(identity='1234:5678'); ble.apply(hotplug=True)
        self.assertIn('unsupported Bluetooth adapter 1234:5678',self.log.getvalue())
        self.assertFalse(self.systemd.calls)
        self.adapter(1); self.adapter(2,'0bda:a728')
        with self.assertRaisesRegex(ValueError,'exactly one'): ble.apply(hotplug=True)
        self.assertFalse(self.receipt.exists()); ble.controller_down.assert_not_called()
        self.explicit({'enabled':True,'adapter':{'usb_id':'0bda:ffff'}})
        with self.assertRaisesRegex(ValueError,'exactly one'): ble.apply(hotplug=True)
        self.assertFalse(self.receipt.exists())
    def test_manual_unmask_conflict_never_remasks_even_absent(self):
        self.adapter(); ble.apply(hotplug=True); self.systemd.enabled='disabled'; self.systemd.calls=[]
        with self.assertRaisesRegex(ValueError,'no automatic re-mask'): ble.apply(hotplug=True)
        self.assertTrue(ble.CONFLICT.exists()); self.assertTrue(all(c[0]=='show' for c in self.systemd.calls))
        self.systemd.enabled='masked'; self.systemd.calls=[]
        with self.assertRaisesRegex(ValueError,'no automatic re-mask'): ble.apply(hotplug=True)
        self.assertTrue(all(c[0]=='show' for c in self.systemd.calls))
    def test_bad_receipt_and_failed_mask_preserve_recovery(self):
        self.adapter(); self.receipt.write_text('{}')
        with self.assertRaises(ValueError): ble.apply(hotplug=True)
        self.assertFalse(self.systemd.calls); self.receipt.unlink()
        self.systemd.fail=('mask',ble.UNIT)
        with self.assertRaises(ValueError): ble.apply(hotplug=True)
        self.assertEqual(json.loads(self.receipt.read_bytes())['enabled'],'enabled')
        self.assertEqual(self.systemd.engine,'active')
        self.systemd.fail=None
        with self.assertRaisesRegex(ValueError,'no automatic re-mask'): ble.apply(hotplug=True)
    def test_config_changed_before_claim_refuses(self):
        self.adapter(); real=ble.selected_controller
        def select(a):
            result=real(a); self.config.write_text('sensors: {rid_ble: {enabled: false}}'); return result
        with patch.object(ble,'selected_controller',select):
            with self.assertRaisesRegex(ValueError,'changed before'): ble.apply(hotplug=True)
        self.assertFalse(self.receipt.exists()); self.assertTrue(all(c[0]=='show' for c in self.systemd.calls))
    def test_main_dry_run_creates_no_runtime_or_receipt(self):
        self.adapter()
        for args in (['apply','--hotplug','--dry-run'], ['restore','--dry-run']):
            with patch.object(ble.os,'geteuid',return_value=0), patch.object(ble.sys,'argv',['helper',*args]):
                ble.main()
            self.assertFalse(self.runtime.exists()); self.assertFalse(self.receipt.exists())
        ble.apply(hotplug=True)
        before=self.receipt.read_bytes(); self.systemd.calls=[]
        with patch.object(ble.os,'geteuid',return_value=0),patch.object(ble.sys,'argv',['helper','restore','--dry-run']):
            ble.main()
        self.assertFalse(self.runtime.exists()); self.assertEqual(self.receipt.read_bytes(),before)
        self.assertFalse(self.systemd.calls)
    def test_off_restores_while_engine_waits_for_gps(self):
        self.adapter(); ble.apply(hotplug=True); self.explicit({'enabled':False})
        self.systemd.engine='activating'; ble.apply(hotplug=True)
        self.assertEqual(self.systemd.engine,'active'); self.assertFalse(self.receipt.exists())
    def test_dry_run_and_reason_summary(self):
        self.adapter(); before=self.config.read_bytes(); ble.apply(dry=True,hotplug=True)
        self.assertEqual(self.config.read_bytes(),before); self.assertFalse(self.receipt.exists())
        self.assertTrue(all(c[0]=='show' for c in self.systemd.calls))

class Ioctl(unittest.TestCase):
    setUp=prior.Controller.setUp
    add=prior.Controller.add
    operation=prior.Controller.operation
    def test_hotplug_busy_does_not_interrupt_owner(self):
        self.systemd.engine='active'; self.ioctl.side_effect=OSError(errno.EBUSY,'owned')
        ble.controller_down({},hotplug=True)
        self.assertEqual(self.ioctl.call_count,1)
        self.assertEqual(self.systemd.engine,'active')
    def test_address_snapshot_without_sysfs_address(self):
        path=self.hci/'hci7'; (path/'address').unlink()
        def read(fd,request,info,*rest):
            self.assertEqual(request,0x800448d3)
            info[10:16]=bytes([6,5,4,3,2,1])
        self.ioctl.side_effect=read
        self.assertEqual(ble.controller_address(path,7),'01:02:03:04:05:06')
        self.ioctl.assert_called_once()
    def test_hotplug_checks_daemon_before_socket(self):
        self.systemd.active=True
        with self.assertRaises(ValueError): ble.controller_down({},hotplug=True)
        self.sock.assert_not_called()

class LocalStatus(unittest.TestCase):
    def snapshot(self, enabled=True, age=0):
        stamp=ble.dt.datetime.fromtimestamp(10000-age,ble.dt.timezone.utc).isoformat()
        return dict(schema_version=1,status_interval_secs=30,written_at=stamp,
                    heartbeat={'node_id':'fixture'}, inventory={'rid_ble':{
                        'state':'degraded','enabled':enabled,'reason':'Waiting: unsupported 1234:5678; recommended 0bda:876e / 0bda:a728'}})
    def test_health_summary_prefers_local_reason_and_explicit_off(self):
        for enabled in (True,False):
            text=ble.summarize_health(self.snapshot(enabled),'[rid_ble] state=Healthy detail=None',10000)
            self.assertIn('Degraded' if enabled else 'Disabled',text)
            self.assertIn('1234:5678',text); self.assertNotIn('Healthy',text)
        self.assertIn('Unknown',ble.summarize_health(self.snapshot(age=100),'',10000))
    def test_status_command_reason_and_explicit_off(self):
        spec=importlib.util.spec_from_file_location('install_log',INSTALL/'install-log.py')
        log=importlib.util.module_from_spec(spec); spec.loader.exec_module(log)
        unit='ActiveState=active\nSubState=running\nInvocationID='+'a'*32
        for enabled in (True,False):
            with patch.object(log,'capture',side_effect=[(0,unit,''),(0,'[rid_ble] state=Healthy detail=None',''),(0,unit,'')]), \
                 patch.object(log,'read_text',return_value=json.dumps(self.snapshot(enabled))), \
                 patch.object(log.time,'time',return_value=10000),patch.object(log.time,'monotonic',return_value=0):
                result=log.engine_snapshot({'started_at':9900,'node_id':'fixture'},10)
                if enabled: self.assertIn('1234:5678',result['states']['Bluetooth'])
                else: self.assertEqual(result['states']['Bluetooth'],'disabled in configuration')

class Wiring(unittest.TestCase):
    def test_udev_exact_allowlist_add_only(self):
        for path in [INSTALL/'brrdfeeder-install.sh',ROOT/'Component/aviary/deploy/udev/99-cybrrd-brrdfeeder.rules']:
            rules=[line for line in path.read_text().splitlines() if 'SYSTEMD_WANTS' in line and 'ble-takeover' in line]
            self.assertEqual(len(rules),2)
            ids=set()
            for line in rules:
                self.assertIn('ACTION=="add"',line); self.assertIn('SUBSYSTEM=="bluetooth"',line)
                v=re.search(r'ATTRS\{idVendor\}=="([0-9a-f]{4})"',line)
                p=re.search(r'ATTRS\{idProduct\}=="([0-9a-f]{4})"',line)
                self.assertIsNotNone(v); self.assertIsNotNone(p); ids.add(v[1]+':'+p[1])
                self.assertNotIn('RUN',line)
            self.assertEqual(ids,{'0bda:876e','0bda:a728'})
    def test_service_timer_mount_and_uninstall(self):
        installer=(INSTALL/'brrdfeeder-install.sh').read_text(); uninstall=(INSTALL/'uninstall.sh').read_text()
        for ext in ['service','timer']:
            body=(ROOT/f'Component/aviary/deploy/quadlet/brrdfeeder-ble-takeover.{ext}').read_text()
            self.assertIn(body,installer); self.assertIn(f'/etc/systemd/system/brrdfeeder-ble-takeover.{ext}',uninstall)
        self.assertIn('ExecStart=/usr/local/libexec/brrdfeeder-bluetooth apply --hotplug',installer)
        for source in [installer,(ROOT/'Component/aviary/deploy/quadlet/brrdfeeder-engine.container').read_text()]:
            self.assertIn('Volume=/run/brrdfeeder-bluetooth:/run/brrdfeeder-bluetooth:ro',source)
            self.assertIn('ExecStartPre=/usr/local/libexec/brrdfeeder-bluetooth prepare',source)
        self.assertLess(uninstall.index('stop_unit system brrdfeeder-ble-takeover.timer'),uninstall.index('act python3 /usr/local/libexec/brrdfeeder-bluetooth restore'))
        self.assertIn('for tree in /run/brrdfeeder-bluetooth',uninstall)
    def test_engine_has_no_config_writer_or_subprocess(self):
        source=(ROOT/'Component/aviary/engine/src/rid_ble.rs').read_text().split('#[cfg(test)]\nmod tests')[0]
        for forbidden in ['std::fs::write','OpenOptions','Command::new','File::create','std::fs::rename']:
            self.assertNotIn(forbidden,source)

if __name__=='__main__': unittest.main(verbosity=2)
