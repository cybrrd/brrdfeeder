#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline contracts for the BRRDfeeder naming packet (2026-09-25).

Guards the rename where it is a SOURCE property: the new self-recognition
name, backward compatibility with the legacy drop-in, and the boundary of the
rename itself (identity strings and historical records must stay untouched).

Behavioural proofs (real uninstall/install runs) live in container-tests.py,
executed only inside a disposable root OS container; evidence under evidence/.
"""
from pathlib import Path
import os
import re
import unittest

ROOT=Path(os.environ.get('NAMING_ROOT',Path(__file__).resolve().parents[5]))
INSTALL=ROOT/'Component/brrdfeeder/install'
INSTALLER=(INSTALL/'brrdfeeder-install.sh').read_text()
HELPER=(INSTALL/'uninstall.sh').read_text()
BOOTSTRAP=(INSTALL/'bootstrap.sh').read_text()
KIT=(ROOT/'Component/aviary/deploy/bootstrap/brrdfeeder-install.sh').read_text()

LEGACY_PATH='/etc/systemd/journald.conf.d/99-brrdfeeder-open.conf'
NEW_PATH='/etc/systemd/journald.conf.d/99-brrdfeeder.conf'
LEGACY_MARKER='BRRDfeeder Open tier'
NEW_MARKER='# BRRDfeeder — protect MicroSD card from journald write wear.'

# Directories whose living text must not name the product "BRRDfeeder Open".
SWEEP_DIRS=[ROOT/'Component/aviary',ROOT/'Component/brrdfeeder',ROOT/'Component/brrdhouse']
SWEEP_SKIP=('Component/aviary/captures/','Component/aviary/docs/decisions/','Component/brrdfeeder/install/tests/',
            'governance/dispatch/','governance/releases/','notes/',
            'governance/2026-09-13-consolidation-discovery-map.md',
            'governance/2026-09-19-OPEN-DIMENSIONS-REGISTER.md',
            'governance/C1BRD-migration-proposal.md')
# Deliberate legacy-artifact references in compat code: the legacy drop-in's
# path/marker are RECOGNITION identities (packet section B) and the quadlet
# test seeds real legacy-file content. Lines carrying them are skipped; prose
# mentions without the artifact reference still fail the sweep.
RECOGNITION_LINE=re.compile(r'99-brrdfeeder-open|LEGACY_MARKER|BRRDfeeder Open tier — protect')
# Files that carry Cy's verbatim quoted requirements, which mention the old
# name because Cy said it. The quote survives; only prose around it changes.
QUOTE_BEARING=['governance/BACKLOG-post-demo.md',
               'design/local-management-console/2026-09-18-REQUIREMENTS-SKELETON.md']
SWEEP_PATTERN=re.compile(r'BRRDfeeder[ -](?:Open|OPEN|open)\b')
TIER_PATTERN=re.compile(r'\bOpen[- ]tier\b')
TIER_DIRS=SWEEP_DIRS
# Historical records: they MUST still carry the old name. Scrubbing them is
# falsifying a record, not fixing naming. ADRs are dated decision transcripts
# (0001's whole point is the retired assumption "BRRDfeeder-Open is the product").


class DropInContract(unittest.TestCase):
    def test_installer_uses_new_path_and_keeps_legacy_recognition(self):
        self.assertIn(f'JOURNALD_DROPIN="{NEW_PATH}"',INSTALLER)
        self.assertIn(LEGACY_PATH,INSTALLER)
        self.assertIn(NEW_MARKER,INSTALLER)
        # The legacy marker remains ONLY as a recognition signature for old files.
        self.assertIn(f"'{LEGACY_PATH}|{LEGACY_MARKER}'",INSTALLER)
        self.assertIn(f"'{NEW_PATH}|",INSTALLER)
    def test_uninstaller_plans_both_paths_with_distinct_markers(self):
        for source in (HELPER,INSTALLER):
            self.assertIn(f'\'{NEW_PATH}|',source)
            self.assertIn(f"'{LEGACY_PATH}|{LEGACY_MARKER}'",source)
        # Each signature must match its own file and only its own file:
        # the legacy content line carries the old tier name, the new line
        # stops right after the product name, so neither is a substring of
        # the other and a foreign file matches neither.
        legacy_content=f'# {LEGACY_MARKER} — protect MicroSD card from journald write wear.'
        self.assertNotIn(NEW_MARKER,legacy_content)
        self.assertNotIn(LEGACY_MARKER,NEW_MARKER)
        self.assertNotIn(LEGACY_MARKER,BOOTSTRAP)
    def test_journald_restart_covers_either_path(self):
        for source,name in ((INSTALLER,'installer'),(HELPER,'uninstall.sh')):
            had=re.search(r'had_journal=0; (.+)',source)
            self.assertTrue(had,name+' lost its had_journal guard')
            self.assertIn(NEW_PATH,had.group(1),name)
            self.assertIn(LEGACY_PATH,had.group(1),name)
    def test_kit_installer_same_rule(self):
        self.assertIn(f'"{NEW_PATH}"',KIT)
        self.assertIn(LEGACY_PATH,KIT)
        self.assertIn(NEW_MARKER,KIT)

class BoundaryContract(unittest.TestCase):
    def test_identity_strings_are_unchanged(self):
        self.assertIn('IMAGE_REPOSITORY="ghcr.io/cybrrd/brrdfeeder"',INSTALLER)
        self.assertIn('OAUTH_CLIENT_ID="376855240068104243"',INSTALLER)
        self.assertIn('remove_images system ghcr.io/cybrrd/brrdfeeder brrdfeeder-engine',HELPER)
        self.assertIn('brrdfeeder-open-tier',INSTALLER)  # Zitadel app name is identity
    def test_bootstrap_banner_and_config_header(self):
        self.assertIn('BANNER="BRRDfeeder — installer"',BOOTSTRAP)
        self.assertIn('# /etc/brrdfeeder/config.yaml — BRRDfeeder node configuration',INSTALLER)
    def test_no_product_rename_left_in_living_text(self):
        bad=[]
        for base in SWEEP_DIRS:
            for path in base.rglob('*'):
                if not path.is_file() or any(p in path.parts for p in ('target', '__pycache__', 'tests')) or any(part in path.as_posix() for part in SWEEP_SKIP):
                    continue
                text=path.read_text(errors='replace')
                relative=path.relative_to(ROOT).as_posix()
                if relative in QUOTE_BEARING:
                    # strip Cy's quoted spans; the name inside a verbatim quote is his, not ours
                    text=re.sub(r'(?s)"[^"]*"','',text)
                if base.is_relative_to(ROOT/'Component'):
                    text='\n'.join(line for line in text.splitlines()
                                   if not RECOGNITION_LINE.search(line))
                if SWEEP_PATTERN.search(text):
                    bad.append((path.as_posix(),'BRRDfeeder[ -]Open'))
                if TIER_PATTERN.search(text) and base in TIER_DIRS:
                    bad.append((path.as_posix(),'Open[- ]tier'))
        self.assertFalse(bad,'old product name survives in living text:\n'+str(bad))

if __name__=='__main__': unittest.main(verbosity=2)
