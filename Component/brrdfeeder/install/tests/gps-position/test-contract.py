#!/usr/bin/env python3
"""Offline GPS contracts. Real service/serial lifecycle is separately exercised."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import yaml

ROOT=Path(__file__).resolve().parents[5]
INSTALL=ROOT/'Component/brrdfeeder/install'
SOURCE=(INSTALL/'brrdfeeder-install.sh').read_text()
spec=importlib.util.spec_from_file_location('gps',INSTALL/'gps-seed.py')
gps=importlib.util.module_from_spec(spec); spec.loader.exec_module(gps)

def sentence(body):
    checksum=0
    for char in body: checksum ^= ord(char)
    return f'${body}*{checksum:02X}\r\n'.encode()

GOOD='GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,'

class Contract(unittest.TestCase):
    def test_real_fix_and_rejections(self):
        value=gps.parse_fix(sentence(GOOD))
        self.assertAlmostEqual(value['latitude'],48.1173)
        self.assertAlmostEqual(value['longitude'],11.5166666667)
        self.assertEqual(value['elevation_meters'],545.4)
        for bad in [GOOD.replace(',1,08,',',0,08,'),
                    GOOD.replace('4807.038,N,01131.000,E','0000.000,N,00000.000,E'),
                    GOOD.replace('4807.038','4860.000'), GOOD.replace('4807.038','9100.000'),
                    GOOD.replace(',N,',',X,'), GOOD.replace(',1,08,',',6,08,'),
                    GOOD.replace('545.4','nan'), GOOD.replace('123519','999999'),
                    GOOD.replace(',M,46.9',',F,46.9'), GOOD+',extra',
                    'GPRMC,123519,V,4807.038,N,01131.000,E,0,0,230394,,,N']:
            with self.subTest(bad=bad): self.assertIsNone(gps.parse_fix(sentence(bad)))
        for bad in [b'',b'$GPGGA,no-checksum\n',b'\xff',sentence(GOOD)[:-4]+b'00\r\n',b'$'+b'x'*10000]:
            self.assertIsNone(gps.parse_fix(bad))

    def test_existing_position_does_not_open_device_or_rewrite(self):
        with tempfile.TemporaryDirectory() as directory:
            config=Path(directory)/'config.yaml'; config.write_text('unchanged')
            before=config.stat()
            with patch.object(gps,'prestart_only'), patch.object(gps,'STARTUP',Path(directory)/'startup.json'), \
                 patch.object(gps,'load_config',return_value=({'node':{'location':gps.parse_fix(sentence(GOOD))}},b'unchanged',before)), \
                 patch.object(gps,'open_gps') as opened, patch.object(gps,'atomic_write') as writer:
                gps.seed()
                opened.assert_not_called(); writer.assert_not_called()
            self.assertEqual(config.read_bytes(),b'unchanged')
            self.assertEqual(config.stat().st_mtime_ns,before.st_mtime_ns)

    def test_no_fix_stream_never_writes_location(self):
        nofix=sentence(GOOD.replace(',1,08,',',0,08,'))+sentence(GOOD.replace('4807.038,N,01131.000,E','0000.000,N,00000.000,E'))
        class EndFixture(Exception): pass
        with patch.object(gps,'prestart_only'), patch.object(gps,'load_config',return_value=({'node':{}},b'original',None)), \
             patch.object(gps,'open_gps',return_value=77), patch.object(gps,'device_matches',return_value=True), patch.object(gps,'report') as report, \
             patch.object(gps.select,'select',return_value=([77],[],[])), \
             patch.object(gps.os,'read',side_effect=[nofix,EndFixture()]), \
             patch.object(gps.os,'close'), patch.object(gps.fcntl,'ioctl'), patch.object(gps,'atomic_write') as writer:
            with self.assertRaises(EndFixture): gps.seed()
            writer.assert_not_called()
            self.assertEqual(report.call_args[0][0],'gps-waiting')

    def test_missing_device_waits_not_errors(self):
        class EndFixture(Exception): pass
        with patch.object(gps,'prestart_only'), patch.object(gps,'load_config',return_value=({'node':{}},b'original',None)), \
             patch.object(gps,'open_gps',side_effect=FileNotFoundError), patch.object(gps,'report') as report, \
             patch.object(gps.time,'sleep',side_effect=EndFixture), patch.object(gps,'atomic_write') as writer:
            with self.assertRaises(EndFixture): gps.seed()
            self.assertEqual(report.call_args[0],('gps-missing','GPS not detected: plug in the GPS'))
            writer.assert_not_called()

    def test_manual_or_running_engine_refuses(self):
        with patch.object(gps.subprocess,'check_output',return_value='0\n'):
            with self.assertRaises(ValueError): gps.prestart_only()
        with patch.object(gps.subprocess,'check_output',side_effect=[str(os.getpid()),'true']), \
             patch.object(gps.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
            with self.assertRaises(ValueError): gps.prestart_only()

    def test_busy_device_is_never_opened(self):
        with patch.object(gps,'held_elsewhere',return_value=True), patch.object(gps.os,'open') as opened:
            with self.assertRaises(BlockingIOError): gps.open_gps('/dev/null',9600)
            opened.assert_not_called()

    def test_fix_atomically_writes_only_location_and_releases_device(self):
        with tempfile.TemporaryDirectory() as directory:
            config=Path(directory)/'config.yaml'; startup=Path(directory)/'startup.json'
            original={'node':{'id':'kept'},'capture':{'interface':'wlan1'}}
            config.write_text(yaml.safe_dump(original)); config.chmod(0o640)
            before=config.stat(); raw=config.read_bytes()
            with patch.object(gps,'CONFIG',config), patch.object(gps,'STARTUP',startup), \
                 patch.object(gps,'prestart_only'), patch.object(gps,'load_config',return_value=(original,raw,before)), \
                 patch.object(gps,'open_gps',return_value=77), patch.object(gps,'device_matches',return_value=True), patch.object(gps,'report'), \
                 patch.object(gps.select,'select',return_value=([77],[],[])), \
                 patch.object(gps.os,'read',return_value=sentence(GOOD)), \
                 patch.object(gps.fcntl,'ioctl') as ioctl:
                real_close=os.close
                with patch.object(gps.os,'close',side_effect=lambda fd: None if fd==77 else real_close(fd)):
                    gps.seed()
                ioctl.assert_called_with(77,gps.termios.TIOCNXCL)
            value=yaml.safe_load(config.read_bytes())
            self.assertEqual(value['node']['id'],'kept')
            self.assertEqual(value['capture'],{'interface':'wlan1'})
            self.assertTrue(gps.location_valid(value['node']['location']))
            self.assertNotEqual(config.stat().st_ino,before.st_ino)
            self.assertEqual(config.stat().st_mode & 0o777,0o640)
            self.assertEqual(config.stat().st_uid,before.st_uid)
    def test_embedding_service_and_uninstall_contract(self):
        embedded=SOURCE.split("<<'GPS_SEED_EOF'\n",1)[1].split('\nGPS_SEED_EOF',1)[0]+'\n'
        self.assertEqual(embedded,(INSTALL/'gps-seed.py').read_text())
        for line in ['ExecStartPre=${GPS_SEED}','TimeoutStartSec=infinity','NotifyAccess=all',
                     'run systemctl --no-block restart brrdfeeder-engine.service']:
            self.assertIn(line,SOURCE)
        self.assertNotIn('run systemctl restart brrdfeeder-engine.service',SOURCE)
        helper=(INSTALL/'uninstall.sh').read_text()
        self.assertIn('/usr/local/libexec/brrdfeeder-gps-seed',helper)
        self.assertLess(helper.index('stop_unit system brrdfeeder-engine.service'),helper.index('for file in "${files[@]}"; do\n'))
        template=SOURCE.split("<<'CFGEOF'\n",1)[1].split('\nCFGEOF',1)[0]
        self.assertNotIn('location',yaml.safe_load(template)['node'])
        self.assertTrue(yaml.safe_load(template)['sensors']['gps']['required'])

    def test_bootstrap_position_has_no_prompt_or_required_flags(self):
        source=(INSTALL/'bootstrap-prepare.sh').read_text()
        section=source.split('# ── Sensor position',1)[1].split('# ── Fetch, verify',1)[0]
        self.assertNotIn('/dev/tty',section)
        self.assertNotIn('read -r',section)
        script='bootstrap_note() { echo "$*"; }; bootstrap_die() { echo "$*"; exit 1; }; bootstrap_say() { :; };\n'
        body=section[section.index('\n'):]
        for mode,lat,lon,expected,phrase in [('fresh',0,0,0,'GPS service will wait'),('fresh',1,1,0,'expert override'),
                                            ('fresh',1,0,1,'together'),('rerun',0,0,0,'preserved')]:
            result=subprocess.run(['bash','-c',script+f'install_mode={mode}; lat_supplied={lat}; lon_supplied={lon};\n'+body],stdin=subprocess.DEVNULL,text=True,capture_output=True)
            self.assertEqual(result.returncode,expected,result.stderr)
            self.assertIn(phrase,result.stdout)

if __name__=='__main__': unittest.main(verbosity=2)
