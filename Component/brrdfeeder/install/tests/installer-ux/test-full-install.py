#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Full installer in a disposable offline OS, using the existing naming fixture.

Real installer, accounts, files and logger; synthetic hardware, OAuth, registry,
systemd and updater. This is a transcript fixture, NOT a live-node acceptance.
"""
import errno
import grp
import importlib.util
import os
from pathlib import Path
import pty
import select
import subprocess
import time
import unittest

assert os.geteuid() == 0 and Path('/run/.containerenv').exists()
assert os.environ.get('BRRD_INSTALLER_UX_CONTAINER') == '1'
ROOT = Path('/repo')
SOURCE_ROOT = Path(os.environ.get('UX_SOURCE_ROOT', '/repo'))
BASELINE = os.environ.get('UX_BASELINE') == '1'
os.environ['BRRD_NAMING_CONTAINER'] = '1'
spec = importlib.util.spec_from_file_location('naming', ROOT/'Component/brrdfeeder/install/tests/brrdfeeder-naming/container-tests.py')
naming = importlib.util.module_from_spec(spec); spec.loader.exec_module(naming)
naming.INSTALLER = SOURCE_ROOT/'Component/brrdfeeder/install/brrdfeeder-install.sh'


class Transcript(unittest.TestCase):
    def test_full_install_and_redacted_diagnostics(self):
        fixture = naming.InstallerRerun('test_rerun_keeps_current_new_dropin')
        fixture.setUp(); self.addCleanup(fixture.tearDown)
        if not BASELINE:
            # Ubuntu's system log layout, in this disposable OS only.
            if not any(group.gr_name == 'syslog' for group in grp.getgrall()):
                subprocess.run(['groupadd', '--system', 'syslog'], check=True)
            os.chown('/var/log', 0, grp.getgrnam('syslog').gr_gid)
            os.chmod('/var/log', 0o775)
        fixture.seed_installed_node()
        # Start enrollment with an existing template and inert installed helper.
        # The fixture must never download or execute a real update helper.
        Path('/etc/brrdfeeder/secrets/brrdfeeder.creds').unlink()
        (fixture.bin/'jq').unlink()  # use the image's real JSON parser
        (fixture.bin/'curl').write_text('''#!/usr/bin/python3
import json, sys
from pathlib import Path
args = sys.argv[1:]
if any('device_authorization' in a for a in args):
    print(json.dumps(dict(device_code='FIXTURE-DEVICE', user_code='FIXTURE-CODE',
        verification_uri_complete='https://oauth.invalid/verify?user_code=FIXTURE-CODE', expires_in=300, interval=0)))
elif any('oauth/v2/token' in a for a in args):
    print('{"access_token":"FIXTURE-ACCESS-TOKEN"}\\n200')
elif '-D' in args:
    Path(args[args.index('-D')+1]).write_text('X-Cybrrd-Node-Id: fixture-node\\r\\n')
    print('-----BEGIN NATS USER JWT-----\\nFIXTURE-CREDS\\n-----END NATS USER JWT-----\\n200')
elif any(a.startswith('http://127.0.0.1:8080') for a in args):
    pass
else: sys.exit(99)
''')
        master, slave = pty.openpty()
        command = ['bash', str(naming.INSTALLER), '--image', naming.PIN, '--console-image',
                   'ghcr.io/cybrrd/brrdhouse@sha256:'+'c'*64, '--console-listen', '127.0.0.1:8080']
        process = subprocess.Popen(command, env={**fixture.env, 'BRRDFEEDER_RUN_ID':'1234abcd'},
                                   stdin=subprocess.DEVNULL, stdout=slave, stderr=slave, start_new_session=True)
        os.close(slave)
        output = bytearray(); deadline = time.monotonic()+120
        try:
            while True:
                if time.monotonic() >= deadline:
                    process.kill(); self.fail('full installer fixture timed out')
                if select.select([master], [], [], .1)[0]:
                    try: chunk = os.read(master, 65536)
                    except OSError as error:
                        if error.errno == errno.EIO: break
                        raise
                    if not chunk: break
                    output.extend(chunk)
                elif process.poll() is not None: break
            rc = process.wait(timeout=5)
        finally:
            os.close(master)
            if process.poll() is None: process.kill(); process.wait()
        transcript = output.decode().replace('\r\n', '\n')
        log = Path('/var/log/brrdfeeder/install-latest.log').read_text()
        out = Path(os.environ.get('UX_EVIDENCE', '/tmp/ux-evidence')); out.mkdir(parents=True, exist_ok=True)
        (out/'full-install.txt').write_text(transcript)
        (out/'full-install.log').write_text(log)
        self.assertEqual(rc, 0, transcript[-5000:])
        for expected in ['INSTALL PAUSED', 'FIXTURE-CODE', 'automatically', 'Node ID: fixture-node',
                         'Console: http://127.0.0.1:8080/', 'GPS: OK', 'Network (NATS): OK',
                         'Wi-Fi capture: OK', 'Bluetooth: OK', 'sudo brrdfeeder support-bundle']:
            self.assertIn(expected, transcript)
        for secret in ['FIXTURE-CODE', 'FIXTURE-DEVICE', 'FIXTURE-ACCESS-TOKEN', 'FIXTURE-CREDS']:
            self.assertNotIn(secret, log)
        if not BASELINE:
            for hidden in ['Zitadel', 'NOT signature verified', 'storage_class', 'console_memory_limit=',
                           'cgroup=', 'console has no memory cap', 'Next: follow README', 'checks signed updates automatically']:
                self.assertNotIn(hidden, transcript)
            metadata = Path('/var/log/brrdfeeder').stat()
            self.assertEqual((metadata.st_uid, metadata.st_gid, metadata.st_mode & 0o7777), (0, 0, 0o750))
            self.assertNotIn('logging degraded', transcript)
            for diagnostic in ['Zitadel', 'NOT signature verified', 'storage_class',
                               'console_memory_limit=', 'device-flow outcome=approved']:
                self.assertIn(diagnostic, log)
            self.assertIn('this version does not update itself', transcript)
            self.assertIn('sudo brrdfeeder uninstall, then run the install command again', transcript)
        else:
            self.assertIn('Zitadel Device Flow', transcript)
            self.assertIn('Next: follow README', transcript)
            self.assertIn('checks signed updates automatically', transcript)


if __name__ == '__main__': unittest.main(verbosity=2)
