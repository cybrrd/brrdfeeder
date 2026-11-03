#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Exercise the installer's actual enrollment exchange with offline effectors.

CYB1 9736af2754616f90fe98c5a62896b71a893312f2, handlers.go:197,
emits only the owner-capacity refusal header. Deregistration, review, role,
revocation-state and other failures have no typed header at that source.
"""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

INSTALL = Path(__file__).resolve().parents[2] / 'brrdfeeder-install.sh'
TEXT = INSTALL.read_text()
EXCHANGE = TEXT.split('  # ---- Step 4/5: Option A exchange')[1].split('\n}\n')[0]
EXCHANGE = '  # ---- Step 4/5: Option A exchange' + EXCHANGE
CALLER_START = TEXT.index('\nif [[ ! -f "$CREDS_PATH" ]]; then', TEXT.index('zitadel_device_flow() {'))
CALLER = TEXT[CALLER_START:TEXT.index('\nrun chmod 0640 "$CREDS_PATH"', CALLER_START)]
MANUAL = TEXT[TEXT.index('manual_creds_instructions() {'):TEXT.index('zitadel_device_flow() {')]
GENERIC = 'Account approval succeeded, but the feeder could not finish linking. Please retry.'
CAPACITY = 'This account has reached its node limit'
HELP = 'https://github.com/cybrrd/brrdfeeder/issues/new?template=install-help.yml'
BODY = ('Owner capacity reached: this account has 1 of 1 node slots. '
        'No new node was enrolled. Existing nodes can still renew.')


def exchange(headers, body=BODY, status='403', caller=False, dry_run=False, existing=False):
    with tempfile.TemporaryDirectory(prefix='enrollment-refusal-') as td:
        root = Path(td)
        (root / 'headers').write_bytes(headers.encode())
        (root / 'body').write_text(body)
        (root / 'tmp').mkdir()
        if existing:
            (root / 'creds').write_text('EXISTING_CREDS_SENTINEL')
        script = r'''
set -euo pipefail
gate() { :; }
log_event() { :; }
poll_outcome() { printf 'outcome:%s\n' "$1"; }
warn() { printf '%s\n' "$*" >&2; }
ok() { :; }
say() { printf '%s\n' "$*"; }
fatal() { printf 'FATAL %s\n' "$*" >&2; exit 1; }
run() { "$@"; }
_restore_xtrace() { printf 'xtrace-restored\n'; }
hostname() { printf 'fixture-node\n'; }
curl() {
  local destination=''
  while (( $# )); do
    if [[ $1 == -D ]]; then destination=$2; shift; fi
    shift
  done
  test -n "$destination"
  cat "$FIXTURE_ROOT/headers" > "$destination"
  cat "$FIXTURE_ROOT/body"
  printf '\n%s' "$FIXTURE_STATUS"
}
atomic_install() {
  # The success path keeps its credential write; refusal must never reach it.
  printf '%s\n' "$*" >> "$FIXTURE_ROOT/writes"
  cat > "$FIXTURE_ROOT/saved-body"
}
access_token='TOKEN_SENTINEL_NEVER_DISPLAY'
refresh_token=''
FLOCK_ENROLL_URL='https://fixture.invalid/not-contacted'
CREDS_PATH="$FIXTURE_ROOT/creds"
CONFIG_PATH="$FIXTURE_ROOT/config"
TARGET_GID=1234
exchange_fixture() {
''' + EXCHANGE + '\n}\n'
        if caller:
            script += MANUAL + '\nzitadel_device_flow() { exchange_fixture; }\n' + CALLER + '\nprintf \'CALLER_CONTINUED\\n\'\n'
        else:
            script += 'exchange_fixture\n'
        env = dict(os.environ, FIXTURE_ROOT=td, FIXTURE_STATUS=status,
                   TMPDIR=str(root / 'tmp'), DRY_RUN='1' if dry_run else '0')
        result = subprocess.run(['bash', '-c', script], env=env,
                                capture_output=True, text=True, timeout=10)
        writes = (root / 'writes').read_text() if (root / 'writes').exists() else ''
        saved = (root / 'saved-body').read_text() if (root / 'saved-body').exists() else ''
        remaining = list((root / 'tmp').iterdir())
        return result, writes, saved, remaining


class EnrollmentRefusal(unittest.TestCase):
    def assert_refusal(self, headers, body=BODY, status='403', typed=False):
        result, writes, saved, remaining = exchange(headers, body, status)
        self.assertEqual(result.returncode, 77 if typed else 1, result.stderr)
        self.assertEqual(writes, '')
        self.assertEqual(saved, '')
        self.assertEqual(remaining, [], 'temporary headers not removed')
        self.assertIn('outcome:exchange-failed', result.stdout)
        self.assertIn('xtrace-restored', result.stdout)
        if typed:
            self.assertIn(CAPACITY, result.stderr)
            self.assertIn(HELP, result.stderr)
            self.assertIn('admin@cybrrd.com', result.stderr)
            self.assertNotIn(GENERIC, result.stderr)
        else:
            self.assertEqual(result.stderr.strip(), GENERIC)
        for secret in ['TOKEN_SENTINEL', 'DEVICE_CODE_SENTINEL', 'NKEY SEED',
                       'CREDENTIAL_SENTINEL', 'SERVER_DIAGNOSTIC_SENTINEL']:
            self.assertNotIn(secret, result.stdout + result.stderr)
        return result

    def test_emitted_owner_capacity_type(self):
        result = self.assert_refusal(
            'HTTP/1.1 403 Forbidden\r\nX-Cybrrd-Enrollment-Refusal: owner-capacity\r\n\r\n',
            typed=True)
        self.assertIn('1 of 1 node slots', result.stderr)
        self.assertIn('No new node was enrolled', result.stderr)

    def test_case_insensitive_header_and_optional_whitespace(self):
        self.assert_refusal('HTTP/2 403\r\nx-CYBRRD-enrollment-REFUSAL:\towner-capacity \t\r\n\r\n', typed=True)

    def test_only_final_response_header_is_used(self):
        interim = 'HTTP/1.1 100 Continue\r\nX-Cybrrd-Enrollment-Refusal: owner-capacity\r\n\r\n'
        self.assert_refusal(interim + 'HTTP/1.1 403 Forbidden\r\n\r\n')
        self.assert_refusal(interim + 'HTTP/1.1 403 Forbidden\r\nX-Cybrrd-Enrollment-Refusal: owner-capacity\r\n\r\n', typed=True)

    def test_unknown_absent_and_unemitted_types_stay_generic(self):
        # These example types are not emitted by the pinned Flock handler.
        for value in ['', 'revoked', 'deregistered', 'manual-review', 'future-type', 'OWNER-CAPACITY']:
            with self.subTest(value=value):
                header = f'X-Cybrrd-Enrollment-Refusal: {value}\r\n' if value else ''
                self.assert_refusal('HTTP/1.1 403 Forbidden\r\n' + header + '\r\n')

    def test_duplicate_headers_fail_closed(self):
        for other in ['owner-capacity', 'future-type']:
            self.assert_refusal('HTTP/1.1 403 Forbidden\r\nX-Cybrrd-Enrollment-Refusal: owner-capacity\r\n'
                                f'X-Cybrrd-Enrollment-Refusal: {other}\r\n\r\n')

    def test_wrong_status_stays_generic(self):
        for status in ['000', '401', '429', '500', '503']:
            with self.subTest(status=status):
                self.assert_refusal(f'HTTP/1.1 {status} Failure\r\nX-Cybrrd-Enrollment-Refusal: owner-capacity\r\n\r\n', status=status)

    def test_bounded_body_never_becomes_customer_output(self):
        header = 'HTTP/1.1 403 Forbidden\r\nX-Cybrrd-Enrollment-Refusal: owner-capacity\r\n\r\n'
        for body in [BODY + '\nSERVER_DIAGNOSTIC_SENTINEL TOKEN_SENTINEL DEVICE_CODE_SENTINEL',
                     '-----BEGIN NATS USER JWT-----\nCREDENTIAL_SENTINEL\nNKEY SEED',
                     'x' * 8192 + BODY, '',
                     'Owner capacity reached: this account has TOKEN_SENTINEL of 1 node slots.']:
            with self.subTest(body_length=len(body)):
                result = self.assert_refusal(header, body, typed=True)
                self.assertLess(len(result.stderr), 800)
                if not body.startswith(BODY):
                    self.assertNotIn('1 of 1 node slots', result.stderr)

    def test_header_injection_and_long_unknown_value_stay_generic(self):
        for value in ['owner-capacity; TOKEN_SENTINEL', 'x' * 8192]:
            self.assert_refusal(f'HTTP/1.1 403 Forbidden\r\nX-Cybrrd-Enrollment-Refusal: {value}\r\n\r\n')

    def test_success_keeps_credential_write_and_no_refusal_message(self):
        body = '-----BEGIN NATS USER JWT-----\nSYNTHETIC_SUCCESS_ONLY\n'
        result, writes, saved, remaining = exchange('HTTP/1.1 200 OK\r\n\r\n', body, '200')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(writes)
        self.assertEqual(saved, body)
        self.assertEqual(remaining, [])
        self.assertNotIn(CAPACITY, result.stderr)
        self.assertNotIn('exchange-failed', result.stdout)

    def test_capacity_stops_actual_caller_without_manual_fallback(self):
        headers = 'HTTP/1.1 403 Forbidden\r\nX-Cybrrd-Enrollment-Refusal: owner-capacity\r\n\r\n'
        result, writes, saved, remaining = exchange(headers, caller=True)
        self.assertEqual(result.returncode, 77, result.stderr)
        self.assertIn(CAPACITY, result.stderr)
        self.assertIn('admin@cybrrd.com', result.stderr)
        self.assertIn('slot frees', result.stderr)
        for unwanted in ('Manual provisioning fallback', 'scp brrdfeeder.creds',
                         'Credentials required', 'CALLER_CONTINUED', 'TOKEN_SENTINEL'):
            self.assertNotIn(unwanted, result.stdout + result.stderr)
        self.assertEqual((writes, saved, remaining), ('', '', []))

    def test_untyped_or_other_error_keeps_existing_manual_fallback(self):
        for status in ('403', '500'):
            result, writes, saved, remaining = exchange(f'HTTP/1.1 {status} Failure\r\n\r\n', status=status, caller=True)
            self.assertEqual(result.returncode, 1)
            self.assertIn(GENERIC, result.stderr)
            self.assertIn('Manual provisioning fallback', result.stderr)
            self.assertIn('scp brrdfeeder.creds', result.stderr)
            self.assertIn('Credentials required', result.stderr)
            self.assertNotIn('CALLER_CONTINUED', result.stdout)
            self.assertEqual((writes, saved, remaining), ('', '', []))

    def test_success_continues_actual_caller(self):
        body = '-----BEGIN NATS USER JWT-----\nSYNTHETIC_SUCCESS_ONLY\n'
        result, writes, saved, remaining = exchange('HTTP/1.1 200 OK\r\n\r\n', body, '200', caller=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('CALLER_CONTINUED', result.stdout)
        self.assertNotIn('Manual provisioning fallback', result.stderr)
        self.assertTrue(writes)
        self.assertEqual(saved, body)
        self.assertEqual(remaining, [])

    def test_dry_run_and_existing_credentials_do_not_enroll(self):
        for opts in ({'dry_run': True}, {'existing': True}):
            result, writes, saved, remaining = exchange('', caller=True, **opts)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('CALLER_CONTINUED', result.stdout)
            self.assertNotIn('outcome:', result.stdout)
            self.assertNotIn('Manual provisioning fallback', result.stderr)
            self.assertEqual((writes, saved, remaining), ('', '', []))


if __name__ == '__main__':
    unittest.main()
