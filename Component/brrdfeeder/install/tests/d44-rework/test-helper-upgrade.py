#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Temporary-root artifact dispatch proofs; never invoke the real installer."""
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import stat
from types import SimpleNamespace
import unittest
from unittest.mock import patch

INSTALL = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('upgrade', INSTALL/'helper-upgrade.py')
upgrade = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upgrade)
STAGING_PARENT = upgrade.staging_parent


class HelperDispatch(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.raw = b'\x7fELF\x02\x01' + bytes(12) + b'\xb7\x00' + bytes(100)
        self.expected = hashlib.sha256(self.raw).hexdigest()
        self.installer = self.root/'installer'
        self.installer.write_text('readonly RELEASE_HELPER_SHA256="'+self.expected+'"\n')
        self.calls = []
        self.download = self.raw
        original_temp = tempfile.TemporaryDirectory
        def staging(**kwargs):
            self.assertEqual(kwargs['dir'], Path('/usr/local/libexec'))
            return original_temp(prefix=kwargs['prefix'], dir=self.root)
        self.stack = []
        for mock in (patch.object(upgrade.os, 'geteuid', return_value=0),
                     patch.object(upgrade.platform, 'machine', return_value='aarch64'),
                     patch.object(upgrade, 'staging_parent', return_value=Path('/usr/local/libexec')),
                     patch.object(upgrade.tempfile, 'TemporaryDirectory', side_effect=staging),
                     patch.object(upgrade.subprocess, 'run', side_effect=self.run_command)):
            mock.start()
            self.addCleanup(mock.stop)

    def run_command(self, argv, **kwargs):
        self.calls.append(argv)
        if argv[0] == '/usr/bin/curl':
            self.assertEqual(argv[-1], 'https://get.cybrrd.com/releases/v1/updater/'+self.expected+'/linux-arm64/brrdfeeder-release')
            self.assertNotIn('--location', argv)
            Path(argv[argv.index('--output')+1]).write_bytes(self.download)
        else:
            self.assertTrue(Path(argv[0]).is_relative_to(self.root))
            self.assertEqual(argv[1:], ['upgrade-local-helper', '--sha256', self.expected])
            self.assertEqual(Path(argv[0]).read_bytes(), self.raw)
        # There is no permitted OAuth/device/Flock/credential-service command.
        for forbidden in ('oauth', 'device', 'enroll', 'hospitality', 'ingest'):
            self.assertNotIn(forbidden, ' '.join(argv).lower())

    def test_default_only_pinned_artifact_then_local_command(self):
        upgrade.main([str(self.installer), '--upgrade-helper'])
        self.assertEqual(len(self.calls), 2)
        self.assertEqual(list(self.root.iterdir()), [self.installer])

    def test_optional_local_copy_never_downloads(self):
        local = self.root/'local'
        local.write_bytes(self.raw)
        upgrade.main([str(self.installer), '--upgrade-helper', '--helper-file', str(local)])
        self.assertEqual(len(self.calls), 1)
        self.assertNotEqual(self.calls[0][0], str(local))

    def test_wrong_hash_never_executes_candidate(self):
        self.download = self.raw+b'tampered'
        with self.assertRaisesRegex(RuntimeError, 'SHA256'):
            upgrade.main([str(self.installer), '--upgrade-helper'])
        self.assertEqual(len(self.calls), 1)

    def test_wrong_arch_never_executes_candidate(self):
        self.raw = self.raw[:18]+b'\x3e\x00'+self.raw[20:]
        self.download = self.raw
        self.expected = hashlib.sha256(self.raw).hexdigest()
        self.installer.write_text('readonly RELEASE_HELPER_SHA256="'+self.expected+'"\n')
        with self.assertRaisesRegex(RuntimeError, 'architecture'):
            upgrade.main([str(self.installer), '--upgrade-helper'])
        self.assertEqual(len(self.calls), 1)

    def test_other_flags_refused_without_commands(self):
        with self.assertRaisesRegex(RuntimeError, 'usage'):
            upgrade.main([str(self.installer), '--upgrade-helper', '--image', 'anything'])
        self.assertEqual(self.calls, [])

    def test_execution_denied_has_actionable_safe_error(self):
        with patch.object(upgrade.subprocess, 'run', side_effect=PermissionError('private filename')):
            local = self.root/'local'
            local.write_bytes(self.raw)
            with self.assertRaisesRegex(upgrade.ExecutionPrerequisiteError, 'mount options') as error:
                upgrade.main([str(self.installer), '--upgrade-helper', '--helper-file', str(local)])
            self.assertNotIn('private filename', str(error.exception))

    def test_staging_checks_every_parent_and_refuses_unsafe_metadata(self):
        safe = SimpleNamespace(st_mode=stat.S_IFDIR | 0o755, st_uid=0)
        with patch.object(Path, 'lstat', return_value=safe) as checked:
            self.assertEqual(STAGING_PARENT(), Path('/usr/local/libexec'))
            self.assertEqual(checked.call_count, 4)
        for bad in (SimpleNamespace(st_mode=stat.S_IFLNK | 0o777, st_uid=0),
                    SimpleNamespace(st_mode=stat.S_IFDIR | 0o775, st_uid=0),
                    SimpleNamespace(st_mode=stat.S_IFDIR | 0o755, st_uid=1000)):
            for index in range(4):
                values = [safe] * 4
                values[index] = bad
                with patch.object(Path, 'lstat', side_effect=values):
                    with self.assertRaises(upgrade.ExecutionPrerequisiteError):
                        STAGING_PARENT()

    def test_embedded_source_and_early_dispatch(self):
        text = (INSTALL/'brrdfeeder-install.sh').read_text()
        embedded = text.split("<<'HELPER_UPGRADE_PY'\n", 1)[1].split('\nHELPER_UPGRADE_PY', 1)[0]+'\n'
        self.assertEqual(embedded, (INSTALL/'helper-upgrade.py').read_text())
        self.assertLess(text.index('if [[ ${1:-} == --upgrade-helper ]]'), text.index('# Default logging starts'))
        for name in ('bootstrap.sh', 'bootstrap-dev.sh', 'bootstrap-d44.sh'):
            wrapper = (INSTALL/name).read_text()
            self.assertIn('--upgrade-helper) BOOT_MODE=upgrade-helper ;;', wrapper)
        # No candidate identity service or general updater execution is reachable.
        source = (INSTALL.parents[1]/'aviary/release-system/local_upgrade.go').read_text()
        for forbidden in ('newUpdater(', '.execute(', 'http.', 'client.', 'renew-owner'):
            self.assertNotIn(forbidden, source)


if __name__ == '__main__':
    unittest.main(verbosity=2)
