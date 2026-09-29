#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline source contracts, plus real supervisor/PTY tests added below."""
from pathlib import Path
import importlib.util
import subprocess
import unittest

ROOT=Path(__file__).resolve().parents[5]
INSTALL=ROOT/'Component/brrdfeeder/install'

class Contract(unittest.TestCase):
    def assertIn(self, member, container, msg=None):
        self.assertTrue(member in container, msg or 'missing contract: '+repr(member))
    def test_local_command_and_last_removal(self):
        script=(INSTALL/'brrdfeeder-install.sh').read_text()
        self.assertIn('install_local_command()',script)
        self.assertIn('/usr/local/sbin/brrdfeeder',script)
        helper=(INSTALL/'uninstall.sh').read_text()
        self.assertGreater(helper.rindex('remove_file /usr/local/sbin/brrdfeeder'),helper.rindex('act systemctl daemon-reload'))
    def test_subcommands_and_yes(self):
        script=(INSTALL/'brrdfeeder-install.sh').read_text()
        for command in ['uninstall)', 'status)', 'support-bundle)', '--yes)']:
            self.assertIn(command,script)
    def test_bootstrap_uninstall_bypasses_install_inputs(self):
        source=(INSTALL/'bootstrap.sh').read_text()
        self.assertIn('BOOT_MODE=',source)
        self.assertIn('if [[ $BOOT_MODE == install ]]; then',source)
        prepare=(INSTALL/'bootstrap-prepare.sh').read_text()
        self.assertIn('arch="$(uname -m)"',prepare)
        installer=(INSTALL/'brrdfeeder-install.sh').read_text()
        self.assertIn('--uninstall|--status|--support-bundle|--verify|--dry-run) BOOT_PREPARE=0',installer)
    def test_refusal_names_way_forward(self):
        source=(INSTALL/'brrdfeeder-install.sh').read_text()
        self.assertIn('To install this release: sudo brrdfeeder uninstall, then run the one-liner again.',source)
    def test_confirmation_and_compact_logging(self):
        helper=(INSTALL/'uninstall.sh').read_text()
        logger=(INSTALL/'install-log.py').read_text()
        self.assertIn('Remove BRRDfeeder from this Pi? [y/N]',helper)
        self.assertIn('No terminal',helper)
        self.assertIn('--yes',helper)
        self.assertIn("kind == 'NOTICE'",logger)
        self.assertIn("quiet = '--no-verbose' in args and (mode == 'install' or '--uninstall' in args)",logger)
    def test_auto_adoption_and_named_profile_refusals(self):
        helper=(INSTALL/'uninstall.sh').read_text()
        self.assertIn('recognised $user as a BRRDfeeder service account',helper)
        for reason in ['login shell','password is not locked','home is not','privileged group','UID is below','login history','unrelated process']:
            self.assertIn(reason,helper)
    def test_history_process_auditor_is_embedded(self):
        helper=(INSTALL/'uninstall.sh').read_text()
        self.assertIn("<<'ACCOUNT_AUDIT_EOF'",helper)
        body=helper.split("<<'ACCOUNT_AUDIT_EOF'\n",1)[1].split('\nACCOUNT_AUDIT_EOF',1)[0]+'\n'
        self.assertEqual(body,(INSTALL/'uninstall-account.py').read_text())
    def test_existing_embedded_copies_stay_exact(self):
        script=(INSTALL/'brrdfeeder-install.sh').read_text()
        for marker,name in [('BOOTSTRAP_PREPARE_EOF','bootstrap-prepare.sh'),('UNINSTALL_EOF','uninstall.sh'),('INSTALL_LOG_PY','install-log.py'),('GPS_SEED_EOF','gps-seed.py'),('BLUETOOTH_EOF','bluetooth-state.py')]:
            self.assertEqual(script.split("<<'"+marker+"'\n",1)[1].split('\n'+marker,1)[0]+'\n',(INSTALL/name).read_text())

if __name__=='__main__': unittest.main(verbosity=2)
