#!/usr/bin/env python3
"""Hermetic build-procedure contract checks; not evidence of a real image build."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("build-arm64.sh")
SOURCE = SCRIPT.parents[3]
SHA = "0123456789abcdef0123456789abcdef01234567"


def mock(command, args):
    with open(os.environ["TEST_LOG"], "a") as out:
        out.write(json.dumps([command, args]) + "\n")
    mode = os.environ["TEST_MODE"]
    if command == "uname":
        print("aarch64")
    elif command == "git":
        args = args[2:] if args[0] == "-C" else args
        if "--show-toplevel" in args: print(SOURCE)
        elif "--is-shallow-repository" in args: print("true" if mode == "shallow" else "false")
        elif args[0] == "rev-parse": print(SHA)
        elif args[0] == "symbolic-ref": print("test/same-ref")
        elif args[0] == "rev-list": print("123")
        elif args[0] == "show": print(os.environ.get("TEST_COMMIT_EPOCH", "1789936260"))
        elif args[0] == "status": print(" M engine/Containerfile" if mode == "dirty" else "")
        elif args[0] == "archive":
            return subprocess.call(["tar", "-cf", "-", "-C", os.environ["TEST_CONTEXT"], "."])
        else: raise ValueError(args)
    else:
        root = None
        if args[0] == "--root":
            root = args[1]
            args = args[6:]
        if args[0] == "info":
            if "{{.Host.Security.Rootless}}" in args: print("false" if mode == "rootful" else "true")
            elif "{{.Store.GraphRoot}}" in args: print(root)
            elif "json" in args: print("{}")
        elif args[0] == "images":
            if "--quiet" in args: print("unexpected-image" if mode == "warm" else "")
            else: print("[]")
        elif args[0] == "pull": pass
        elif args[0] == "build":
            epoch = os.environ.get("TEST_COMMIT_EPOCH", "1789936260")
            if os.environ.get("SOURCE_DATE_EPOCH") != epoch or "SOURCE_DATE_EPOCH=" + epoch not in args:
                return 65
            return 9 if mode == "build-fails" else 0
        elif args[0] == "run":
            if "/bin/uname" in args: print("x86_64" if mode == "probe-fails" else "aarch64")
            elif "/bin/sh" in args and mode == "smoke-fails": return 8
            elif "awk" in args:
                print("0" if mode == "wrong-shadow-day" else int(os.environ.get("TEST_COMMIT_EPOCH", "1789936260")) // 86400)
        elif args[:2] == ["image", "inspect"]:
            if "--format" not in args: print("[]")
            else:
                fmt = args[args.index("--format") + 1]
                print("arm64" if "Architecture" in fmt else SHA if "revision" in fmt else
                      "1123" if "build_seq" in fmt else "sha256:" + "a" * 64)
        elif args[0] == "save":
            if mode == "save-fails": return 7
            Path(args[args.index("--output") + 1]).write_text("mock archive; not a real image\n")
        else: raise ValueError(args)
    return 0


class BuildContract(unittest.TestCase):
    def test_guards_and_clean_procedure(self):
        for mode in ["check", "shallow", "dirty", "rootful", "warm", "unpinned",
                     "probe-fails", "build-fails", "smoke-fails", "wrong-shadow-day", "save-fails", "complete",
                     "ambient-past", "ambient-future", "source-date-changes"]:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory(prefix="d36-contract-") as name:
                temp = Path(name)
                binary = temp / "bin"
                binary.mkdir()
                for command in ["git", "podman", "uname"]:
                    exe = binary / command
                    exe.write_text("#!/bin/sh\nexec " + sys.executable + " " + str(Path(__file__).resolve())
                                   + " --mock " + command + ' "$@"\n')
                    exe.chmod(0o755)
                context = temp / "context/engine"
                context.mkdir(parents=True)
                data = (SCRIPT.parent.parent / "engine/Containerfile").read_text()
                if mode == "unpinned": data = data.replace("@sha256:", "@not-sha256:")
                (context / "Containerfile").write_text(data)
                env = dict(os.environ, PATH=str(binary) + ":" + os.environ["PATH"],
                           TEST_MODE=mode, TEST_LOG=str(temp / "commands.jsonl"), TEST_CONTEXT=str(context.parent))
                env["SOURCE_DATE_EPOCH"] = "1893456000" if mode == "ambient-future" else "946684800"
                if mode == "source-date-changes": env["TEST_COMMIT_EPOCH"] = "1790022660"
                args = ["--check"] if mode == "check" else ["same-tag", str(temp / "out")]
                result = subprocess.run(["bash", str(SCRIPT), *args], env=env, text=True, capture_output=True)
                complete = mode in ["complete", "ambient-past", "ambient-future", "source-date-changes"]
                self.assertEqual(result.returncode == 0, mode == "check" or complete, result.stderr)
                calls = [json.loads(line) for line in (temp / "commands.jsonl").read_text().splitlines()]
                if complete:
                    build = next(args for command, args in calls if command == "podman" and "build" in args)
                    self.assertEqual(build[0], "--root")
                    for flag in ["--runroot", "--tmpdir", "--timestamp", "--pull=never"]: self.assertIn(flag, build)
                    self.assertNotIn("--target", build)
                    self.assertIn("BUILD_SEQ=1123",build)
                    self.assertIn("SOURCE_DATE_EPOCH=" + env.get("TEST_COMMIT_EPOCH", "1789936260"), build)
                    pulls = [args for command, args in calls if command == "podman" and "pull" in args]
                    self.assertEqual(len(pulls), 2)
                    self.assertTrue(all("@sha256:" in args[-1] for args in pulls))
                    self.assertTrue((temp / "out/brrdfeeder-engine-same-tag.tar.sha256").exists())
                else:
                    self.assertFalse((temp / "out/brrdfeeder-engine-same-tag.tar").exists())


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--mock":
        sys.exit(mock(sys.argv[2], sys.argv[3:]))
    unittest.main()
