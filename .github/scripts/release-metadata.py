#!/usr/bin/env python3
"""Bind same-run archives/SBOMs to the immutable source before publication."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

IMAGES = {'engine': 'ghcr.io/cybrrd/brrdfeeder', 'console': 'ghcr.io/cybrrd/brrdhouse'}

def require(ok, message):
    if not ok:
        raise ValueError(message)

def command(*args):
    return subprocess.check_output(args, text=True).strip()

def context():
    require(os.environ.get('GITHUB_REPOSITORY') == 'cybrrd/brrdfeeder', 'foreign repository')
    tag = os.environ.get('GITHUB_REF_NAME', '')
    require(re.fullmatch(r'v[0-9][A-Za-z0-9_.-]*', tag), 'select a version tag')
    require(os.environ.get('GITHUB_REF') == 'refs/tags/'+tag, 'branch dispatch is not a release')
    revision = os.environ.get('GITHUB_SHA', '')
    require(re.fullmatch(r'[a-f0-9]{40}', revision), 'invalid source revision')
    require(command('git', 'rev-parse', 'HEAD') == revision, 'checkout revision mismatch')
    require(command('git', 'rev-parse', 'refs/tags/'+tag+'^{commit}') == revision, 'tag moved')
    require(command('git', 'rev-parse', '--is-shallow-repository') == 'false', 'shallow release')
    count = int(command('git', 'rev-list', '--count', 'HEAD'))
    require(count >= 1, 'empty release history')
    return {'tag': tag, 'revision': revision, 'build_seq': 1000+count}

def regular(path):
    require(not path.is_symlink() and path.is_file(), 'missing or symlink artifact: '+str(path))

def sha(path):
    regular(path)
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024*1024), b''):
            digest.update(chunk)
    return digest.hexdigest()

def measure(component, directory):
    require(component in IMAGES, 'unknown component')
    require(not directory.is_symlink() and not directory.parent.is_symlink(), 'symlink directory')
    receipt = dict(context(), component=component, image=IMAGES[component], schema=1)
    archive = directory/'image.oci.tar'
    sbom = directory/'sbom.cdx.json'
    receipt.update(archive_sha256=sha(archive), sbom_sha256=sha(sbom))
    require(json.loads(sbom.read_text()).get('bomFormat') == 'CycloneDX', 'invalid SBOM')
    ref = 'oci-archive:'+str(archive)
    digest = command('skopeo', 'inspect', '--format', '{{.Digest}}', ref)
    require(re.fullmatch(r'sha256:[a-f0-9]{64}', digest), 'invalid image digest')
    config = json.loads(command('skopeo', 'inspect', '--config', ref))
    labels = config.get('config', {}).get('Labels', {})
    label = 'com.macawi.'+('brrdfeeder' if component == 'engine' else 'brrdhouse')+'.build_seq'
    require(config.get('architecture') == 'arm64' and config.get('os') == 'linux', 'not Linux ARM64')
    require(labels.get('org.opencontainers.image.revision') == receipt['revision'], 'image revision mismatch')
    require(labels.get(label) == str(receipt['build_seq']), 'image build sequence mismatch')
    receipt['digest'] = digest
    return receipt

def verified(root):
    records = []
    for component in IMAGES:
        directory = root/component
        path = directory/'metadata.json'
        regular(path)
        expected = json.loads(path.read_text())
        actual = measure(component, directory)
        require(expected == actual, component+' artifact/metadata drift')
        records.append(actual)
    return records

def verify_host(directory):
    require(not directory.is_symlink(), 'symlink host artifact directory')
    installer = Path('Component/brrdfeeder/install/brrdfeeder-install.sh').read_text()
    pin = re.search(r'^readonly RELEASE_HELPER_SHA256="([a-f0-9]{64})"$', installer, re.M)
    require(pin is not None and sha(directory/'brrdfeeder-release-arm64') == pin[1], 'host helper pin mismatch')
    regular(directory/'GO-LICENSE')

def main():
    mode = sys.argv[1]
    if mode == 'context':
        print(json.dumps(context()))
    elif mode == 'create':
        component, directory = sys.argv[2], Path(sys.argv[3])
        receipt = measure(component, directory)
        (directory/'metadata.json').write_text(json.dumps(receipt, indent=2)+'\n')
    elif mode == 'verify':
        root = Path(sys.argv[2])
        # Verify BOTH components completely before returning any publish input.
        records = verified(root)
        verify_host(root.parent/'host-updater')
        for r in records:
            print('\t'.join((r['component'], r['image'], r['digest'])))
    else:
        raise ValueError('unknown metadata operation')

if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as exc:
        sys.exit('release refused: '+str(exc))
