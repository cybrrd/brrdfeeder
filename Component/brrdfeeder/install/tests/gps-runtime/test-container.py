#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Real offline Podman startup, never an installer or host GPS device."""
import os
import importlib.util
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[5]
SOURCE = (ROOT/'Component/brrdfeeder/install/brrdfeeder-install.sh').read_text()


class Container(unittest.TestCase):
    def test_missing_gps_does_not_prevent_container_start(self):
        image = os.environ['TEST_OS_IMAGE']
        mapping = re.search(r'^AddDevice=(.*):/dev/\$\{GPS_SYMLINK\}:rw$', SOURCE, re.M)[1]
        with tempfile.TemporaryDirectory(prefix='gps-container-') as folder:
            # The production absent-device mapping must be a stable harmless
            # character device, not a symlink to a disappearing USB endpoint.
            source = Path(folder)/'absent'
            if mapping == '/run/brrdfeeder-gps/device':
                spec = importlib.util.spec_from_file_location('runtime', ROOT/'Component/brrdfeeder/install/gps-runtime.py')
                runtime = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(runtime)
                runtime.ROOT = Path(folder)
                runtime.prepare({}, None)  # actual production absent-device preparation
                source = Path(folder)/'device'
                self.assertEqual(source.resolve(), Path('/dev/null'))
            command = ['podman', 'run', '--rm', '--pull=never', '--network=none',
                       '--read-only', '--cap-drop=all', '--security-opt=no-new-privileges',
                       '--device', str(source)+':/dev/cybrrd_gps:rw', image,
                       'python3', '-c', 'import os,termios; print("CONTAINER_STARTED", flush=True); '
                       'fd=os.open("/dev/cybrrd_gps",os.O_RDONLY); '
                       '\ntry: termios.tcgetattr(fd)\nexcept termios.error: print("GPS_NOT_LIVE")']
            result = subprocess.run(command, capture_output=True, text=True, timeout=45)
            self.assertEqual(result.returncode, 0, result.stdout+result.stderr)
            self.assertIn('CONTAINER_STARTED', result.stdout)
            self.assertIn('GPS_NOT_LIVE', result.stdout)


if __name__ == '__main__':
    unittest.main(verbosity=2)
