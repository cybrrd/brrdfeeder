#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Local startup/fallback proofs. No host install, containers, or network."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("launcher", Path(__file__).with_name("launcher.py"))
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / "release"
        self.receipt = self.root / "receipt"
        self.previous = self.root / "release.previous"
        self.old = b"#!/bin/sh\nprintf 'brrdfeeder-release self-test v1\\n'\n"
        self.previous.write_bytes(self.old)
        self.previous.chmod(0o755)
        self.digest = hashlib.sha256(self.old).hexdigest()
        Path(str(self.previous) + ".sha256").write_text(self.digest + "\n")

    def recover(self):
        # Test fixtures live under writable /tmp and intentionally do not claim
        # root ownership. Production's trusted reader is tested separately.
        launcher.recover(self.binary, self.root, self.receipt,
                         read=lambda path, owner, limit=launcher.LIMIT: path.read_bytes())

    def test_cannot_start_falls_back_offline(self):
        self.binary.write_bytes(b"not an executable")
        self.binary.chmod(0o755)
        self.recover()
        self.assertEqual(self.binary.read_bytes(), self.old)
        self.assertTrue(launcher.starts(self.binary))
        self.assertIn(self.digest, self.receipt.read_text())

    def test_absent_candidate_falls_back(self):
        self.recover()
        self.assertEqual(self.binary.read_bytes(), self.old)

    def test_unconfirmed_startable_candidate_restored(self):
        self.binary.write_bytes(self.old + b"# new signed executable\n")
        self.binary.chmod(0o755)
        (self.root / "host-transaction.json").write_text(json.dumps({"previous_sha256": self.digest, "candidate_sha256": hashlib.sha256(self.binary.read_bytes()).hexdigest()}))
        self.recover()
        self.assertEqual(self.binary.read_bytes(), self.old)
        self.assertFalse((self.root / "host-transaction.json").exists())

    def test_confirmed_candidate_untouched(self):
        new = self.old + b"# confirmed\n"
        self.binary.write_bytes(new)
        self.binary.chmod(0o755)
        self.recover()
        self.assertEqual(self.binary.read_bytes(), new)

    def test_tampered_previous_refuses(self):
        self.binary.write_bytes(b"broken")
        self.previous.write_bytes(self.old + b"# tampered\n")
        with self.assertRaisesRegex(RuntimeError, "checksum"):
            self.recover()
        self.assertEqual(self.binary.read_bytes(), b"broken")

    def test_bad_previous_refuses(self):
        self.binary.write_bytes(b"broken")
        self.previous.write_bytes(b"also broken")
        Path(str(self.previous) + ".sha256").write_text(hashlib.sha256(b"also broken").hexdigest())
        with self.assertRaisesRegex(RuntimeError, "cannot start"):
            self.recover()

    def test_production_reader_rejects_untrusted_tmp(self):
        self.binary.write_bytes(self.old)
        with self.assertRaisesRegex(RuntimeError, "unsafe host path"):
            launcher.trusted(self.binary, 0)

    def test_no_network_or_container_dependency(self):
        source = Path(launcher.__file__).read_text()
        for bad in ("import socket", "import urllib", '"podman"', '"curl"', '"docker"'):
            self.assertNotIn(bad, source)


if __name__ == "__main__":
    unittest.main(verbosity=2)
