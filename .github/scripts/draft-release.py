#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Prepare assets and create a draft only; never publish or overwrite a release."""
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

spec = importlib.util.spec_from_file_location('metadata', Path(__file__).with_name('release-metadata.py'))
metadata = importlib.util.module_from_spec(spec)
spec.loader.exec_module(metadata)
ctx = metadata.context()
root = Path(os.environ['RUNNER_TEMP'])
records = metadata.verified(root/'release')
metadata.verify_host(root/'host-updater')
out = root/'release-notes'
out.mkdir(exist_ok=True)
notes = ['Product version: '+ctx['product_version'], 'Source: `'+ctx['revision']+'`',
         'Build sequence: '+str(ctx['build_seq']), '']
assets = []
urls = {}
for receipt in records:
    component = receipt['component']
    url = os.environ[component.upper()+'_ATTESTATION_URL']
    metadata.require(re.fullmatch(r'https://github\.com/cybrrd/brrdfeeder/attestations/[0-9]+', url), 'invalid attestation URL')
    urls[component] = url
    notes += ['- '+receipt['image']+'@'+receipt['digest'], '  Provenance: '+url]
    for file in ['sbom.cdx.json', 'metadata.json', 'image.txt']:
        source = root/'release'/component/file
        metadata.regular(source)
        dest = out/(component+'-'+file)
        shutil.copyfile(source, dest)
        assets.append(str(dest))
# The bootstrap helper is independently installer-pinned, not history-numbered.
host = root/'host-updater/brrdfeeder-release-arm64'
shutil.copyfile(host, out/host.name)
assets.append(str(out/host.name))
license_file = root/'host-updater/GO-LICENSE'
metadata.regular(license_file)
shutil.copyfile(license_file, out/'GO-LICENSE')
assets.append(str(out/'GO-LICENSE'))
(out/'attestations.json').write_text(json.dumps(urls, indent=2)+'\n')
(out/'digests.txt').write_text('\n'.join(r['image']+'@'+r['digest'] for r in records)+'\n')
assets += [str(out/'attestations.json'), str(out/'digests.txt')]
(out/'SHA256SUMS').write_text(''.join(metadata.sha(Path(p))+'  '+Path(p).name+'\n' for p in assets))
assets.append(str(out/'SHA256SUMS'))
(out/'notes.md').write_text('\n'.join(notes)+'\n')
# gh refuses if a release already exists. No --clobber, edit, or publish path.
subprocess.run(['gh', 'release', 'create', ctx['tag'], '--repo', 'cybrrd/brrdfeeder',
                '--draft', '--verify-tag', '--generate-notes', '--title', ctx['tag'],
                '--notes-file', str(out/'notes.md'), *assets], check=True)
with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
    summary.write('\nDraft created; the release approver reviews and publishes it.\n'+'\n'.join(notes)+'\n')
