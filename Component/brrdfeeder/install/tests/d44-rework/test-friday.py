#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline layout/promotion proof; synthetic signature shape, NOT crypto proof."""
import hashlib
import importlib.util
import json
import re
from pathlib import Path
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
spec = importlib.util.spec_from_file_location('publisher', ROOT/'Component/brrdfeeder/release/publish-webroot.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)

class Friday(unittest.TestCase):
    def test_public_unchanged_and_variants(self):
        # Frozen wrapper includes the approved sudo/log UX changes. Only its
        # installer pin may differ; real sudoers behavior has separate coverage.
        base = (HERE/'fixtures/bootstrap.sh').read_text()
        public = (module.INSTALL/'bootstrap.sh').read_text()
        normalize = lambda s: re.sub(r'^INSTALLER_SHA256="[a-f0-9]{64}"$', 'INSTALLER_SHA256="<verified below>"', s, flags=re.M)
        self.assertEqual(normalize(public),normalize(base))
        digest = hashlib.sha256((module.INSTALL/'brrdfeeder-install.sh').read_bytes()).hexdigest()
        self.assertEqual(module.pin(public,'INSTALLER_SHA256'),digest)
        for file,ring in [('bootstrap-dev.sh','dev'),('bootstrap-d44.sh','general')]:
            text = (module.INSTALL/file).read_text()
            self.assertEqual(module.pin(text,'INSTALLER_SHA256'),digest)
            self.assertIn('--ring '+ring+' "$@"',text)
            self.assertIn('https://get.cybrrd.com/dev/brrdfeeder-install.sh',text)

    def test_atomic_file_and_symlink_refusal(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            module.put(root,'dev/install.sh',b'first')
            module.put(root,'dev/install.sh',b'second')
            self.assertEqual((root/'dev/install.sh').read_bytes(), b'second')
            (root/'evil').symlink_to(root/'dev', target_is_directory=True)
            with self.assertRaises(ValueError): module.put(root,'evil/install.sh',b'hostile')
            self.assertEqual((root/'dev/install.sh').read_bytes(),b'second')

    def test_publish_dev_then_mainstream(self):
        # Subprocess uses a disposable synthetic repository, never /etc/world.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); script = root/'Component/brrdfeeder/release/publish-webroot.py'
            script.parent.mkdir(parents=True); script.write_bytes((ROOT/'Component/brrdfeeder/release/publish-webroot.py').read_bytes())
            install = root/'Component/brrdfeeder/install'; install.mkdir(parents=True)
            web = root/'web'; web.mkdir(); (web/'install.sh').write_text('PROVEN PUBLIC')
            binary = root/'helper'; binary.write_bytes(b'fixture host')
            license = root/'LICENSE'; license.write_text('fixture license')
            sha = hashlib.sha256(binary.read_bytes()).hexdigest()
            installer = 'readonly RELEASE_HELPER_SHA256="'+sha+'"\n'
            (install/'brrdfeeder-install.sh').write_text(installer)
            manifest = dict(schema='cybrrd.release.v1',ring='dev',audience='configured-ring:dev',sig='a'*128,digest='sha256:'+'1'*64,console_digest='sha256:'+'2'*64)
            bootstrap = '\n'.join([f'ENGINE_IMAGE="ghcr.io/cybrrd/brrdfeeder@{manifest["digest"]}"',f'CONSOLE_IMAGE="ghcr.io/cybrrd/brrdhouse@{manifest["console_digest"]}"',f'INSTALLER_SHA256="{hashlib.sha256(installer.encode()).hexdigest()}"'])
            (install/'bootstrap-dev.sh').write_text(bootstrap+'\n#dev')
            (install/'bootstrap-d44.sh').write_text(bootstrap+'\n#general')
            release = root/'release.json'
            for mode,ring in [('dev','dev'),('mainstream','general')]:
                manifest.update(ring=ring,audience='configured-ring:'+ring); release.write_text(json.dumps(manifest))
                args = ['python3',str(script),'--mode',mode,'--webroot',str(web),'--release',str(release),'--host-binary',str(binary),'--go-license',str(license)]
                p = subprocess.run(args,capture_output=True,text=True); self.assertEqual(p.returncode,0,p.stderr)
                self.assertTrue((web/f'releases/v1/{ring}/release.json').exists())
                self.assertFalse((web/f'releases/v1/{ring}/updater.json').exists())
                self.assertEqual((web/'install.sh').read_text(),'PROVEN PUBLIC' if mode=='dev' else bootstrap+'\n#general')
            binary.write_bytes(b'tampered'); before = (web/'install.sh').read_bytes()
            self.assertNotEqual(subprocess.run(args,capture_output=True).returncode,0)
            self.assertEqual((web/'install.sh').read_bytes(),before)

if __name__ == '__main__': unittest.main(verbosity=2)
