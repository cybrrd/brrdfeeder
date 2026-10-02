#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline BLE migration, truthful summary, engine rfkill and reboot fixtures."""
import contextlib
import datetime as dt
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[5]
INSTALL=ROOT/'Component/brrdfeeder/install'
spec=importlib.util.spec_from_file_location('ble',INSTALL/'bluetooth-state.py')
ble=importlib.util.module_from_spec(spec);spec.loader.exec_module(ble)

class Policy(unittest.TestCase):
    def test_new_and_rerun_missing_policy(self):
        for usb in ble.SUPPORTED:
            for sensors in [{},{'rid_ble':{'enabled':True,'adapter':{'usb_id':usb}}}]:
                enabled,changed=ble.enabled_config(sensors,[usb])
                self.assertTrue(enabled);self.assertTrue(changed)
                self.assertIs(sensors['rid_ble']['unblock_rfkill'],True)
                self.assertEqual(ble.enabled_config(sensors,[usb]),(True,False))
    def test_explicit_policies_win(self):
        for enabled in [False,True]:
            for unblock in [False,True]:
                for usb in ble.SUPPORTED:
                    sensors={'rid_ble':dict(enabled=enabled,unblock_rfkill=unblock,adapter={'usb_id':usb})}
                    before=json.dumps(sensors)
                    self.assertEqual(ble.enabled_config(sensors,[usb]),(enabled,False))
                    self.assertEqual(json.dumps(sensors),before)
        sensors={};self.assertEqual(ble.enabled_config(sensors,[]),(False,False));self.assertEqual(sensors,{})
    def test_engine_source_targets_and_reruns_each_start(self):
        source=(ROOT/'Component/aviary/engine/src/rid_ble.rs').read_text()
        self.assertLess(source.index('resolve_adapter(&self.0.adapter)'),source.index('crate::rfkill::prepare_controller(index, self.0.unblock_rfkill'))
        self.assertLess(source.index('crate::rfkill::prepare_controller'),source.index('HciSocket::open(index, 1)'))

class Summary(unittest.TestCase):
    def status(self,age=0,interval=30):
        return dict(schema_version=1,status_interval_secs=interval,
                    written_at=dt.datetime.fromtimestamp(10000-age,dt.timezone.utc).isoformat())
    def test_latest_engine_state_wins_with_reason(self):
        journal='[rid_ble] state=Healthy detail=None\n[rid_ble] state=Failed detail=Some("RF-kill errno 132")'
        text=ble.summarize_health(self.status(),journal,now=10000)
        self.assertIn('RID.BLE Failed:',text);self.assertIn('RF-kill errno 132',text);self.assertNotIn('Healthy',text)
        self.assertIn('Healthy',ble.summarize_health(self.status(),journal+'\n[rid_ble] state=Healthy detail=None',10000))
        self.assertIn('Degraded',ble.summarize_health(self.status(),'[rid_ble] state=Degraded detail=Some("no advertisers")',10000))
        self.assertIn('Disabled',ble.summarize_health(self.status(),'[rid_ble] disabled',10000))
    def test_freshness_scales_and_missing_is_unknown(self):
        journal='[rid_ble] state=Healthy detail=None'
        for status in [self.status(age=91),self.status(age=-1),self.status(interval=0),{},self.status(interval=True)]:
            self.assertIn('Unknown',ble.summarize_health(status,journal,10000))
        self.assertIn('Healthy',ble.summarize_health(self.status(age=90),journal,10000))
        self.assertIn('Healthy',ble.summarize_health(self.status(age=91,interval=31),journal,10000))
        self.assertIn('Unknown',ble.summarize_health(self.status(),'',10000))
    def test_detail_cannot_emit_ansi(self):
        text=ble.summarize_health(self.status(),'[rid_ble] state=Failed detail=\x1b[31mseeded',10000)
        self.assertNotIn('\x1b',text);self.assertIn('\\u001b',text)
    def test_real_collection_current_invocation_and_restart(self):
        with tempfile.TemporaryDirectory() as tmp:
            status=Path(tmp)/'status.json';status.write_text(json.dumps(self.status()))
            for changed in [False,True]:
                invocation='a'*32
                with patch.object(ble,'STATUS',status),patch.object(ble.time,'time',return_value=10000), \
                     patch.object(ble,'ctl',side_effect=['ActiveState=active\nInvocationID='+invocation,('b'*32 if changed else invocation)]), \
                     patch.object(ble.subprocess,'run',return_value=subprocess.CompletedProcess([],0,'[rid_ble] state=Failed detail=Some("RF-kill")','')) as run, \
                     contextlib.redirect_stdout(io.StringIO()) as out:
                    ble.health_summary()
                    self.assertIn('Unknown' if changed else 'Failed',out.getvalue())
                    self.assertIn('_SYSTEMD_INVOCATION_ID='+invocation,run.call_args.args[0])
    def test_wiring_inventory_and_persistent_limit_disclosed(self):
        source=(INSTALL/'brrdfeeder-install.sh').read_text()
        # Installation now waits for current-invocation BLE readiness in its
        # supervisor; the detailed standalone health summary remains available.
        self.assertIn("elif args == ['health-summary']: health_summary()",source)
        self.assertIn("'_SYSTEMD_INVOCATION_ID='+invocation",source)
        self.assertIn("'Bluetooth': 'Check the Bluetooth adapter",source)
        self.assertNotIn('sudo bash $0',source)
        helper=(INSTALL/'uninstall.sh').read_text()
        self.assertIn('Radio soft-block state is not restored',helper)
        self.assertNotIn('reinstall enrolls as a NEW node',helper)
        self.assertIn('same owner and hostname',helper)
        self.assertIn('[[ $HAVE_REALTEK -eq 1 ]] || say "Legacy Nordic',source)

