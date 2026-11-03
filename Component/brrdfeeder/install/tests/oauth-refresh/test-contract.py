#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Execute the real device flow with synthetic provider responses; no network."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[5]
INSTALL = Path(os.environ.get('OAUTH_TEST_INSTALLER', ROOT/'Component/brrdfeeder/install/brrdfeeder-install.sh'))
TEXT = INSTALL.read_text()
FLOW = TEXT.split('zitadel_device_flow() {', 1)[1].split('\n}\n', 1)[0]
SCOPE = re.search(r'^readonly OAUTH_SCOPE="(.*)"', TEXT, re.M)[1]

class RefreshPolicy(unittest.TestCase):
    def test_callers_do_not_request_offline_access(self):
        self.assertEqual(SCOPE, 'openid profile')
        recovery = (ROOT/'Component/aviary/engine/src/credential_renewal.rs').read_text()
        self.assertIn('("scope", "openid profile")', recovery)
        self.assertNotIn('offline_access', recovery)

    def test_enrollment_ignores_refresh_reply_and_preserves_old_file(self):
        for refresh in [None, 'REFRESH_SECRET_SENTINEL']:
            for existing in [False, True]:
                with self.subTest(refresh=refresh is not None, existing=existing), tempfile.TemporaryDirectory() as td:
                    root = Path(td)
                    if existing: (root/'oauth_refresh.token').write_text('OLD_TOKEN_SENTINEL\n')
                    reply = {'access_token': 'ACCESS_SECRET_SENTINEL', 'id_token': 'ID_SECRET_SENTINEL'}
                    if refresh: reply['refresh_token'] = refresh
                    (root/'reply').write_text(json.dumps(reply))
                    script = r'''
set -euo pipefail
cd "$FIXTURE_ROOT"
OAUTH_ISSUER=https://provider.invalid
OAUTH_CLIENT_ID=fixture
FLOCK_ENROLL_URL=https://flock.invalid/enroll
CREDS_PATH="$FIXTURE_ROOT/creds"
REFRESH_TOKEN_PATH="$FIXTURE_ROOT/oauth_refresh.token"
CONFIG_PATH="$FIXTURE_ROOT/config"
TARGET_GID=1234
OAUTH_SCOPE=''' + repr(SCOPE) + r'''
gate() { :; }
log_event() { :; }
log_secret() { :; }
warn() { printf '%s\n' "$*" >&2; }
ok() { printf '%s\n' "$*"; }
say() { printf '%s\n' "$*"; }
run() { "$@"; }
sleep() { :; }
hostname() { printf fixture-node; }
curl() {
  printf '%s\n' "$*" >> "$FIXTURE_ROOT/requests"
  case "$*" in
    *device_authorization*) printf '%s' '{"device_code":"DEVICE_SECRET_SENTINEL","user_code":"VISIBLE_CODE","verification_uri":"https://provider.invalid/approve","expires_in":300,"interval":0}' ;;
    */oauth/v2/token*) cat "$FIXTURE_ROOT/reply"; printf '\n200' ;;
    *flock.invalid/enroll*)
      while (( $# )); do
        if [[ $1 == -D ]]; then printf 'HTTP/1.1 200 OK\r\n' > "$2"; break; fi
        shift
      done
      printf '%s\n200' '-----BEGIN NATS USER JWT-----
NATS_CREDENTIAL_SENTINEL' ;;
    *) return 99 ;;
  esac
}
atomic_install() {
  test "$1 $2 $3" = '0640 root 1234'
  test "$4" = "$CREDS_PATH"
  cat > "$4"
}
zitadel_device_flow() {
''' + FLOW + '\n}\nset -x\nzitadel_device_flow\n'
                    result = subprocess.run(['bash', '-c', script], env={**os.environ, 'FIXTURE_ROOT': td}, capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIn('NATS_CREDENTIAL_SENTINEL', (root/'creds').read_text())
                    requests = (root/'requests').read_text()
                    self.assertIn('scope=openid profile', requests)
                    self.assertIn('Authorization: Bearer ACCESS_SECRET_SENTINEL', requests)
                    self.assertNotIn('offline_access', requests)
                    output = result.stdout + result.stderr
                    for secret in ['ACCESS_SECRET_SENTINEL', 'REFRESH_SECRET_SENTINEL', 'ID_SECRET_SENTINEL', 'DEVICE_SECRET_SENTINEL', 'NATS_CREDENTIAL_SENTINEL', 'OLD_TOKEN_SENTINEL']:
                        self.assertNotIn(secret, output)
                    self.assertNotIn('enables non-interactive creds renewal', output)
                    old = root/'oauth_refresh.token'
                    if existing: self.assertEqual(old.read_text(), 'OLD_TOKEN_SENTINEL\n')
                    else: self.assertFalse(old.exists())

if __name__ == '__main__': unittest.main(verbosity=2)
