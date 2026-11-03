#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline bootstrap argv -> real installer parser/selection -> Quadlet files.

Only production shell fragments for argument assembly, parsing, image selection
and rendering execute. All paths are temporary; no root, downloads, hardware,
enrollment, systemd or container runtime. IMAGE_OVERRIDE_INSTALLER selects a
saved baseline installer for the revert control without modifying the checkout.
"""
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import unittest

INSTALL = Path(__file__).resolve().parents[2]
SOURCE = Path(os.environ.get('IMAGE_OVERRIDE_INSTALLER', INSTALL/'brrdfeeder-install.sh')).read_text()
VARIANTS = ('bootstrap.sh', 'bootstrap-dev.sh', 'bootstrap-d44.sh')
ENGINE = 'ghcr.io/cybrrd/brrdfeeder@sha256:'
CONSOLE = 'ghcr.io/cybrrd/brrdhouse@sha256:'
USER_ENGINE, USER_CONSOLE = ENGINE+'a'*64, CONSOLE+'b'*64
OLD_ENGINE, OLD_CONSOLE = ENGINE+'c'*64, CONSOLE+'d'*64


def pin(text, name):
    return re.search(r'^'+name+r'="([^"]+)"$', text, re.M)[1]


class Overrides(unittest.TestCase):
    def run_install(self, args=(), *, variant='bootstrap.sh', installed=None):
        parser = SOURCE[SOURCE.index('DRY_RUN=0\n'):SOURCE.index('\nif [[ $STATUS_ONLY -eq 1 ]]')]
        selection = SOURCE[SOURCE.index('valid_image_pin() {'):SOURCE.index('\nreadonly CONSOLE_IMAGE')+len('\nreadonly CONSOLE_IMAGE')]
        engine_template = SOURCE.split('NEW_QUADLET=$(cat <<EOF\n', 1)[1].split('\nEOF\n)', 1)[0]
        console_template = SOURCE.split('<<CONSOLE_QUADLET_EOF\n', 1)[1].split('\nCONSOLE_QUADLET_EOF', 1)[0]
        bootstrap = (INSTALL/variant).read_text() if variant else ''
        assembly = ''
        if variant:
            start = bootstrap.index('if [[ $BOOT_MODE == install ]]; then\n',
                                    bootstrap.index('export BRRDFEEDER_BOOTSTRAP_PREPARE=1'))
            assembly = bootstrap[start:bootstrap.index('\nfi', start)+3]
        with tempfile.TemporaryDirectory(prefix='brrd-image-override-') as tmp:
            root = Path(tmp)
            engine, console = root/'brrdfeeder-engine.container', root/'brrdhouse.container'
            before = {}
            if installed:
                for path, image in zip((engine, console), installed):
                    path.write_text('[Container]\nImage='+image+'\n')
                    before[path.name] = path.read_bytes()
            variables = dict(BOOT_MODE='install', BOOT_PREPARE=str(int(bool(variant))),
                             IMAGE_REPOSITORY=ENGINE.split('@')[0], CONSOLE_REPOSITORY=CONSOLE.split('@')[0],
                             QUADLET_FILE=str(engine), CONSOLE_QUADLET_FILE=str(console),
                             TARGET_UID='1001', TARGET_GID='1001', DIALOUT_GID='20',
                             GPS_SYMLINK='cybrrd_gps', GPS_SEED='/fixture/gps-seed',
                             CONFIG_PATH=str(root/'config.yaml'), SECRETS_DIR=str(root/'secrets'),
                             STATE_DIR=str(root/'state'), STATUS_DIR=str(root/'status'),
                             CONSOLE_PORT='8080', UNIT_HOSTNAME='fixture', CONSOLE_MEMORY_ARGS='')
            if variant:
                variables.update(ENGINE_IMAGE=pin(bootstrap, 'ENGINE_IMAGE'),
                                 CONSOLE_IMAGE=pin(bootstrap, 'CONSOLE_IMAGE'))
            script = 'set -euo pipefail\n'+''.join(k+'='+shlex.quote(v)+'\n' for k, v in variables.items())
            script += '''fatal() { printf 'FATAL: %s\n' "$*" >&2; exit 1; }
say() { printf '%s\n' "$*" >&2; }
retained_engine_version() { :; }
'''+assembly+'\n'+parser+'\n'+selection+'''
cat > "$QUADLET_FILE" <<ENGINE_EOF
'''+engine_template+'''
ENGINE_EOF
cat > "$CONSOLE_QUADLET_FILE" <<CONSOLE_EOF
'''+console_template+'\nCONSOLE_EOF\n'
            result = subprocess.run(['bash', '-c', script, 'fixture', *args], text=True, capture_output=True,
                                    env={**os.environ, 'LC_ALL': 'C'}, timeout=10)
            files = {p.name: p.read_bytes() for p in (engine, console) if p.exists()}
            if result.returncode:
                self.assertEqual(files, before, 'refusal must not render or replace either Quadlet')
            return result, files

    def assert_images(self, result, files, engine, console):
        self.assertEqual(result.returncode, 0, result.stderr)
        for name, expected in [('brrdfeeder-engine.container', engine), ('brrdhouse.container', console)]:
            images = re.findall(r'^Image=(.*)$', files[name].decode(), re.M)
            self.assertEqual(images, [expected], name)

    def test_fresh_defaults(self):
        for variant in VARIANTS:
            with self.subTest(variant=variant):
                text = (INSTALL/variant).read_text()
                self.assert_images(*self.run_install(variant=variant), pin(text, 'ENGINE_IMAGE'), pin(text, 'CONSOLE_IMAGE'))

    def test_fresh_user_overrides_both_defaults(self):
        for variant in VARIANTS:
            with self.subTest(variant=variant):
                self.assert_images(*self.run_install(['--image', USER_ENGINE, '--console-image', USER_CONSOLE], variant=variant), USER_ENGINE, USER_CONSOLE)

    def test_independent_override_preserves_other_default(self):
        for variant in VARIANTS:
            text = (INSTALL/variant).read_text()
            for flag, value in [('--image', USER_ENGINE), ('--console-image', USER_CONSOLE)]:
                with self.subTest(variant=variant, flag=flag):
                    self.assert_images(*self.run_install([flag, value], variant=variant),
                                       USER_ENGINE if flag == '--image' else pin(text, 'ENGINE_IMAGE'),
                                       USER_CONSOLE if flag == '--console-image' else pin(text, 'CONSOLE_IMAGE'))

    def test_conflicting_duplicates_last_wins(self):
        for variant in (*VARIANTS, None):
            with self.subTest(variant=variant):
                args = ['--image', OLD_ENGINE, '--console-image', OLD_CONSOLE,
                        '--image', USER_ENGINE, '--console-image', USER_CONSOLE]
                self.assert_images(*self.run_install(args, variant=variant), USER_ENGINE, USER_CONSOLE)

    def test_invalid_last_pin_never_falls_back(self):
        for flag, invalid in [('--image', ENGINE.split('@')[0]+':latest'),
                              ('--console-image', CONSOLE.split('@')[0]+':latest'),
                              ('--image', ENGINE+'f'*63), ('--console-image', ENGINE+'a'*64)]:
            with self.subTest(flag=flag, invalid=invalid):
                result, _ = self.run_install([flag, invalid])
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('tags are not accepted', result.stderr)

    def test_missing_last_value_refuses(self):
        for flag in ('--image', '--console-image'):
            with self.subTest(flag=flag):
                result, _ = self.run_install([flag])
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('requires', result.stderr)

    def test_rerun_defaults_preserve_installed_pins(self):
        for variant in VARIANTS:
            with self.subTest(variant=variant):
                self.assert_images(*self.run_install(variant=variant, installed=(OLD_ENGINE, OLD_CONSOLE)), OLD_ENGINE, OLD_CONSOLE)

    def test_rerun_explicit_matching_pins_allowed(self):
        for variant in (*VARIANTS, None):
            with self.subTest(variant=variant):
                args = ['--image', OLD_ENGINE, '--console-image', OLD_CONSOLE]
                self.assert_images(*self.run_install(args, variant=variant, installed=(OLD_ENGINE, OLD_CONSOLE)), OLD_ENGINE, OLD_CONSOLE)

    def test_rerun_explicit_conflicts_refused(self):
        for variant in (*VARIANTS, None):
            for flag, value in [('--image', USER_ENGINE), ('--console-image', USER_CONSOLE)]:
                with self.subTest(variant=variant, flag=flag):
                    result, _ = self.run_install([flag, value], variant=variant, installed=(OLD_ENGINE, OLD_CONSOLE))
                    self.assertNotEqual(result.returncode, 0, 'explicit pin silently ignored')
                    self.assertIn('Refusing to replace', result.stderr)
                    self.assertIn(flag, result.stderr)

    def test_rerun_explicit_default_is_still_an_explicit_pin(self):
        for variant in VARIANTS:
            text = (INSTALL/variant).read_text()
            for flag, value in [('--image', pin(text, 'ENGINE_IMAGE')),
                                ('--console-image', pin(text, 'CONSOLE_IMAGE'))]:
                with self.subTest(variant=variant, flag=flag):
                    result, _ = self.run_install([flag, value], variant=variant, installed=(OLD_ENGINE, OLD_CONSOLE))
                    self.assertNotEqual(result.returncode, 0, 'explicit pin equal to bootstrap default was silently ignored')
                    self.assertIn(flag, result.stderr)


if __name__ == '__main__':
    unittest.main(verbosity=2)
