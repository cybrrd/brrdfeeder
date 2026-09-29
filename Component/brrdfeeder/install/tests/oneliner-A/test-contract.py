#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline one-liner/source migration checks; real sudo proof is separate."""
import hashlib
import re
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[5]
INSTALL=ROOT/'Component/brrdfeeder/install'

class Contract(unittest.TestCase):
    def test_actual_pin_and_preparation_embedding(self):
        script=(INSTALL/'brrdfeeder-install.sh').read_text()
        # D44 remains isolated under /dev until native acceptance; public stays proven.
        variant='bootstrap-dev.sh' if (INSTALL/'bootstrap-dev.sh').exists() else 'bootstrap.sh'
        pin=re.search('INSTALLER_SHA256="([0-9a-f]{64})"',(INSTALL/variant).read_text()).group(1)
        self.assertEqual(pin,hashlib.sha256((INSTALL/'brrdfeeder-install.sh').read_bytes()).hexdigest())
        self.assertEqual(script.split("<<'BOOTSTRAP_PREPARE_EOF'\n",1)[1].split('\nBOOTSTRAP_PREPARE_EOF',1)[0]+'\n',(INSTALL/'bootstrap-prepare.sh').read_text())
    def test_root_only_preparation_is_gated_for_recovery_and_dryrun(self):
        script=(INSTALL/'brrdfeeder-install.sh').read_text()
        guard='BOOT_PREPARE='+script.split('BOOT_PREPARE=',1)[1].split('if [[ $BOOT_PREPARE == 1 ]]',1)[0]
        for mode in ['--uninstall','--status','--support-bundle','--verify','--dry-run','--interface']:
            result=subprocess.run(['bash','-c','BRRDFEEDER_BOOTSTRAP_PREPARE=1\n'+guard+'echo "$BOOT_PREPARE"','fixture',mode],capture_output=True,text=True,check=True)
            self.assertEqual(result.stdout.strip(),'1' if mode=='--interface' else '0')
        bootstrap=(INSTALL/'bootstrap.sh').read_text()
        self.assertNotIn('apt-get',bootstrap);self.assertNotIn('rm -f "$CONFIG_PATH"',bootstrap)
        self.assertLess(bootstrap.index('INSTALLER CHECKSUM MISMATCH'),bootstrap.index('sudo -v'))
        self.assertNotIn('sudo -E',bootstrap.replace('# Explicit handoff only: no sudo -E, no broad caller environment preservation.',''))
    def test_preparation_fresh_template_and_existing_preserved(self):
        source=(INSTALL/'bootstrap-prepare.sh').read_text()
        # Only root check is bypassed in this user-owned fixture. Every mutable
        # config path is temporary; command stubs prevent host/package changes.
        source='\n'.join(l for l in source.splitlines() if not l.startswith('[[ $EUID == 0 ]]'))
        prefix='''set -euo pipefail
CONFIG_PATH=$1; shift
log_event() { echo "$*"; }
run_step() { shift; "$@"; }
uname() { echo aarch64; }
ip() { echo '1.1.1.1 via 192.0.2.1 dev eth0 src 192.0.2.50'; }
apt-get() { echo UNEXPECTED_PACKAGE_CALL; return 99; }
'''
        with tempfile.TemporaryDirectory() as tmp:
            config=Path(tmp)/'config.yaml'
            for kind in ['fresh','template','existing']:
                if kind=='template': config.write_text('interface: EDIT-ME-wlanX\n')
                if kind=='existing': config.write_text('interface: wlan1\n')
                result=subprocess.run(['bash','-c',prefix+source+'\nprintf "ARG:%s\\n" "$@"','fixture',str(config),'--interface','wlan1'],capture_output=True,text=True)
                self.assertEqual(result.returncode,0,result.stdout+result.stderr)
                self.assertIn('ARG:192.0.2.50:8080',result.stdout)
                self.assertNotIn('UNEXPECTED_PACKAGE_CALL',result.stdout)
                if kind=='existing': self.assertEqual(config.read_text(),'interface: wlan1\n')
                else: self.assertFalse(config.exists())
    def test_advertised_commands_and_no_dead_temp_hints(self):
        for name in ['bootstrap.sh','install-log.py','bootstrap-prepare.sh']:
            text=(INSTALL/name).read_text()
            self.assertNotIn('https://get.cybrrd.com | sudo bash',text)
        installer=(INSTALL/'brrdfeeder-install.sh').read_text()
        self.assertNotIn('sudo bash $0',installer)
        self.assertIn('sudo brrdfeeder status',installer)
        self.assertIn('read -r -t 60',installer)

if __name__=='__main__': unittest.main(verbosity=2)
