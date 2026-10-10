#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Bind same-run archives/SBOMs to the immutable source before publication."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

IMAGES = {'engine': 'ghcr.io/cybrrd/brrdfeeder', 'console': 'ghcr.io/cybrrd/brrdhouse'}
ENGINE_MANIFEST = Path('Component/aviary/engine/Cargo.toml')
SEMVER = re.compile(
    r'(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)'
    r'(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?'
    r'(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?')

def require(ok, message):
    if not ok:
        raise ValueError(message)

def command(*args):
    return subprocess.check_output(args, text=True).strip()

def product_version(tag=None, manifest=ENGINE_MANIFEST):
    with manifest.open('rb') as stream:
        package = tomllib.load(stream).get('package', {})
    require(package.get('name') == 'engine', 'invalid engine manifest')
    version = package.get('version')
    require(isinstance(version, str), 'missing engine product version')
    require(SEMVER.fullmatch(version) is not None,
            'engine product version is not canonical SemVer: '+version)
    if tag is not None:
        require(tag == 'v'+version,
                'release tag '+tag+' does not match engine product version '+version)
    return version

def context(dry_run=False):
    require(os.environ.get('GITHUB_REPOSITORY') == 'cybrrd/brrdfeeder', 'foreign repository')
    event = os.environ.get('GITHUB_EVENT_NAME')
    require(event in ('push', 'workflow_dispatch'), 'untrusted release event')
    workflow = 'cybrrd/brrdfeeder/.github/workflows/release.yml@'+os.environ.get('GITHUB_REF', '')
    require(os.environ.get('GITHUB_WORKFLOW_REF') == workflow, 'wrong release workflow')
    run = {}
    for field in ('run_id', 'run_attempt'):
        value = os.environ.get('GITHUB_'+field.upper(), '')
        require(re.fullmatch(r'[1-9][0-9]{0,18}', value), 'invalid '+field)
        run[field] = int(value)
    if dry_run:
        require(os.environ.get('GITHUB_EVENT_NAME') == 'workflow_dispatch',
                'dry run requires workflow_dispatch')
        require(os.environ.get('GITHUB_REF') == 'refs/heads/main' and
                os.environ.get('GITHUB_REF_NAME') == 'main',
                'dry run requires the main branch')
        tag = None
        version = product_version()
    else:
        tag = os.environ.get('GITHUB_REF_NAME', '')
        require(re.fullmatch(r'v[0-9][A-Za-z0-9_.-]*', tag), 'select a version tag')
        version = product_version(tag)
        require(os.environ.get('GITHUB_REF') == 'refs/tags/'+tag, 'branch dispatch is not a release')
    revision = os.environ.get('GITHUB_SHA', '')
    require(re.fullmatch(r'[a-f0-9]{40}', revision), 'invalid source revision')
    require(command('git', 'rev-parse', 'HEAD') == revision, 'checkout revision mismatch')
    if not dry_run:
        require(command('git', 'rev-parse', 'refs/tags/'+tag+'^{commit}') == revision, 'tag moved')
    require(command('git', 'rev-parse', '--is-shallow-repository') == 'false', 'shallow release')
    count = int(command('git', 'rev-list', '--count', 'HEAD'))
    require(count >= 1, 'empty release history')
    return {'tag': tag, 'product_version': version, 'revision': revision,
            'build_seq': 1000+count, 'workflow_ref': workflow, 'event': event, **run}

def regular(path):
    require(not path.is_symlink() and path.is_file(), 'missing or symlink artifact: '+str(path))

def sha(path):
    regular(path)
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024*1024), b''):
            digest.update(chunk)
    return digest.hexdigest()

def measure(component, directory, dry_run=False):
    require(component in IMAGES, 'unknown component')
    require(not directory.is_symlink() and not directory.parent.is_symlink(), 'symlink directory')
    receipt = dict(context(dry_run), component=component, image=IMAGES[component], schema=1)
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
    args = sys.argv[2:]
    dry_run = args[:1] == ['--dry-run']
    if dry_run:
        args = args[1:]
    if mode == 'context':
        require(not args, 'unexpected context arguments')
        print(json.dumps(context(dry_run)))
    elif mode == 'create':
        require(len(args) == 2, 'create requires component and directory')
        component, directory = args[0], Path(args[1])
        receipt = measure(component, directory, dry_run)
        (directory/'metadata.json').write_text(json.dumps(receipt, indent=2)+'\n')
    elif mode == 'verify':
        require(not dry_run and len(args) == 1, 'verify requires a release directory')
        root = Path(args[0])
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
