# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Dedicated exhaustive offline gate. Never called by normal cargo test."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--shards", type=int, default=16)
    args = parser.parse_args()
    assert 1 <= args.shards <= 4096
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    manifest = str(ROOT / "Cargo.toml")
    started = time.monotonic()
    subprocess.run(["cargo", "build", "--offline", "--locked", "--release", "--manifest-path", manifest, "-p", "cybrrd-rid-v5", "--bin", "v5-exhaustive"], check=True)
    meta = json.loads(subprocess.check_output(["cargo", "metadata", "--offline", "--locked", "--no-deps", "--format-version", "1", "--manifest-path", manifest]))
    binary = Path(meta["target_directory"]) / "release/v5-exhaustive"
    binary_sha = hashlib.sha256(binary.read_bytes()).hexdigest()
    shards = []
    for i in range(args.shards):
        start, end = (2**32 * i // args.shards), (2**32 * (i + 1) // args.shards)
        result = subprocess.run([str(binary), str(start), str(end)], check=True, stdout=subprocess.PIPE)
        receipt = json.loads(result.stdout)
        assert receipt["start"] == start and receipt["end_exclusive"] == end and receipt["cases"] == 2 * (end - start) and receipt["status"] == "passed"
        receipt["binary_sha256"] = binary_sha
        (out / f"timestamp-{i:04}.json").write_text(json.dumps(receipt, indent=2) + "\n")
        shards.append(receipt)
        print(f"timestamp shard {i+1}/{args.shards} passed", flush=True)
    assert sum(s["cases"] for s in shards) == 8_589_934_592
    result = subprocess.run(["cargo", "test", "--offline", "--locked", "--release", "--manifest-path", manifest, "-p", "cybrrd-rid-v5", "--test", "exhaustive", "--test", "other_semantics", "--", "--ignored", "--nocapture"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    (out / "exhaustive-domains.log").write_bytes(result.stdout)
    if result.returncode:
        print(result.stdout.decode(errors="replace"), file=sys.stderr)
        return result.returncode
    domains = [json.loads(line) for line in result.stdout.decode().splitlines() if line.startswith('{"cases":') or line.startswith('{"gate":')]
    expected = {"numeric_non_u32": 460112, "reserved_patterns": 33554986, "other_semantics": 66904}
    assert {d["gate"]: d["cases"] for d in domains} == expected
    source_files = sorted(p for p in ROOT.rglob("*") if p.is_file() and not {"target", "receipts", "validation", "__pycache__"}.intersection(p.parts))
    source_sha = hashlib.sha256(b"".join(str(p.relative_to(ROOT)).encode() + b"\0" + hashlib.sha256(p.read_bytes()).digest() for p in source_files)).hexdigest()
    receipt = {"status": "passed", "numeric_cases": 8_590_394_704, "reserved_cases": 33_554_986, "other_semantic_cases": 66_904,
               "timestamp_shards": len(shards), "timestamp_binary_sha256": binary_sha, "source_manifest_sha256": source_sha,
               "elapsed_seconds": round(time.monotonic() - started, 3), "domains": domains,
               "scope": "full numeric primitives and u16 altitude message bindings; full reserved byte-codec patterns with deterministic message-binding checks; no hardware/hop proof"}
    (out / "exhaustive-gate.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt))
    return 0


if __name__ == "__main__":
    sys.exit(main())
