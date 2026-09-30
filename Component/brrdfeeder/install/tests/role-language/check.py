#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Reject project-person attributions in shipped text; inspect staged new paths too."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[5]
# Initial character classes keep the policy's regex definitions distinct from
# actual prose. Product examples and the cyclic scheduling term are not people.
PATTERN = re.compile(r"(?<!\w)(?:[s]ynth|[c]y|[j]amie|[k]imi|[c]odex|[g]lm|[g]emini|"
    r"[c]airn|[t]ally|[f]athom|[c]olophon|[c]rucible|[c]laude(?! Code CLI\b)|"
    r"[t]yler|[e]ric|[s]agan|[s]aker|(?<!round-)[r]obin|[c]ardinal|[c]athartes|"
    r"the [p]ack|[p]ack-(?:consensus|canonical|ratified)|#185 [D]rop 2|"
    r"BRRDfeeder [O]pen tier)\b", re.I)
# Reviewed recognition-only exceptions: (exact path, SHA256 of stripped line).
# No directory-wide or whole-word exemptions. The inventory is code reviewed.
ALLOWLIST = {
    ('Component/aviary/cybrrd-rid-protocol/src/astm.rs',
     '5cccbb03cb873909e862f09438cf8eb254517fca7470a9146feb03e773f5517c'): 'Rule 2: ASTM protocol terminology or its negative-control fixture',
    ('Component/aviary/deploy/bootstrap/brrdfeeder-install.sh',
     '456a4e72ea8b5ce3b7d5ef8964dee4537c13ad0fa0a9429d1f0b41c9c6616481'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/aviary/engine/src/hunter.rs',
     'f2765ae1a9fb48f634c52fec9fc9afd215e58a573c555730378258f0956ec0b4'): 'Rule 2: ASTM protocol terminology or its negative-control fixture',
    ('Component/aviary/tools/test-installer-quadlet.sh',
     '8a8fa33f2a7c6352fb43c33618091d9849ea01aa54865d02d6096f9437af3cee'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/brrdfeeder-install.sh',
     '8afb411b4dfbe68e728de815703a1d80da708500ff19a2ac4a8d6b7a9bd548b7'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/brrdfeeder-install.sh',
     'db0e0376f503afcaccf1c7481a7c1eaeaae9487acfaff8c0f5c0dd7a0577b7e6'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/brrdfeeder-install.sh',
     '517981d10c780d25987b40f9b20e71876202a13d7849f057d25a374c816b24bf'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/brrdfeeder-install.sh',
     'a7a3888ed52e4cf61c3c4ac4714ed4f4a6ca1a20ea4633a8f3c6c6e4b125327e'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/brrdfeeder-install.sh',
     '7fe8dbfffcc84c33d04be6a4216a5e39738e9ac14e4f6bd25f72bdfa2a83dcd4'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/brrdfeeder-install.sh',
     '727e8ac949c74be1f8a2c35e6f833c4a8d65aa1a282b52662a255155a71a285e'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/brrdfeeder-install.sh',
     'd3c70a15c7a3bc269781316c6635a5d6e2954bdc145d5aed7bab4cd4ba6159aa'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/tests/brrdfeeder-naming/container-tests.py',
     '44ca56940ed0d82dee9f82bbb3df7795251b57c6a80745a2d06eca77ce163f83'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/tests/brrdfeeder-naming/test-contract.py',
     'fefdb2235f6888f2a388f36d7d2683f8435124f659690fe9d8f07e121949c572'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/tests/brrdfeeder-naming/test-contract.py',
     'f4dca03827ba603b1687484071a55746baff81ee06f187c4915532476ff49785'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/tests/installer-vcgencmd/test-safety.py',
     '4186c15d382226b515e38a4d8946a84518a87658c9fb259990ed3f7a8c37e234'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/tests/installer-vcgencmd/test-safety.py',
     '6f0326686af4dd279e4cde64802f3d13793b101fccc5f443f0b8a14a2b37a468'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/tests/installer-vcgencmd/test-safety.py',
     '29ef1935e43aaacbb4cc705356e30c87b3e24bc21be98215ee084f26eb79539c'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/tests/uninstall/test-contract.py',
     'ee4eda4935ca1c4dbbd44d48bb6e605a619ba466f9e7f7cfc8a1e36d4791711f'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/uninstall.sh',
     'db0e0376f503afcaccf1c7481a7c1eaeaae9487acfaff8c0f5c0dd7a0577b7e6'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/uninstall.sh',
     '517981d10c780d25987b40f9b20e71876202a13d7849f057d25a374c816b24bf'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/uninstall.sh',
     'a7a3888ed52e4cf61c3c4ac4714ed4f4a6ca1a20ea4633a8f3c6c6e4b125327e'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/uninstall.sh',
     '7fe8dbfffcc84c33d04be6a4216a5e39738e9ac14e4f6bd25f72bdfa2a83dcd4'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
    ('Component/brrdfeeder/install/uninstall.sh',
     '727e8ac949c74be1f8a2c35e6f833c4a8d65aa1a282b52662a255155a71a285e'): 'Rule 3: input-side legacy recognition or input compatibility fixture',
}

def inspect(path, data):
    if b'\0' in data:
        return []
    findings = []
    for number, line in enumerate(data.decode('utf-8', errors='replace').splitlines(), 1):
        hits = list(PATTERN.finditer(line))
        if not hits:
            continue
        digest = hashlib.sha256(line.strip().encode()).hexdigest()
        if (path, digest) in ALLOWLIST:
            continue
        findings.append(dict(path=path, line=number, words=[m.group() for m in hits]))
    return findings

def scan(root, revision=None):
    def git(*args):
        return subprocess.check_output(['git', *args], cwd=root)
    # Private checkout: only the public export is in scope. Public checkout:
    # every tracked file, including new index entries, is mandatory.
    manifest = root / 'governance/github-migration/public-cut/public-scope-paths.txt'
    scope = manifest.read_text().splitlines() if manifest.exists() else []
    if revision:
        paths = git('ls-tree', '-rz', '--name-only', revision, '--', *scope).split(b'\0')
    else:
        paths = git('ls-files', '-z', '--', *scope).split(b'\0')
    findings = []
    for raw in paths:
        if not raw:
            continue
        path = raw.decode()
        data = git('show', revision + ':' + path) if revision else (root / path).read_bytes()
        findings.extend(inspect(path, data))
    return findings

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--revision')
    args = parser.parse_args()
    findings = scan(args.root, args.revision)
    print(json.dumps(dict(findings=findings, count=len(findings)), indent=2))
    raise SystemExit(bool(findings))
