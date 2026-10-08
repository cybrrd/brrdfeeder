#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""No privileges/network: redaction and supervisor contract with bounded stubs."""
import contextlib
import grp
import importlib.util
import io
import os
import re
from pathlib import Path
import subprocess
import tempfile
import tarfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

ROOT=Path(__file__).resolve().parents[5]
INSTALL=ROOT/'Component/brrdfeeder/install'
spec=importlib.util.spec_from_file_location('install_log', INSTALL/'install-log.py')
log=importlib.util.module_from_spec(spec)
spec.loader.exec_module(log)

class Contract(unittest.TestCase):
    def test_single_update_status_uses_approved_uninstall_reinstall_and_relink_wording(self):
        self.assertEqual(log.SELF_UPDATE_STATUS,
            'Self-Update is installed but not yet active — this version does not update itself. '
            'To move to a newer release today: sudo brrdfeeder uninstall, then run the install command again '
            '(you will link the sensor to your account again).')

    def test_new_log_does_not_persist_legacy_wireless_output(self):
        with tempfile.TemporaryDirectory() as directory:
            script=Path(directory)/'fixture.sh'; script.write_text('exit 0\n')
            private='PRIVATE-HOME-SSID-DO-NOT-RECORD'
            raw='wireless_devices= rc=0 dur=0.1s\n\tInterface wlan0\n\t\tssid '+private+'\npower=unavailable\n'
            with patch.object(log,'LOG_DIR',Path(directory)/'logs'), patch.object(log,'environment',return_value=raw), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(log.supervise(str(script),[]),0)
            saved=next((Path(directory)/'logs').glob('install-*.log')).read_text()
            self.assertNotIn(private,saved)
            self.assertNotIn('Interface wlan0',saved)
            self.assertIn('result=OK',saved)

    def test_only_configured_usb_capture_adapter_is_queried(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); sysnet=root/'net'; sysnet.mkdir()
            usb=root/'usb'; usb.mkdir(); (usb/'idVendor').write_text('0e8d'); (usb/'idProduct').write_text('7612')
            builtin=root/'sdio'; builtin.mkdir()
            for name,device in [('wlan1',usb),('wlan0',builtin)]:
                (sysnet/name).mkdir(); (sysnet/name/'device').symlink_to(device)
            real=log.read_text
            for iface in ['wlan1','wlan0']:
                def read(path,*args,**kwargs):
                    if str(path)=='/etc/brrdfeeder/config.yaml': return 'capture:\n  interface: '+iface+'\n'
                    if Path(path).is_relative_to(root): return real(path,*args,**kwargs)
                    return 'fixture'
                with patch.object(log,'SYS_NET',sysnet), patch.object(log,'read_text',side_effect=read), \
                     patch.object(log,'capture',return_value=(0,'fixture',0)) as capture, \
                     patch.object(log,'power_check',return_value=('unavailable','','')):
                    log.environment()
                wireless=[c.args[0] for c in capture.call_args_list if c.args[0][0]=='iw']
                self.assertEqual(wireless,[['iw','dev','wlan1','info']] if iface=='wlan1' else [])

    def test_wireless_environment_never_collects_unscoped_interfaces(self):
        private='PRIVATE-HOME-SSID-DO-NOT-RECORD'
        commands=[]
        def capture(args, **kwargs):
            commands.append(args)
            if args==['iw','dev']:
                return 0,'phy#0\n\tInterface wlan0\n\t\taddr aa:bb:cc:dd:ee:ff\n\t\tssid '+private+'\n',0
            return 0,'fixture',0
        with patch.object(log,'capture',side_effect=capture), patch.object(log,'read_text',return_value='fixture'), patch.object(log,'power_check',return_value=('unavailable','','')):
            raw=log.environment()
        self.assertNotIn(private,raw)
        for command in [['iw','dev'],['iw','phy'],['ip','-brief','address'],['ip','route']]:
            self.assertNotIn(command,commands)

    def test_legacy_wifi_private_values_never_enter_support_bundle(self):
        private='PRIVATE-HOME-SSID-DO-NOT-RECORD'
        legacy='mode=uninstall\nwireless_devices= rc=0 dur=0.1s\nphy#0\n\tInterface wlan0\n\t\taddr aa:bb:cc:dd:ee:ff\n\t\tssid '+private+'\n\t\tchannel 6 (2437 MHz)\npower=unavailable\nresult=OK\n'
        safe=log.Redactor().text(legacy)
        self.assertNotIn(private,safe)
        self.assertNotIn('aa:bb:cc:dd:ee:ff',safe)
        self.assertNotIn('channel 6',safe)
        self.assertIn('result=OK',safe)
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); (root/'uninstall-fixture.log').write_text(legacy)
            actual=log.read_text
            def read(path,*args,**kwargs):
                if Path(path).is_relative_to(root): return actual(path,*args,**kwargs)
                raise FileNotFoundError(str(path))
            output=io.StringIO()
            with patch.object(log,'LOG_DIR',root), patch.object(log,'read_text',side_effect=read), \
                 patch.object(log,'environment',return_value='fixture'), patch.object(log,'reachability',return_value='fixture'), \
                 patch.object(log,'power_check',return_value=('unavailable','','')), patch.object(log,'capture',return_value=(1,'fixture',0)), \
                 patch.object(log.pwd,'getpwnam',side_effect=KeyError), contextlib.redirect_stdout(output):
                self.assertEqual(log.bundle('11223344',log.Redactor()),0)
            archive_path=Path(re.search(r'Support bundle: (\S+)',output.getvalue())[1])
            try:
                with tarfile.open(archive_path) as archive:
                    for member in archive.getmembers():
                        content=archive.extractfile(member).read().decode()
                        self.assertNotIn(private,content)
                        self.assertNotIn('aa:bb:cc:dd:ee:ff',content)
            finally: archive_path.unlink()

    def test_ubuntu_syslog_parent_is_accepted_without_relaxing_other_paths(self):
        original_stat = Path.stat
        def check(mode, uid=0, gid=104, path='/var/log'):
            def info(item, *args, **kwargs):
                if str(item) == path:
                    return SimpleNamespace(st_mode=mode, st_uid=uid, st_gid=gid)
                if str(item) in ('/', '/var', '/var/log'):
                    return SimpleNamespace(st_mode=0o40755, st_uid=0, st_gid=0)
                return original_stat(item, *args, **kwargs)
            with patch.object(Path, 'stat', info), patch.object(Path, 'mkdir'), \
                 patch.object(os, 'geteuid', return_value=0), \
                 patch.object(grp, 'getgrnam', return_value=SimpleNamespace(gr_gid=104)):
                log.Log.safe_parent(Path(path)/'fixture-log-parent')
        check(0o40775)
        for kwargs in ({'mode':0o40777}, {'mode':0o40775, 'uid':1000},
                       {'mode':0o40775, 'gid':1000}, {'mode':0o40775, 'path':'/var'}):
            with self.subTest(kwargs=kwargs), self.assertRaises(OSError):
                check(**kwargs)

    def test_embedded(self):
        source=(INSTALL/'brrdfeeder-install.sh').read_text()
        embedded=source.split("<<'INSTALL_LOG_PY'\n",1)[1].split('\nINSTALL_LOG_PY\n',1)[0]+'\n'
        self.assertEqual(embedded,(INSTALL/'install-log.py').read_text())
        self.assertIn((INSTALL/'log-events.sh').read_text().strip(),source)

    def test_redaction_positive_and_negative(self):
        r=log.Redactor()
        seeded='user_code=FAKE-CODE\n'+ 'eyJhbGciOiJIUzI1NiJ9.eyJzZWVkIjoidGVzdCJ9.signature'+'\n-----BEGIN NATS USER JWT-----\nCREDENTIAL-SEED\n-----END NATS USER JWT-----\n'
        value=r.text(seeded)
        for secret in ['FAKE-CODE','eyJhbGci','CREDENTIAL-SEED']:
            self.assertNotIn(secret,value)
        for kind in ['device-code','secret','nats-creds']:
            self.assertIn('[REDACTED:'+kind+']',value)
        ordinary='node_id=public-node wlan1 192.0.2.52 1546:01a7 41.5,-87.5 sha256:'+'a'*64
        self.assertEqual(r.text(ordinary),ordinary)
        self.assertNotIn('[REDACTED:',r.text(ordinary))
        self.assertEqual(r.text('\x1b[31mhello\x1b[0m\x1b]0;title\x07'),'hello')
        self.assertIn('[REDACTED:token]',r.text('Authorization: Bearer token-value'))
        self.assertNotIn('a'*40,r.text('password: '+ 'a'*40))

    def test_multiline_detail_is_log_only_and_redacted(self):
        with tempfile.TemporaryDirectory() as directory:
            script=Path(directory)/'fixture.sh'
            script.write_text('set -eu\nsource '+str(INSTALL/'log-events.sh')+'\n'
                              'log_secret device-code FIXTURE-CODE\n'
                              "log_event DETAIL $'first\\nsecond FIXTURE-CODE'\n")
            output=io.StringIO()
            with patch.object(log,'LOG_DIR',Path(directory)/'logs'), patch.object(log,'environment',return_value='fixture=true'), contextlib.redirect_stdout(output):
                self.assertEqual(log.supervise(str(script),[]),0)
            text=next((Path(directory)/'logs').glob('install-*.log')).read_text()
            self.assertIn('first\nsecond [REDACTED:device-code]',text)
            self.assertNotIn('first',output.getvalue())
            self.assertNotIn('second',output.getvalue())
            self.assertNotIn('FIXTURE-CODE',text)

    def test_forced_failure_last_40_and_terminal(self):
        with tempfile.TemporaryDirectory() as directory:
            script=Path(directory)/'fixture.sh'
            script.write_text('set -eu\nsource '+str(INSTALL/'log-events.sh')+'\n'
                              'fail() { for i in {1..50}; do echo "line-$i"; done; echo user_code=FAKE-CODE; return 125; }\n'
                              'run_step images/pull-engine fail\n')
            output=io.StringIO()
            with patch.object(log,'LOG_DIR',Path(directory)/'logs'), patch.object(log,'environment',return_value='fixture=true'), \
                 patch.dict(os.environ,{'BRRDFEEDER_RUN_ID':'aabbccdd'}), contextlib.redirect_stdout(output):
                rc=log.supervise(str(script),['--no-verbose'])
            self.assertEqual(rc,125)
            path=next((Path(directory)/'logs').glob('install-*.log'))
            text=path.read_text()
            # In explicit quiet mode detail stays in the file; only the explicit
            # enrollment phase relays device-flow display to the terminal.
            self.assertNotIn('FAKE-CODE',output.getvalue())
            self.assertNotIn('FAKE-CODE',text)
            self.assertIn('[CMD]   step=images/pull-engine fail  rc=125 dur=',text)
            result=text.split('==== RESULT ====\n')[1]
            self.assertIn('failed_step=images/pull-engine',result)
            lines=result.split('last_output=\n')[1].split('\nlog=')[0].splitlines()
            self.assertEqual(len(lines),40)
            self.assertEqual(lines[0],'    line-12')
            self.assertEqual(lines[-1],'    user_code=[REDACTED:device-code]')
            self.assertEqual(path.stat().st_mode & 0o777,0o640)
            self.assertIn('run aabbccdd',output.getvalue().splitlines()[0])
            self.assertTrue(output.getvalue().splitlines()[-1].endswith('(run aabbccdd)'))

    def test_dryrun_only_log_no_latest(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory)/'logs'
            with patch.object(log,'LOG_DIR',folder):
                item=log.Log('dryrun','aabbccdd')
                item.write('safe\n'); os.close(item.fd)
            self.assertEqual([p.name for p in folder.iterdir()],[item.path.name])

    def test_bundle_cannot_override_dryrun(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(log,'LOG_DIR',Path(directory)/'logs'), patch.object(log,'environment',return_value='fixture=true'), \
                 patch.object(log,'bundle') as archive, contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(log.supervise(str(INSTALL/'brrdfeeder-install.sh'),['--support-bundle','--dry-run']),2)
                archive.assert_not_called()
            files=list((Path(directory)/'logs').iterdir())
            self.assertEqual(len(files),1)
            self.assertTrue(files[0].name.startswith('dryrun-'))

    def test_handled_command_failure_does_not_mislabel_later_refusal(self):
        with tempfile.TemporaryDirectory() as directory:
            script=Path(directory)/'fixture.sh'
            script.write_text('set -eu\nsource '+str(INSTALL/'log-events.sh')+'\n'
                              'run_step services/probe bash -c "exit 2" || echo recovered\n'
                              'echo FATAL configuration-refusal\nexit 1\n')
            with patch.object(log,'LOG_DIR',Path(directory)/'logs'), patch.object(log,'environment',return_value='fixture=true'), contextlib.redirect_stdout(io.StringIO()):
                rc=log.supervise(str(script),[])
            self.assertEqual(rc,1)
            text=next((Path(directory)/'logs').glob('install-*.log')).read_text()
            self.assertIn('failed_step=services/check\n',text)

    def test_capture_timeout(self):
        rc,text,duration=log.capture(['bash','-c','sleep 10'],timeout=.1)
        self.assertEqual(rc,124)
        self.assertLess(duration,2)
        self.assertIn('[timeout]',text)

    def test_log_fallback_is_explicit(self):
        output=io.StringIO()
        with patch.object(log.Log,'safe_parent',side_effect=OSError('read-only filesystem')), contextlib.redirect_stdout(output):
            item=log.Log('verify','11223344')
        try:
            item.write('redacted diagnostic\n'); os.close(item.fd)
            self.assertTrue(str(item.path).startswith('/tmp/brrdfeeder-verify-11223344-'))
            self.assertEqual(output.getvalue().count('logging degraded'),1)
            self.assertIn('read-only filesystem', output.getvalue())
            self.assertIn('read-only filesystem', item.path.read_text())
            self.assertTrue(item.path.read_text().endswith('redacted diagnostic\n'))
        finally:
            item.path.unlink()

    def test_large_bundle_is_capped_and_declares_missing(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory)
            for i in range(11):
                (folder/f'install-{i}.log').write_text('mode=install\n'+os.urandom(100000).hex()+'\nresult=FAILED\nfailed_step=images/pull-engine\n')
            output=io.StringIO()
            real_read=log.read_text
            def fixture_read(path,*args,**kwargs):
                if Path(path).is_relative_to(folder): return real_read(path,*args,**kwargs)
                raise FileNotFoundError(str(path))
            with patch.object(log,'LOG_DIR',folder), patch.object(log,'environment',return_value='fixture=true'), \
                 patch.object(log,'read_text',side_effect=fixture_read), \
                 patch.object(log,'capture',return_value=(1,os.urandom(100000).hex(),.01)), \
                 patch.object(log,'reachability',return_value='four fixture endpoints unreachable'), \
                 patch.object(log.pwd,'getpwnam',side_effect=KeyError), contextlib.redirect_stdout(output):
                self.assertEqual(log.bundle('11223344',log.Redactor()),0)
            path=Path(re.search(r'Support bundle: (\S+)',output.getvalue())[1])
            try:
                self.assertLess(path.stat().st_size,2_000_000)
                with tarfile.open(path) as archive:
                    manifest=archive.extractfile('bundle/MANIFEST.txt').read().decode()
                    self.assertIn('newest 10',manifest)
                    self.assertIn('truncated:',manifest)
                    self.assertIn('absent: /etc/brrdfeeder/config.yaml',manifest)
            finally:
                path.unlink()

    def test_redacted_spool_failure_does_not_abort_install(self):
        class FullDisk:
            def write(self,value):
                self_seen.append(value)
                raise OSError('No space left on device')
            def close(self): pass
        self_seen=[]
        with tempfile.TemporaryDirectory() as directory:
            script=Path(directory)/'fixture.sh'
            script.write_text('set -eu\nsource '+str(INSTALL/'log-events.sh')+'\n'
                              'run_step images/probe echo eyJhbGciOiJIUzI1NiJ9.payload.signature\n')
            with patch.object(log,'LOG_DIR',Path(directory)/'logs'), patch.object(log,'environment',return_value='fixture=true'), \
                 patch.object(log.tempfile,'SpooledTemporaryFile',return_value=FullDisk()), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(log.supervise(str(script),[]),0)
            self.assertIn('[REDACTED:secret]',self_seen[0])
            self.assertNotIn('eyJhbGci',self_seen[0])
            text=next((Path(directory)/'logs').glob('install-*.log')).read_text()
            self.assertIn('logging degraded:',text)
            self.assertIn('result=OK\n',text)

    def test_bundle_redacts_current_sources_not_only_saved_logs(self):
        raw='user_code=BUNDLE-ONLY-CODE\neyJhbGciOiJIUzI1NiJ9.bundle.signature\n-----BEGIN NATS USER JWT-----\nBUNDLE-ONLY-CREDS\n-----END NATS USER JWT-----\n'
        with tempfile.TemporaryDirectory() as directory:
            def fixture_read(path,*args,**kwargs):
                if str(path)=='/etc/brrdfeeder/config.yaml': return raw
                raise FileNotFoundError(str(path))
            output=io.StringIO()
            with patch.object(log,'LOG_DIR',Path(directory)), patch.object(log,'read_text',side_effect=fixture_read), \
                 patch.object(log,'capture',return_value=(1,raw,.01)), patch.object(log,'environment',return_value=raw), \
                 patch.object(log,'reachability',return_value='unreachable'), patch.object(log.pwd,'getpwnam',side_effect=KeyError), contextlib.redirect_stdout(output):
                self.assertEqual(log.bundle('11223344',log.Redactor()),0)
            path=Path(re.search(r'Support bundle: (\S+)',output.getvalue())[1])
            try:
                with tarfile.open(path) as archive:
                    for name in ['bundle/config.yaml','bundle/engine-log.txt','bundle/environment.txt']:
                        text=archive.extractfile(name).read().decode()
                        for secret in ['BUNDLE-ONLY-CODE','eyJhbGci','BUNDLE-ONLY-CREDS']: self.assertNotIn(secret,text)
                        for kind in ['device-code','secret','nats-creds']: self.assertIn('[REDACTED:'+kind+']',text)
            finally:
                path.unlink()

if __name__=='__main__': unittest.main(verbosity=2)
