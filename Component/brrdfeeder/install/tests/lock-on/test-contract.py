#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Exercise only the extracted pure migration, never the installer."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[5]
SOURCE = (ROOT / 'Component/brrdfeeder/install/brrdfeeder-install.sh').read_text()
OLD = b'capture:\n  interface: "wlan1"\n  hunter:\n    enabled: true\n    lock_on_duration_ms: 2000\n    lock_on_min_ms: 500\n'


class Contract(unittest.TestCase):
    def render(self, data, mode='render'):
        script = SOURCE.split("<<'LOCK_ON_MIGRATION_PY'\n", 1)[1].split('\nLOCK_ON_MIGRATION_PY', 1)[0]
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'config.yaml'
            path.write_bytes(data)
            result = subprocess.run(['python3', '-c', script, mode, str(path)], capture_output=True)
            self.assertEqual(path.read_bytes(), data, 'filter must not mutate its input')
            return result

    def test_old_pair_only_removed_and_rerun_noop(self):
        original = b'# keep this\nnode:\n  id: custom\n' + OLD + b'other: 42\n\n'
        expected = original.replace(b'    lock_on_duration_ms: 2000\n', b'').replace(b'    lock_on_min_ms: 500\n', b'')
        self.assertEqual(self.render(original, 'check').returncode, 0)
        result = self.render(original)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, expected)
        self.assertEqual(self.render(expected).stdout, expected)
        self.assertEqual(self.render(expected, 'check').returncode, 3)

    def test_operator_choices_and_ambiguous_yaml_preserved(self):
        fixtures = [OLD.replace(b'2000', b'0'), OLD.replace(b'500', b'700'),
                    OLD.replace(b'2000', b'02000'), OLD.replace(b'2000', b'"2000"'),
                    OLD.replace(b'2000', b'2000 # operator'),
                    OLD.replace(b'    lock_on_min_ms: 500\n', b''),
                    OLD + OLD, OLD + b'alias: &a 5\nref: *a\n',
                    b'example: |\n' + b''.join(b'  ' + line for line in OLD.splitlines(keepends=True)),
                    OLD.replace(b'  hunter:', b'  unrelated:'),
                    OLD.replace(b'\n', b'\r\n'), b'not: [valid YAML']
        for value in fixtures:
            with self.subTest(value=value):
                self.assertEqual(self.render(value).stdout, value)
                self.assertEqual(self.render(value, 'check').returncode, 3)

    def test_wired_after_yaml_dependency_and_before_service_start(self):
        self.assertIn('\n  migrate_template_lock_on\n', SOURCE)
        call = SOURCE.index('\n  migrate_template_lock_on\n')
        self.assertGreater(call, SOURCE.index('fuse-overlayfs python3 python3-yaml'))
        self.assertLess(call, SOURCE.index('run systemctl --no-block restart brrdfeeder-engine.service'))
        self.assertIn('if [[ $DRY_RUN -eq 0 ]]; then\n  migrate_template_lock_on\nfi', SOURCE)


if __name__ == '__main__':
    unittest.main()
