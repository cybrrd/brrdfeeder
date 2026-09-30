#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline contract tests; do not execute the installer on the host."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[5]
SCRIPT = ROOT / 'Component/brrdfeeder/install/brrdfeeder-install.sh'
TEXT = SCRIPT.read_text()

def heredoc(marker):
    return TEXT.split("<<'" + marker + "'\n", 1)[1].split('\n' + marker, 1)[0]

class Package(unittest.TestCase):
    def test_shipped_provisioner_is_embedded_byte_for_byte(self):
        self.assertEqual(heredoc('STATUS_PROVISIONER_EOF') + '\n',
                         (ROOT / 'Component/brrdhouse/deploy/provision-status.sh').read_text())
        self.assertIn('\n  "$STATUS_PROVISIONER"\n', TEXT)

    def test_identity_helper_unchanged(self):
        self.assertEqual(heredoc('IDENTITY_SH_EOF') + '\n',
                         (ROOT / 'Component/aviary/deploy/bootstrap/brrdfeeder-image-identity.sh').read_text())

    def test_same_pin_validator_for_both_images(self):
        body = TEXT.split('valid_image_pin() {\n', 1)[1].split('\n}', 1)[0]
        self.assertEqual(TEXT.count('valid_image_pin()'), 1)
        self.assertIn('valid_image_pin "$CONSOLE_IMAGE" "$CONSOLE_REPOSITORY"', TEXT)
        for repository in ['ghcr.io/cybrrd/brrdfeeder', 'ghcr.io/cybrrd/brrdhouse']:
            for value, accepted in [
                (repository + '@sha256:' + 'a' * 64, True),
                (repository + ':latest', False),
                (repository + '@sha256:' + 'a' * 63, False),
                (repository + '@sha256:' + 'A' * 64, False),
                (repository + ':latest@sha256:' + 'a' * 64, False),
                (repository + '@sha256:' + 'a' * 64 + '\nImage=evil', False),
                ('localhost/fake@sha256:' + 'a' * 64, False),
                ('', False),
            ]:
                with self.subTest(repository=repository, value=value):
                    proc = subprocess.run(['bash', '-c', 'valid_image_pin() {\n' + body + '\n}\nvalid_image_pin "$1" "$2"',
                                           'test', value, repository])
                    self.assertEqual(proc.returncode == 0, accepted)

    def test_console_image_is_pinned_in_own_rootless_store(self):
        for contract in ['console_run podman pull "$CONSOLE_IMAGE"',
                         'console_run podman image inspect "$CONSOLE_IMAGE"',
                         'grep -qxF "$CONSOLE_IMAGE"',
                         '"$REQUESTED_CONSOLE_IMAGE" == "$INSTALLED_CONSOLE_IMAGE"',
                         'CONSOLE_IMAGE=$INSTALLED_CONSOLE_IMAGE']:
            self.assertIn(contract, TEXT)
        self.assertLess(TEXT.index('Console RepoDigests mismatch'), TEXT.index('NEW_QUADLET='))

    def test_mounts_and_nonroot_service_boundaries(self):
        console = TEXT.split('<<CONSOLE_QUADLET_EOF\n', 1)[1].split('\nCONSOLE_QUADLET_EOF', 1)[0]
        for line in ['User=65532:65532', 'DropCapability=all', 'NoNewPrivileges=true',
                     'ReadOnly=true', 'ReadOnlyTmpfs=false', 'Pull=never',
                     'Volume=${STATUS_DIR}:${STATUS_DIR}:ro', 'WantedBy=default.target']:
            self.assertIn(line, console.splitlines())
        self.assertEqual([line for line in console.splitlines() if line.startswith('Volume=')],
                         ['Volume=${STATUS_DIR}:${STATUS_DIR}:ro'])
        self.assertIn('User=${TARGET_UID}:${TARGET_GID}', TEXT)
        self.assertIn('Volume=${STATUS_DIR}:${STATUS_DIR}:rw', TEXT)
        self.assertNotIn('User=0:0', TEXT)
        self.assertIn('loginctl enable-linger "$CONSOLE_USER"', TEXT)
        self.assertIn('console_run systemctl --user restart brrdhouse.service', TEXT)

    def test_listener_accepts_literal_lan_and_rejects_unsafe_addresses(self):
        code = heredoc('LISTEN_PY')
        for value, accepted in [('192.0.2.50:8080', True), ('[fd00::50]:8080', True),
                                ('127.0.0.1:8080', True), ('0.0.0.0:8080', False),
                                ('[::]:8080', False), ('100.64.0.1:8080', False),
                                ('8.8.8.8:8080', False), ('192.0.2.50:80', False),
                                ('192.0.2.50:08080', False), ('evil.example:8080', False),
                                ('[fe80::1]:8080', False), ('127.0.0.1:8080\nExec=bad', False)]:
            with self.subTest(value=value):
                proc = subprocess.run(['python3', '-c', code, value], capture_output=True)
                self.assertEqual(proc.returncode == 0, accepted, proc.stderr)

    def test_status_config_is_package_owned_without_duplicate_node(self):
        # Missing installer dependencies are a test failure, not an inherited pass.
        import yaml
        code = heredoc('STATUS_CONFIG_PY')
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'config.yaml'
            config.write_text('node:\n  id: kept\n  status_interval_secs: 77\ncapture:\n  interface: wlan1\n')
            proc = subprocess.run(['python3', '-c', code, str(config), '/var/lib/brrdfeeder-status/status.json'], capture_output=True)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            value = yaml.safe_load(config.read_text())
            self.assertEqual(value['node']['status_file'], '/var/lib/brrdfeeder-status/status.json')
            self.assertEqual(value['node']['status_interval_secs'], 77)
            self.assertEqual(value['capture']['interface'], 'wlan1')
            config.write_text('node: {}\nnode: {}\n')
            before = config.read_bytes()
            proc = subprocess.run(['python3', '-c', code, str(config), '/var/lib/brrdfeeder-status/status.json'], capture_output=True)
            self.assertNotEqual(proc.returncode, 0)
            self.assertEqual(before, config.read_bytes())

    def test_customer_docs_have_no_separate_console_procedure(self):
        readme = (ROOT / 'Component/brrdhouse/README.md').read_text()
        for manual in ['sudo bash deploy/provision-status.sh', 'Add this line under', 'Arrange the dedicated console']:
            self.assertNotIn(manual, readme)
        self.assertIn('no separate console installation', readme)
        self.assertIn('--console-image', (SCRIPT.parent / 'README.md').read_text())

if __name__ == '__main__':
    unittest.main(verbosity=2)
