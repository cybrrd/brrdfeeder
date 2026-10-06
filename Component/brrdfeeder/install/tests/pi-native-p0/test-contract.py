#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline P0 regressions: actual shell fragments + logger, fake OAuth, PTY display."""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shlex
import subprocess
import tempfile
import termios
import time
import unittest

ROOT = Path(__file__).resolve().parents[5]
INSTALL = ROOT/'Component/brrdfeeder/install'
SOURCE = (INSTALL/'brrdfeeder-install.sh').read_text()
BOOTSTRAP = (INSTALL/'bootstrap.sh').read_text()


class Contract(unittest.TestCase):
    def memory(self, controllers, *, missing=False, dryrun=False):
        helper = (SOURCE.split('console_memory_policy() {', 1)[1].split('\n}\n', 1)[0]
                  if 'console_memory_policy() {' in SOURCE else '\n :')
        template = SOURCE.split('<<CONSOLE_QUADLET_EOF\n', 1)[1].split('\nCONSOLE_QUADLET_EOF', 1)[0]
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)/'cgroup'
            root.mkdir()
            # Kernel support does NOT imply this user's delegation supports it.
            (root/'cgroup.controllers').write_text('cpuset cpu io memory pids\n')
            user = root/'user.slice/user-1003.slice/user@1003.service'
            user.mkdir(parents=True)
            if not missing:
                (user/'cgroup.controllers').write_text(controllers+'\n')
                (user/'cgroup.subtree_control').write_text('')
            helper = helper.replace('/sys/fs/cgroup', str(root))
            script = '''set -eu
CONSOLE_UID=1003
say() { echo "$*" >&2; }
warn() { echo "$*" >&2; }
log_event() { echo "$*" >&2; }
systemctl() { echo queried >> "$PROBE_LOG"; echo /user.slice/user-1003.slice/user@1003.service; }
console_run() { echo queried >> "$PROBE_LOG"; "$@"; }
console_memory_policy() {'''+helper+'''
}
console_memory_policy
CONSOLE_IMAGE=ghcr.io/cybrrd/brrdhouse@sha256:'''+('a'*64)+'''
STATUS_DIR=/var/lib/brrdfeeder-status
CONSOLE_LISTEN=127.0.0.1:8080
UNIT_HOSTNAME=fixture
CONSOLE_PORT=8080
cat <<UNIT
'''+template+'\nUNIT\n'
            p = subprocess.run(['bash', '-c', 'PROBE_LOG='+shlex.quote(d+'/probes')+'\nDRY_RUN='+str(int(dryrun))+'\n'+script], capture_output=True, text=True)
            self.assertEqual(p.returncode, 0, p.stderr)
            if dryrun:
                self.assertFalse(Path(d+'/probes').exists())
            for hardening in ['--pids-limit=64', 'ReadOnly=true', 'DropCapability=all',
                              'NoNewPrivileges=true', 'User=65532:65532', 'UserNS=keep-id:', ':ro']:
                self.assertIn(hardening, p.stdout)
            generator = Path('/usr/lib/systemd/system-generators/podman-system-generator')
            if generator.exists():
                (Path(d)/'brrdhouse.container').write_text(p.stdout)
                generated = subprocess.run([str(generator), '--user', '--dryrun'],
                    env=dict(os.environ, QUADLET_UNIT_DIRS=d), text=True, capture_output=True)
                self.assertEqual(generated.returncode, 0, generated.stderr)
                command = next(line for line in generated.stdout.splitlines() if line.startswith('ExecStart='))
                self.assertIn('--pids-limit=64', command)
                self.assertEqual('--memory=96m' in command, '--memory=96m' in p.stdout)
            return p.stdout, p.stderr

    def test_kernel_memory_without_user_delegation_omits_limit(self):
        unit, log = self.memory('cpu pids')
        self.assertNotIn('--memory', unit)
        self.assertIn('console_memory_limit=omitted', log)
        self.assertIn('memory-not-delegated', log)

    def test_delegated_memory_retains_limit(self):
        unit, log = self.memory('cpu memory pids')
        self.assertIn('--memory=96m --memory-swap=96m', unit)
        self.assertIn('console_memory_limit=96m', log)

    def test_unknown_delegation_omits_limit(self):
        unit, log = self.memory('', missing=True)
        self.assertNotIn('--memory', unit)
        self.assertIn('console_memory_limit=omitted', log)

    def test_dryrun_does_not_probe_or_claim_limit(self):
        unit, log = self.memory('cpu memory pids', dryrun=True)
        self.assertNotIn('--memory', unit)
        self.assertIn('console_memory_limit=deferred', log)

    def oauth(self, display, handoff=True, scenario='approved', trace=False):
        with tempfile.TemporaryDirectory() as d:
            directory = Path(d)
            start = 'enrollment_display_available() {' if 'enrollment_display_available() {' in SOURCE else 'manual_creds_instructions() {'
            flow = start+SOURCE.split(start, 1)[1].split('\n# Verify USB hardware visible', 1)[0]
            driver = directory/'driver.py'
            driver.write_text('import importlib.util, sys\nfrom pathlib import Path\n'
                f's=importlib.util.spec_from_file_location("log",{str(INSTALL/"install-log.py")!r})\n'
                'log=importlib.util.module_from_spec(s); s.loader.exec_module(log)\n'
                f'log.LOG_DIR=Path({str(directory/"logs")!r})\n'
                'log.environment=lambda **kwargs: "offline fixture; no probes"\n'
                'sys.exit(log.supervise(sys.argv[1],[]))\n')
            script = directory/'installer.sh'
            script.write_text('set -euo pipefail\n'
                f'if [[ -z ${{BRRDFEEDER_LOG_CHILD:-}} ]]; then exec python3 {shlex.quote(str(driver))} "$0"; fi\n'
                f'source {shlex.quote(str(INSTALL/"log-events.sh"))}\n'
                f'cd {shlex.quote(d)}\n'
                f'SCENARIO={shlex.quote(scenario)}\n'
                '''DRY_RUN=0
CREDS_PATH=$PWD/fake.creds
REFRESH_TOKEN_PATH=$PWD/fake.refresh
CONFIG_PATH=$PWD/config.yaml
TARGET_GID=$(id -g)
OAUTH_ISSUER=https://oauth.invalid
OAUTH_CLIENT_ID=fixture
OAUTH_SCOPE=fixture
FLOCK_ENROLL_URL=https://flock.invalid/enroll
warn() { echo " !! $*"; }
ok() { echo "$*"; }
say() { echo "$*"; }
fatal() { echo "$*"; exit 1; }
# Inert file sink for the TTY/enrollment fixture; durable publication has its
# own root container tests and must not chown this unprivileged temp tree.
atomic_install() { cat > "$4"; }
gate() { log_event PHASE "$1"; }
run() { run_step enrollment/fixture "$@"; }
chown() { :; }  # no host ownership mutations
curl() {
  echo called >> calls
  case "$*" in
    *device_authorization*)
      echo auth >> auths
      n=$(wc -l < auths)
      printf '{"device_code":"SEEDED-DEVICE-SECRET-%s","user_code":"P0-CODE-1234-%s","verification_uri_complete":"https://oauth.invalid/verify?user_code=P0-CODE-1234-%s","expires_in":300,"interval":0}\\n' "$n" "$n" "$n" ;;
    *oauth/v2/token*)
      echo poll >> polls; n=$(wc -l < polls)
      case "$SCENARIO:$n" in
        expired:*|expired-once:1) printf '%s\\n400\\n' '{"error":"expired_token"}'; return ;;
        denied:*) printf '%s\\n400\\n' '{"error":"access_denied"}'; return ;;
        slow-pending:1|slow-pending:2) printf '%s\\n400\\n' '{"error":"authorization_pending"}'; return ;;
        slow-pending:3) printf '%s\\n400\\n' '{"error":"slow_down"}'; return ;;
      esac
      printf '%s\\n200\\n' '{"access_token":"eyJhbGciOiJIUzI1NiJ9.eyJzZWVkIjoidGVzdCJ9.signature"}' ;;
    *flock.invalid*)
      if [[ $SCENARIO == exchange-failed ]]; then printf 'unavailable\\n503\\n'; return; fi
      printf '%s\\n' '-----BEGIN NATS USER JWT-----' 'SEEDED-CREDS-CONTENT' '-----END NATS USER JWT-----' 200 ;;
    *) echo unexpected-endpoint >&2; return 98 ;;
  esac
}
sleep() { echo "$1" >> sleeps; if [[ $SCENARIO == deadline ]]; then echo $(($(cat clock 2>/dev/null || echo 0) + 400)) > clock; fi; }
date() { if [[ $SCENARIO == deadline && $1 == +%s ]]; then cat clock 2>/dev/null || echo 0; else command date "$@"; fi; }
'''+('set -x\n' if trace else '')+flow+'\necho FLOW_COMPLETED\n')
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
            for key in ['BRRDFEEDER_LOG_CHILD', 'BRRDFEEDER_LOG_TOKEN', 'BRRDFEEDER_DISPLAY_FD']:
                env.pop(key, None)
            prefix = f'[[ ! -t 0 ]] || exit 97\ninstaller={shlex.quote(str(script))}\n'
            if handoff:
                # Nonprivileged display/metadata fixture only. The root-staged
                # bootstrap and real non-root sudo/use_pty are now exercised by
                # oneliner-rework's container; never invoke host sudo here.
                prefix += 'export BRRDFEEDER_RUN_ID=aabbccdd BRRDFEEDER_BOOTSTRAP_NOTES=fixture; rc=0\n'
                code = prefix+'bash "$installer" || rc=$?\nexit "$rc"\n'
            else:
                code = prefix+'exec bash "$installer"\n'
            master, slave = pty.openpty()
            def session():
                os.setsid()
                if display in ('controlling', 'tty-only'):
                    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
            try:
                stdout = slave if display in ('controlling', 'stdout') else subprocess.PIPE
                p = subprocess.Popen(['bash'], stdin=subprocess.PIPE, stdout=stdout, stderr=subprocess.STDOUT,
                                     env=env, preexec_fn=session, pass_fds=(slave,) if display == 'tty-only' else ())
                os.close(slave); slave = -1
                p.stdin.write(code.encode()); p.stdin.close()
                output = bytearray()
                deadline = time.monotonic()+15
                while p.poll() is None or select.select([master], [], [], 0)[0]:
                    if time.monotonic() > deadline:
                        p.kill(); self.fail('PTY flow timed out')
                    if select.select([master], [], [], .1)[0]:
                        try:
                            chunk = os.read(master, 65536)
                            if not chunk: break
                            output.extend(chunk)
                        except OSError as e:
                            if e.errno == errno.EIO: break
                            raise
                p.wait(timeout=5)
                if p.stdout:
                    captured = p.stdout.read(); p.stdout.close()
                else:
                    captured = b''
                logs = '\n'.join(f.read_text() for f in (directory/'logs').glob('install-*.log') if not f.is_symlink())
                calls = (directory/'calls').read_text().count('called') if (directory/'calls').exists() else 0
                transcript = (output+captured).decode()
                self.assertIn('P0-CODE-1234', transcript)
                if scenario in ('expired', 'deadline', 'denied', 'exchange-failed'):
                    self.assertEqual(p.returncode, 1, captured.decode())
                    self.assertIn('Manual provisioning fallback', captured.decode())
                    self.assertFalse((directory/'fake.creds').exists())
                else:
                    self.assertEqual(p.returncode, 0, transcript)
                    self.assertIn('FLOW_COMPLETED', transcript)
                    self.assertEqual(calls, {'approved': 3, 'expired-once': 5, 'slow-pending': 6}[scenario])
                    self.assertIn('SEEDED-CREDS-CONTENT', (directory/'fake.creds').read_text())
                    self.assertIn('[REDACTED:device-code]', logs)
                auths = (directory/'auths').read_text().count('auth')
                self.assertEqual(auths, {'expired': 3, 'deadline': 3, 'expired-once': 2}.get(scenario, 1))
                expected = {'approved': ['approved'], 'expired': ['expired'],
                            'deadline': ['expired'],
                            'expired-once': ['expired', 'approved'], 'denied': ['denied'],
                            'slow-pending': ['pending', 'slow_down', 'approved'],
                            'exchange-failed': ['approved', 'exchange-failed']}[scenario]
                for state in expected:
                    self.assertIn('device-flow outcome='+state, logs)
                    self.assertNotIn('device-flow outcome='+state, transcript)
                self.assertIn('remaining', transcript)
                if scenario == 'slow-pending':
                    self.assertEqual(logs.count('device-flow outcome=pending'), 1)
                    self.assertEqual((directory/'sleeps').read_text().splitlines(), ['0','0','0','5'])
                if scenario == 'deadline':
                    self.assertFalse((directory/'polls').exists(), 'do not poll an expired code')
                for secret in ['P0-CODE-1234', 'SEEDED-DEVICE-SECRET', 'SEEDED-CREDS-CONTENT', 'eyJhbGci']:
                    self.assertNotIn(secret, logs)
                self.assertNotIn('\x1b', logs)
            finally:
                os.close(master)
                if slave >= 0: os.close(slave)

    def test_piped_bootstrap_with_controlling_terminal(self):
        self.oauth('controlling')

    def test_piped_installer_with_stdout_only_terminal(self):
        self.oauth('stdout', handoff=False)

    def test_piped_installer_with_only_controlling_terminal(self):
        self.oauth('tty-only', handoff=False)

    def test_no_tty_still_displays_link_and_code(self):
        self.oauth('none')

    def test_expired_code_reissues_in_place(self):
        self.oauth('none', scenario='expired-once')

    def test_expiry_exhausts_three_codes(self):
        self.oauth('none', scenario='expired')

    def test_pending_and_slowdown_transitions(self):
        self.oauth('none', scenario='slow-pending')

    def test_denied_does_not_retry(self):
        self.oauth('none', scenario='denied')

    def test_approved_but_exchange_failed_is_logged(self):
        self.oauth('none', scenario='exchange-failed')

    def test_local_deadline_reissues_without_polling_expired_code(self):
        self.oauth('none', scenario='deadline')

    def test_xtrace_does_not_log_codes_or_tokens(self):
        self.oauth('none', trace=True)


if __name__ == '__main__':
    unittest.main(verbosity=2)
