#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Actual shell blocks and supervisor, isolated commands/files; never installs."""
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[5]
INSTALL = ROOT / 'Component/brrdfeeder/install'
SOURCE = (INSTALL / 'brrdfeeder-install.sh').read_text()
OUT = Path(os.environ.get('INSTALLER_PROOF_OUT', str(Path(__file__).parent / 'evidence')))


class Contract(unittest.TestCase):
    def run_fixture(self, body, *, firmware=False, scenario='', deadline=8):
        OUT.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='installer-result-proof-') as directory:
            d = Path(directory)
            stub = d / 'vcgencmd'
            stub.write_text('#!/bin/bash\ntrap "" TERM\necho $$ > "$PROBE_PID"\nwhile :; do sleep 30; done\n')
            stub.chmod(0o755)
            child = d / 'fixture.sh'
            child.write_text('set -euo pipefail\necho $$ > "$CHILD_PID"\nsource ' + str(INSTALL / 'log-events.sh') + '\n' + '''
say() { echo "$*"; }
warn() { echo " !! $*"; }
ok() { echo " OK $*"; }
fatal() { echo "FATAL $*"; exit 1; }
gate() { LOG_PHASE=$1; shift; log_event PHASE "$LOG_PHASE"$'\\t'"$*"; }
run() { "$@"; }
DRY_RUN=0
CONSOLE_LISTEN=192.0.2.10:8080
STATUS_DIR=/fixture/status
CONFIG_PATH="$FIXTURE_CONFIG"
TARGET_UID=1234
TARGET_GID=1234
BLUETOOTH_HELPER=/dev/null
INSTALL_ENGINE_RESTART_AT=0
sleep() { :; }
systemctl() { if [[ $* == *SubState* ]]; then echo running; else echo active; fi; }
console_run() { "$@"; }
podman() { if [[ $* == *brrdhouse* ]]; then echo brrdhouse; else echo brrdfeeder-engine; fi; }
curl() { :; }
stat() { echo 1234:1234:755; }
chronyc() { echo 'Reference ID A123'; }
journalctl() { printf '%s\\n' 'backhaul connected' 'GPS reached Healthy' 'monitor mode established' 'Capture loop active' '[rid_ble] state=Healthy detail=receiving'; }
''' + body)
            (d / 'config.yaml').write_text('node:\n  id: brrdfeeder-fixture-001\n')
            driver = d / 'driver.py'
            driver.write_text('import importlib.util, sys\nfrom pathlib import Path\n'
                f's=importlib.util.spec_from_file_location("logger",{str(INSTALL / "install-log.py")!r})\n'
                'm=importlib.util.module_from_spec(s); s.loader.exec_module(m)\n'
                f'm.LOG_DIR=Path({str(d / "logs")!r})\n'
                'm.environment=lambda **kw: "isolated fixture"\n' +
                ('''import datetime, json, time
started=time.monotonic()
original_capture=m.capture
original_read=m.read_text
def fixture_capture(args, **kw):
    ready=(time.monotonic()-started >= (20 if SCENARIO == 'slow' else 0)) and SCENARIO != 'deadline'
    if args[0] == 'systemctl':
        return (0, 'ActiveState='+('failed' if SCENARIO == 'service-failure' else 'active')+'\\nSubState=running\\nInvocationID='+'a'*32+'\\n', 0)
    if args[0] == 'journalctl':
        assert args[-1] == '_SYSTEMD_INVOCATION_ID='+'a'*32
        return (0, '[rid_ble] state='+('Healthy' if ready else 'Initializing')+' detail=fixture', 0)
    return original_capture(args, **kw)
def fixture_read(path, *a, **kw):
    if str(path) != '/var/lib/brrdfeeder-status/status.json': return original_read(path, *a, **kw)
    ready=(time.monotonic()-started >= (20 if SCENARIO == 'slow' else 0)) and SCENARIO != 'deadline'
    return json.dumps({'schema_version':1,'written_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'status_interval_secs':30,'heartbeat':{'node_id':'brrdfeeder-fixture-001','gps':{'state':'healthy' if ready else 'initializing'},
        'radio_status':'up' if ready else 'down'},'links':{'nats_state':'connected' if ready else 'connecting'},
        'inventory':{'capture':[{'monitor_mode':ready}]}})
m.capture=fixture_capture
m.read_text=fixture_read
''' .replace('SCENARIO', repr(scenario)) if scenario else
                'm.wait_engine=lambda *a,**kw: {"running":True,"states":{k:"OK" for k in ("GPS","Network (NATS)","Wi-Fi capture","Bluetooth")},"failure":None}\n') +
                'sys.exit(m.supervise(sys.argv[1],[]))\n')
            env = dict(os.environ, PATH=str(d) + ':' + os.environ['PATH'], PROBE_PID=str(d / 'pid'),
                       CHILD_PID=str(d / 'child-pid'), FIXTURE_CONFIG=str(d / 'config.yaml'),
                       BRRDFEEDER_RUN_ID='abc12345', PYTHONDONTWRITEBYTECODE='1')
            # Do not query real firmware in the rendering-only fixture.
            if not firmware:
                stub.write_text('#!/bin/sh\necho throttled=0x0\n')
            p = subprocess.Popen(['python3', str(driver), str(child)], env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
            start = time.monotonic()
            try:
                output, _ = p.communicate(timeout=deadline)
            except subprocess.TimeoutExpired:
                os.killpg(p.pid, signal.SIGKILL)
                # The logger launches its own session; stop the fixture's firmware
                # PID explicitly too, never a name-based/broad process kill.
                if (d / 'pid').exists():
                    try: os.killpg(int((d / 'pid').read_text()), signal.SIGKILL)
                    except ProcessLookupError: pass
                p.stdout.close()
                p.wait(timeout=2)
                self.fail('caller exceeded 8-second harness deadline (firmware hang)')
            finally:
                if (d / 'child-pid').exists():
                    try: os.killpg(int((d / 'child-pid').read_text()), signal.SIGKILL)
                    except ProcessLookupError: pass
                if (d / 'pid').exists():
                    try: os.kill(int((d / 'pid').read_text()), signal.SIGKILL)
                    except ProcessLookupError: pass
            return p.returncode, output.decode(), time.monotonic() - start

    def test_power_block_returns_despite_hung_firmware(self):
        block = '# --- Power-supply sanity' + SOURCE.split('# --- Power-supply sanity', 1)[1].split('# --- Legacy migration', 1)[0]
        rc, text, elapsed = self.run_fixture(block + '\necho CONTINUED\n', firmware=True)
        (OUT / 'power-timeout.txt').write_text(text)
        self.assertEqual(rc, 0, text)
        self.assertLess(elapsed, 7)
        self.assertIn('CONTINUED', text)
        self.assertIn('power check: unknown (firmware did not answer)', text)
        self.assertIn('reboot', text.lower())

    def test_actual_installer_ending_has_clear_final_screen(self):
        body = 'gate verification' + SOURCE.split('gate verification', 1)[1]
        rc, text, _ = self.run_fixture(body)
        (OUT / 'success-screen.txt').write_text(text)
        self.assertEqual(rc, 0, text)
        self.assertIn('BRRDfeeder is installed and running.', text)
        final = text.split('BRRDfeeder is installed and running.', 1)[1]
        for value in ['brrdfeeder-fixture-001', 'http://192.0.2.10:8080/', 'GPS: OK', 'Network (NATS): OK',
                      'Wi-Fi capture: OK', 'Bluetooth: OK', 'sudo brrdfeeder status',
                      'sudo brrdfeeder support-bundle', 'sudo brrdfeeder uninstall']:
            self.assertIn(value, final)
        self.assertTrue(final.strip().endswith('(run abc12345)'))
        self.assertNotIn('Pack-discipline', text)

    def test_slow_start_waits_for_actual_twenty_second_markers(self):
        body = 'gate verification' + SOURCE.split('gate verification', 1)[1]
        rc, text, elapsed = self.run_fixture(body, scenario='slow', deadline=30)
        (OUT / 'slow-start-screen.txt').write_text(text)
        self.assertEqual(rc, 0, text)
        self.assertGreaterEqual(elapsed, 20)
        self.assertLess(elapsed, 25)
        self.assertIn('still working', text)
        self.assertNotIn('WARNING:', text)
        for name in ['GPS', 'Network (NATS)', 'Wi-Fi capture', 'Bluetooth']:
            self.assertIn(name+': OK', text)

    def test_sixty_second_deadline_warns_then_clear_running_screen(self):
        body = 'gate verification' + SOURCE.split('gate verification', 1)[1]
        rc, text, elapsed = self.run_fixture(body, scenario='deadline', deadline=68)
        (OUT / 'startup-deadline-screen.txt').write_text(text)
        self.assertEqual(rc, 0, text)
        self.assertGreaterEqual(elapsed, 60)
        self.assertLess(elapsed, 65)
        final = text.split('BRRDfeeder is installed and running.', 1)[1]
        self.assertIn('WARNING: GPS: still starting after the startup wait.', text)
        for name in ['GPS', 'Network (NATS)', 'Wi-Fi capture', 'Bluetooth']:
            self.assertIn(name+': still starting', final)
        self.assertNotIn(' !! ', final)

    def test_failed_service_never_prints_success(self):
        body = 'gate verification' + SOURCE.split('gate verification', 1)[1]
        rc, text, _ = self.run_fixture(body, scenario='service-failure')
        (OUT / 'failed-service-screen.txt').write_text(text)
        self.assertEqual(rc, 1, text)
        self.assertNotIn('is installed and running', text)
        self.assertIn('FAILED at verification/engine-startup', text)
        self.assertIn('support-bundle', text)
        self.assertTrue(text.strip().endswith('(run abc12345)'))

    def test_real_command_failure_screen(self):
        rc, text, _ = self.run_fixture("run_step images/pull-engine bash -c 'echo download-failed; exit 42'\n")
        (OUT / 'command-failure-screen.txt').write_text(text)
        self.assertEqual(rc, 42)
        self.assertNotIn('is installed and running', text)
        self.assertIn('FAILED at images/pull-engine', text)
        self.assertIn('Log:', text)
        self.assertIn('support-bundle', text)
        self.assertTrue(text.strip().endswith('(run abc12345)'))


if __name__ == '__main__':
    unittest.main(verbosity=2)
