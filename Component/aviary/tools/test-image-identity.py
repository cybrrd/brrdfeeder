"""Offline host helper contract. Synthetic Podman only, not the sandbox proof."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[1] / "deploy/bootstrap/brrdfeeder-image-identity.sh"


class IdentityHandoff(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="d33-identity-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.state = self.root / "state"
        self.state.mkdir(mode=0o755)
        self.cid = self.root / "container.cid"
        self.cid.write_text("c" * 64 + "\n")
        self.fake = self.root / "podman"
        self.fake.write_text('#!/bin/sh\n[ "$D33_FAIL" = 0 ] || exit 77\nprintf "%s\\n" "$D33_INSPECT"\n')
        self.fake.chmod(0o755)
        self.env = dict(os.environ, PATH=str(self.root) + ":" + os.environ["PATH"],
                        INVOCATION_ID="a" * 32, D33_FAIL="0",
                        D33_INSPECT="c" * 64 + " sha256:" + "d" * 64 + " true")

    def run_helper(self, mode):
        return subprocess.run(["bash", str(HELPER), mode, str(self.state), str(self.cid)],
                              env=self.env, capture_output=True, text=True)

    def record(self):
        return json.loads((self.state / "identity.json").read_text())

    def test_new_start_invalidates_and_failure_does_not_reuse_known(self):
        self.assertEqual(self.run_helper("prepare").returncode, 0)
        self.assertEqual(self.record()["state"], "pending")
        self.assertEqual(self.run_helper("resolve").returncode, 0)
        self.assertEqual(self.record()["image_digest"], "sha256:" + "d" * 64)
        self.assertEqual((self.state / "identity.json").stat().st_mode & 0o777, 0o644)
        self.env["INVOCATION_ID"] = "b" * 32
        self.assertEqual(self.run_helper("prepare").returncode, 0)
        self.assertNotIn("image_digest", self.record())
        self.assertEqual(self.record()["invocation_id"], "b" * 32)
        self.assertEqual(self.run_helper("resolve").returncode, 0)
        self.env["D33_FAIL"] = "1"
        failed = self.run_helper("resolve")
        self.assertEqual(failed.returncode, 1)
        self.assertIn("running_identity_unverified", failed.stderr)
        self.assertEqual(self.record()["state"], "unverified")
        self.assertNotIn("image_digest", self.record(), "known digest survived resolution failure")

    def test_wrong_container_stopped_or_missing_digest_is_unknown(self):
        for result in ["e" * 64 + " sha256:" + "d" * 64 + " true",
                       "c" * 64 + " sha256:" + "d" * 64 + " false",
                       "c" * 64 + "  true", "c" * 64 + " <no value> true"]:
            with self.subTest(result=result):
                self.env["D33_INSPECT"] = result
                self.assertEqual(self.run_helper("resolve").returncode, 1)
                self.assertEqual(self.record()["state"], "unverified")
                self.assertNotIn("image_digest", self.record())

    def test_unsafe_runtime_directory_and_invalid_invocation_refused(self):
        self.env["INVOCATION_ID"] = "invalid"
        self.assertNotEqual(self.run_helper("prepare").returncode, 0)
        self.assertFalse((self.state / "identity.json").exists())
        self.env["INVOCATION_ID"] = "a" * 32
        self.state.chmod(0o777)
        self.assertNotEqual(self.run_helper("prepare").returncode, 0)
        self.assertFalse((self.state / "identity.json").exists())


if __name__ == "__main__":
    unittest.main()
