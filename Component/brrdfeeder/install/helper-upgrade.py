#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Embedded opt-in dispatch: pin authority is the verified installer itself.

Only the SHA-addressed public artifact may be fetched. No identity endpoints,
ordinary installer preparation, package installation or enrollment fallback.
"""
import hashlib
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile


def main(args):
    if os.geteuid() != 0:
        raise RuntimeError("helper upgrade requires root via the verified bootstrap")
    if len(args) not in (2, 4) or args[1] != '--upgrade-helper' or (len(args) == 4 and args[2] != '--helper-file'):
        raise RuntimeError("usage: --upgrade-helper [--helper-file PATH]")
    if platform.machine() not in ('aarch64', 'arm64'):
        raise RuntimeError("the pinned helper requires ARM64")
    pins = re.findall(r'^readonly RELEASE_HELPER_SHA256="([a-f0-9]{64})"$', Path(args[0]).read_text(), re.M)
    if len(pins) != 1:
        raise RuntimeError("verified installer helper pin unavailable")
    expected = pins[0]
    # Root-private staging, not an operator-writable executable pathname.
    with tempfile.TemporaryDirectory(prefix='brrdfeeder-helper-', dir='/run') as stage:
        candidate = Path(stage) / 'brrdfeeder-release'
        if len(args) == 4:
            with open(args[3], 'rb') as source:
                raw = source.read((32 << 20) + 1)
            if len(raw) > 32 << 20:
                raise RuntimeError("helper exceeds size limit")
            candidate.write_bytes(raw)
        else:
            url = 'https://get.cybrrd.com/releases/v1/updater/' + expected + '/linux-arm64/brrdfeeder-release'
            subprocess.run(['/usr/bin/curl', '--fail', '--silent', '--show-error', '--proto', '=https',
                            '--tlsv1.2', '--connect-timeout', '10', '--max-time', '120',
                            '--max-filesize', '33554432', '--output', str(candidate), url], check=True)
        raw = candidate.read_bytes()
        if len(raw) > 32 << 20 or hashlib.sha256(raw).hexdigest() != expected:
            raise RuntimeError("helper SHA256 mismatch; no installed state changed")
        if len(raw) < 20 or raw[:6] != b'\x7fELF\x02\x01' or raw[18:20] != b'\xb7\x00':
            raise RuntimeError("wrong helper architecture; refusing execution")
        candidate.chmod(0o700)
        subprocess.run([str(candidate), 'upgrade-local-helper', '--sha256', expected], check=True)


if __name__ == '__main__':
    try:
        main(sys.argv[1:])
    except (OSError, RuntimeError, subprocess.SubprocessError):
        # Never echo credentials, caller-controlled filenames or config errors.
        print('Helper upgrade refused or failed; existing identity was not changed. No enrollment attempted.', file=sys.stderr)
        sys.exit(1)
