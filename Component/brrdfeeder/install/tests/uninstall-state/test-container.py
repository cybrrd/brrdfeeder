#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Four-tree acceptance: disposable OS only, no production or host writes."""
import os
from pathlib import Path
import unittest
from fixture import Fixture, S, P, T, E, UID, GID, RENEW, file


class Layouts(Fixture):
    def test_v0829_used_updater(self):
        self.seed('0.8.29'); self.positive()

    def test_v0830_used_updater(self):
        self.seed('0.8.30'); self.positive()

    def test_v090_with_renewal_secrets(self):
        self.seed(); self.positive()

    def test_optional_and_crash_states(self):
        for version in ('0.8.29','0.8.30','0.9.0'):
            with self.subTest(version=version):
                self.seed(version,optional=True,crash=True); self.positive()

    def test_unknown_S_P_E_preserves_everything(self):
        for tree in (S,P,E):
            with self.subTest(tree=str(tree)):
                self.seed(); file(tree/'operator-keeps-this')
                self.refused('unrecognised')

    def test_unknown_secret_and_capture_children(self):
        for tree in (S/'credentials',S/'capture'):
            with self.subTest(tree=str(tree)):
                self.seed(optional=True); file(tree/'operator-keeps-this',0o600,UID,GID)
                self.refused('unknown')

    def test_status_unknown_regular_rule_is_explicitly_deferred(self):
        self.seed(); file(T/'operator-unknown',0o600,UID,GID)
        self.positive()


class Safety(Fixture):
    def test_every_new_file_rejects_symlink_and_second_link(self):
        paths=[S/'release_attempt.json',P/'attempt.json',S/'status.json',S/'status.json.tmp.101.0',
               S/'credentials/brrdfeeder.creds',S/'credentials/previous.creds',S/'credentials'/RENEW,
               S/'credentials/.renew.lock',S/'capture/capture-00.pcap',
               P/'.host-recover-abcd_123',E/'.host-recover-abcd_123',E/'.config.yaml-abcd_123',E/'.update-0']
        for path in paths:
            for kind in ('symlink','hardlink'):
                with self.subTest(path=str(path),kind=kind):
                    self.seed(optional=True,crash=True)
                    outside=self.work/'outside'; outside.unlink(missing_ok=True)
                    if kind=='symlink':
                        outside.write_bytes(path.read_bytes()); path.unlink(); path.symlink_to(outside)
                    else: os.link(path,outside)
                    original=outside.read_bytes()
                    self.refused('REFUSED')
                    self.assertEqual(outside.read_bytes(),original)

    def test_secret_owner_modes_and_directory_types(self):
        for target,change in [('directory','mode'),('directory','owner'),('directory','symlink'),
                              ('active','mode'),('active','owner'),('active','fifo')]:
            with self.subTest(target=target,change=change):
                self.seed()
                path=S/'credentials' if target=='directory' else S/'credentials/brrdfeeder.creds'
                if change=='mode': path.chmod(0o755 if target=='directory' else 0o644)
                if change=='owner': os.chown(path,0,0)
                if change=='symlink':
                    moved=self.work/'kept-secrets'; path.rename(moved); path.symlink_to(moved)
                if change=='fifo': path.unlink(); os.mkfifo(path)
                self.refused('REFUSED')

    def test_pending_only_exact_internal_pair_is_allowed(self):
        for kind in ('outside','three-links','wrong-name','two-temporaries','symlink','wrong-mode','wrong-owner'):
            with self.subTest(kind=kind):
                self.seed(crash=True)
                temp=P/'.pending-123456'; target=P/'pending_update.json'
                outside=self.work/'outside'; outside.unlink(missing_ok=True)
                if kind=='outside': temp.unlink(); os.link(target,outside)
                if kind=='three-links': os.link(target,outside)
                if kind=='wrong-name': temp.rename(P/'.pending-not-generated')
                if kind=='two-temporaries': target.rename(P/'.pending-987655')
                if kind=='symlink': temp.unlink(); temp.symlink_to(target)
                if kind=='wrong-mode': temp.chmod(0o644)
                if kind=='wrong-owner': os.chown(temp,UID,GID)
                self.refused('REFUSED')

    def test_pending_single_temporary_and_single_target(self):
        self.seed(crash=True); (P/'.pending-123456').unlink(); self.positive()

    def test_pending_pair_requires_private_directory(self):
        for change in ('mode','owner'):
            with self.subTest(change=change):
                self.seed(crash=True)
                if change=='mode': P.chmod(0o755)
                else: os.chown(P,UID,GID)
                self.refused('wrong package directory')

    def test_new_records_and_temporaries_require_expected_metadata(self):
        for path in (S/'release_attempt.json',P/'attempt.json',E/'.update-0',
                     E/'.host-recover-abcd_123',E/'.config.yaml-abcd_123',P/'.host-recover-abcd_123',
                     S/'status.json',S/'capture/capture-00.pcap'):
            for change in ('mode','owner'):
                with self.subTest(path=str(path),change=change):
                    self.seed(optional=True,crash=True)
                    if change=='mode': path.chmod(0o666)
                    else: os.chown(path,123,123)
                    self.refused('wrong package file')

    def test_temporary_names_are_not_blanket_prefixes(self):
        for root,name in [(P,'.pending-01'),(P,'.pending-4294967296'),(P,'.pending-not-a-number'),
                          (E,'.update-4294967296'),(E,'.update-01'),(E,'.update-123.extra'),
                          (P,'.host-recover-too-long-name'),(E,'.host-recover-ABCDEFGH'),
                          (E,'.config.yaml-toolong123'),(S,'status.json.tmp.0.0'),
                          (S,'status.json.tmp.1.x'),(S,'status.json.tmp.4294967296.0'),
                          (S,'status.json.tmp.1.18446744073709551616'),(S/'capture','capture-16.pcap'),
                          (S/'credentials','.renew-'+('U'+'A'*55))]:
            with self.subTest(name=name):
                self.seed(optional=True,crash=True)
                file(root/name,0o600,UID if root in (S,S/'capture',S/'credentials') else 0,
                     GID if root in (S,S/'capture',S/'credentials') else 0)
                self.refused('REFUSED')

    def test_optional_paths_require_matching_config(self):
        for config in ('node: {status_file: /var/lib/brrdfeeder-status/status.json}\ncapture: {}\n',
                       'node: {status_file: /var/lib/brrdfeeder/status.json}\ncapture: {savefile: {directory: /somewhere/else}}\n'):
            self.seed(optional=True); (E/'config.yaml').write_text(config)
            self.refused('not configured here')

    def test_mount_boundary_observation_refuses_before_any_removal(self):
        for path in (S/'credentials',S/'credentials/brrdfeeder.creds',P/'.pending-123456',S/'capture'):
            with self.subTest(path=str(path)):
                self.seed(optional=True,crash=True)
                file(self.bin/'findmnt',0o755,data='#!/bin/sh\nprintf "%s\\n" '+str(path)+'\n')
                self.refused('mounted filesystem')
                (self.bin/'findmnt').unlink()


if __name__=='__main__': unittest.main(verbosity=2)