class Rust(unittest.TestCase):
    def test_unchanged_rfkill_code_and_two_boots(self):
        # Standalone compile of the unmodified module, not an engine rebuild.
        # The only external dependency is libc::ERFKILL (Linux errno 132).
        original=(ROOT/'Component/aviary/engine/src/rfkill.rs').read_text()
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            extra=r'''
#[cfg(test)] mod repeated_start_fixture {
 use super::*;
 #[test] fn soft_block_reapplied_at_each_boot_is_target_unblocked() {
  let root=std::env::temp_dir().join(format!("rfkill-repeat-{}",std::process::id()));
  std::fs::create_dir_all(root.join("rfkill23")).unwrap();
  let entry=root.join("rfkill23"); let dev=root.with_extension("device");
  for (k,v) in [("name","hci4"),("type","bluetooth"),("hard","0")] {
   std::fs::write(entry.join(k),v).unwrap();
  }
  for _boot in 0..2 {
   std::fs::write(entry.join("soft"),"1").unwrap();
   std::fs::write(&dev,[]).unwrap();
   prepare_at(&root,&dev,4,true).unwrap();
   let mut expected=23u32.to_ne_bytes().to_vec(); expected.extend([2,2,0,0]);
   assert_eq!(std::fs::read(&dev).unwrap(),expected);
  }
  std::fs::remove_dir_all(root).unwrap();std::fs::remove_file(dev).unwrap();
 }
}
'''
            (root/'rfkill.rs').write_text(original+extra)
            (root/'fixture.rs').write_text('extern crate self as libc; pub const ERFKILL:i32=132;\n#[path="rfkill.rs"] mod rfkill;\n')
            subprocess.run(['rustc','--edition=2021','--test',str(root/'fixture.rs'),'-o',str(root/'test')],check=True,capture_output=True)
            result=subprocess.run([str(root/'test'),'--nocapture'],capture_output=True,text=True,check=True)
            self.assertIn('2 passed',result.stdout)
            print(result.stdout)

if __name__=='__main__': unittest.main(verbosity=2)
