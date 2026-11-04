# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Normal offline gate, with deterministic representative numeric bindings."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out = args.out.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, V5_RECEIPT_DIR=str(args.out))
    manifest = str(ROOT / "Cargo.toml")
    commands = [
        ["cargo", "fmt", "--manifest-path", manifest, "--package", "cybrrd-rid-v5", "--", "--check"],
        ["cargo", "clippy", "--offline", "--locked", "--manifest-path", manifest, "-p", "cybrrd-rid-v5", "--all-targets", "--", "-D", "warnings"],
        ["cargo", "test", "--offline", "--locked", "--manifest-path", manifest, "-p", "cybrrd-rid-v5", "--", "--nocapture"],
        ["cargo", "build", "--offline", "--locked", "--release", "--manifest-path", manifest, "-p", "cybrrd-rid-v5"],
    ]
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--offline", "--locked", "--no-deps", "--format-version", "1", "--manifest-path", manifest], env=env))
    binary = Path(metadata["target_directory"]) / "release/v5-test-ingress"
    commands.append([sys.executable, str(ROOT / "tools/verify_contract.py"), "--binary", str(binary), "--receipt", str(args.out / "independent-contract.json")])
    receipts = []
    for index, command in enumerate(commands):
        started = time.monotonic()
        result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env, cwd=ROOT)
        log = args.out / f"default-{index}.log"
        log.write_bytes(result.stdout)
        receipts.append({"command": command, "exit_code": result.returncode, "elapsed_seconds": round(time.monotonic() - started, 3), "log": log.name, "sha256": hashlib.sha256(result.stdout).hexdigest()})
        print(f"{index}: exit={result.returncode} {log}", flush=True)
        if result.returncode:
            print(result.stdout.decode(errors="replace"), file=sys.stderr)
            break
    passed = len(receipts) == len(commands) and all(r["exit_code"] == 0 for r in receipts)
    (args.out / "default-gate.json").write_text(json.dumps({"status": "passed" if passed else "failed", "exhaustive_domains": "separate release_gate.py", "commands": receipts}, indent=2) + "\n")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
