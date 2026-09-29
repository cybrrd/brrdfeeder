#!/usr/bin/env python3
"""One LOCAL operator publish into an existing webroot; no SSH/network/service calls.

Dev preserves public install.sh. Mainstream must be explicitly selected after
the dev proof and Cy approval. Each replacement is atomic; the batch is ordered
(artifacts, release, bootstrap), not falsely claimed as one filesystem transaction.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile

ROOT = Path(__file__).resolve().parents[3]
INSTALL = ROOT/'Component/brrdfeeder/install'

def pin(text, name):
    return re.search(r'^'+name+r'="([^"\n]+)"', text, re.M)[1]

def put(root, relative, data):
    target = root/relative
    for part in [root, *target.relative_to(root).parents]:
        path = part if part == root else root/part
        if path.is_symlink(): raise ValueError('symlink webroot parent: '+str(path))
    if target.is_symlink(): raise ValueError('symlink webroot file: '+str(target))
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix='.d44-', dir=target.parent)
    try:
        with os.fdopen(fd, 'wb') as out:
            out.write(data); out.flush(); os.fchmod(out.fileno(), 0o644); os.fsync(out.fileno())
        os.replace(tmp, target)
        parent = os.open(target.parent, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(parent)
        finally: os.close(parent)
    finally:
        if os.path.exists(tmp): os.unlink(tmp)
    print('published '+str(target)+' sha256='+hashlib.sha256(data).hexdigest())

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--webroot', type=Path, required=True)
    p.add_argument('--mode', choices=['dev','mainstream'], required=True)
    p.add_argument('--release', type=Path, required=True, help='S5 publisher output for N (or proved N+1 on mainstream)')
    p.add_argument('--host-binary', type=Path, required=True)
    p.add_argument('--go-license', type=Path, required=True)
    a = p.parse_args()
    root = a.webroot.absolute()
    if not root.is_dir() or any(x.is_symlink() for x in [root, *root.parents]): p.error('existing non-symlink webroot required')
    ring = 'dev' if a.mode == 'dev' else 'general'
    release = json.loads(a.release.read_text())
    if release['schema'] != 'cybrrd.release.v1' or release['ring'] != ring or release['audience'] != 'configured-ring:'+ring or not re.fullmatch(r'[0-9a-f]{128}', release['sig']): p.error('signed ring publisher output required')
    # This is an artifact consistency check, NOT an independent signature gate.
    # Only pass bytes returned by the pinned-key-verifying publisher.
    bootstrap = (INSTALL/('bootstrap-dev.sh' if ring == 'dev' else 'bootstrap-d44.sh')).read_bytes()
    text = bootstrap.decode()
    if pin(text, 'ENGINE_IMAGE') != 'ghcr.io/cybrrd/brrdfeeder@'+release['digest'] or pin(text, 'CONSOLE_IMAGE') != 'ghcr.io/cybrrd/brrdhouse@'+release['console_digest']: p.error('release/bootstrap pins differ; regenerate variants for proved receipt')
    installer = (INSTALL/'brrdfeeder-install.sh').read_bytes()
    if pin(text, 'INSTALLER_SHA256') != hashlib.sha256(installer).hexdigest(): p.error('installer pin differs')
    binary = a.host_binary.read_bytes()
    sha = hashlib.sha256(binary).hexdigest()
    if pin(installer.decode().replace('readonly ', ''), 'RELEASE_HELPER_SHA256') != sha: p.error('host helper pin differs')
    base = 'releases/v1/updater/'+sha+'/linux-arm64/'
    put(root, base+'brrdfeeder-release', binary)
    put(root, base+'GO-LICENSE', a.go_license.read_bytes())
    put(root, 'dev/brrdfeeder-install.sh', installer)
    put(root, 'releases/v1/'+ring+'/release.json', a.release.read_bytes())
    put(root, 'dev/install.sh' if ring == 'dev' else 'install.sh', bootstrap)
    print('No updater.json published; no Caddy reload performed. Public bootstrap '+('preserved' if ring == 'dev' else 'switched after release'))

if __name__ == '__main__': main()
