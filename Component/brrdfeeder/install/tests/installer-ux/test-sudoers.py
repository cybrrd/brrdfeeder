#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Real Debian sudo policy. Disposable network-disabled container ONLY."""
import errno
import fcntl
import os
from pathlib import Path
import pty
import pwd
import select
import subprocess
import termios
import time
import unittest

assert os.geteuid() == 0 and Path('/run/.containerenv').exists()
assert os.environ.get('BRRD_INSTALLER_UX_CONTAINER') == '1'
ROOT = Path(os.environ.get('UX_SOURCE_ROOT', '/repo'))
BASELINE = os.environ.get('UX_BASELINE') == '1'
SOURCE = (ROOT/'Component/brrdfeeder/install/bootstrap.sh').read_text()


class Sudoers(unittest.TestCase):
    def setUp(self):
        subprocess.run(['useradd', '-m', '-G', 'sudo', 'fixture'], capture_output=True)
        subprocess.run(['chpasswd'], input='fixture:FIXTURE-PASS\n', text=True, check=True)
        self.policy = Path('/etc/sudoers.d/010_fixture-nopasswd')
        self.policy.write_text('Defaults verifypw=all\nfixture ALL=(ALL:ALL) NOPASSWD:ALL\n')
        self.policy.chmod(0o440)
        subprocess.run(['visudo', '-c'], check=True, capture_output=True)
        subprocess.run(['runuser', '-u', 'fixture', '--', 'sudo', '-K'], check=True)

    def command(self, *args):
        return subprocess.run(['runuser', '-u', 'fixture', '--', *args],
                              stdin=subprocess.DEVNULL, capture_output=True, text=True, start_new_session=True)

    def bootstrap_gate(self, terminal=False):
        # Execute the real privilege gate only. The final elevated command is
        # inert; no installer, downloaded source, package or host operation.
        block = SOURCE.split('  sudo_args=()\n', 1)[1].split('\nfi\n# Do not exec:', 1)[0]
        code = '''set -euo pipefail
die() { echo "$*"; exit 1; }
sudo_args=(); rc=0
RUN_ID=1234abcd; NOTES=fixture; BLOG=/tmp/fixture.log
installer=/tmp/inert; INSTALLER_SHA256=fixture
root_handoff='echo ELEVATED_FIXTURE'
'''+block+'\nexit "$rc"\n'
        argv = ['runuser', '-u', 'fixture', '--', 'bash', '-c', code]
        if not terminal:
            return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                                  text=True, start_new_session=True, timeout=10)
        master, slave = pty.openpty()
        account = pwd.getpwnam('fixture')
        os.fchown(slave, account.pw_uid, account.pw_gid)
        def session():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=slave,
                                   stderr=slave, preexec_fn=session)
        os.close(slave)
        output = bytearray(); sent = False; deadline = time.monotonic()+10
        try:
            while True:
                if time.monotonic() >= deadline:
                    process.kill(); self.fail('sudo fixture timed out')
                if select.select([master], [], [], .1)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError as error:
                        if error.errno == errno.EIO: break
                        raise
                    if not chunk: break
                    output.extend(chunk)
                    if b'password for fixture:' in output and not sent:
                        os.write(master, b'FIXTURE-PASS\n'); sent = True
                elif process.poll() is not None:
                    break
            return subprocess.CompletedProcess(argv, process.wait(timeout=2), output.decode(), '')
        finally:
            os.close(master)
            if process.poll() is None: process.kill(); process.wait()

    def test_debian_validation_differs_from_execution(self):
        self.assertNotEqual(self.command('sudo', '-n', '-v').returncode, 0)
        self.assertEqual(self.command('sudo', '-n', 'true').returncode, 0)
        result = self.bootstrap_gate()
        self.assertEqual(result.returncode, 1 if BASELINE else 0, result.stdout+result.stderr)
        if not BASELINE: self.assertIn('ELEVATED_FIXTURE', result.stdout)

    def test_passwordless_terminal_has_no_password_prompt(self):
        result = self.bootstrap_gate(terminal=True)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual('password for fixture:' in result.stdout, BASELINE)

    def test_password_only_terminal_authenticates(self):
        self.policy.write_text('Defaults verifypw=all\n')
        result = self.bootstrap_gate(terminal=True)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('password for fixture:', result.stdout)
        self.assertIn('ELEVATED_FIXTURE', result.stdout)

    def test_no_tty_password_only_is_actionable(self):
        self.policy.write_text('Defaults verifypw=all\n')
        result = self.bootstrap_gate()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('ELEVATED_FIXTURE', result.stdout)
        if not BASELINE:
            self.assertIn('no terminal is available for a password prompt', result.stdout)
            self.assertNotIn('no noninteractive sudo permission', result.stdout)

    def test_true_permission_does_not_authorize_installer(self):
        self.policy.write_text('Defaults verifypw=all\nfixture ALL=(ALL) NOPASSWD:/usr/bin/true\n')
        result = self.bootstrap_gate()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('ELEVATED_FIXTURE', result.stdout)


if __name__ == '__main__': unittest.main(verbosity=2)
