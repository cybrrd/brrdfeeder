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
PATTERN = re.compile(r"\b(?:[s]ynth|[c]y|[j]amie|[k]imi|[c]odex|[g]lm|[g]emini|"
    r"[c]airn|[t]ally|[f]athom|[c]olophon|[c]rucible|[c]laude(?! Code CLI)|"
    r"[t]yler|[e]ric|[s]agan|[s]aker|(?<!round-)[r]obin|[c]ardinal|[c]athartes|"
    r"the [p]ack|[p]ack-(?:consensus|canonical|ratified)|#185 [D]rop 2|"
    r"BRRDfeeder [O]pen tier)\b", re.I)
# Reviewed recognition-only exceptions: (exact path, SHA256 of stripped line).
# No directory-wide or whole-word exemptions. The inventory is code reviewed.
ALLOWLIST = {}

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
