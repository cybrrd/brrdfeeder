#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Execute the actual enrollment prompt with synthetic responses and no network."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

INSTALL = Path(__file__).resolve().parents[2]
BARE = 'https://hospitality.cybrrd.com/device'
COMPLETE = BARE + '?user_code=TEST-1234'
COMPARE = 'Approve ONLY if the code shown in the browser exactly matches the code shown on this node.'
WARNING = 'If you did not start this install, do not approve.'

class DevicePrompt(unittest.TestCase):
    def prompt(self, **links):
        source = (INSTALL/'brrdfeeder-install.sh').read_text()
        start = source.index('  # ---- Step 1: request device authorization')
        end = source.index('  # ---- Step 3: poll the token endpoint', start)
        script = '''set -eu
curl() { cat "$FIXTURE_RESPONSE"; }
warn() { echo "$*"; }
log_secret() { :; }
_restore_xtrace() { :; }
prompt() {
local attempt=1 B='' Y='' G='' R=''
''' + source[start:end] + '\n}\nprompt\n'
        with tempfile.TemporaryDirectory() as tmp:
            response = Path(tmp)/'response.json'
            response.write_text(json.dumps(dict(device_code='fixture-device-secret',
                user_code='TEST-1234', expires_in=300, interval=5, **links)))
            return subprocess.run(['bash','-c',script], capture_output=True, text=True,
                env=dict(os.environ,FIXTURE_RESPONSE=str(response),OAUTH_ISSUER='https://example.invalid',
                         OAUTH_CLIENT_ID='fixture',OAUTH_SCOPE='openid'),timeout=5)
    def check_prompt(self, links, expected):
        result = self.prompt(**links)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(expected, result.stdout)
        self.assertIn('TEST-1234', result.stdout)
        self.assertIn(COMPARE, result.stdout)
        self.assertIn(WARNING, result.stdout)
        self.assertNotIn('fixture-device-secret', result.stdout)
        return result.stdout
    def test_bare_preferred_with_both_links(self):
        text = self.check_prompt(dict(verification_uri=BARE,verification_uri_complete=COMPLETE),BARE)
        self.assertNotIn(COMPLETE,text)
    def test_bare_only(self):
        self.check_prompt(dict(verification_uri=BARE),BARE)
    def test_complete_only_fallback(self):
        self.check_prompt(dict(verification_uri_complete=COMPLETE),COMPLETE)
    def test_missing_link_refused(self):
        result=self.prompt()
        self.assertNotEqual(result.returncode,0)
        self.assertIn('no approval link',result.stdout)

if __name__ == '__main__': unittest.main(verbosity=2)
