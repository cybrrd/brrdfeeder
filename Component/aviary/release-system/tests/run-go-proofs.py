#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""D44 revival controls; mutations run in isolated copies, never live sources."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

root = Path(__file__).resolve().parents[4]
source = root / "Component/aviary/release-system"
out = Path(os.environ.get('D44_EVIDENCE_DIR', str(Path(__file__).resolve().parent / 'evidence')))
out.mkdir(parents=True, exist_ok=True)


def run(name, command, cwd=source, expect=0):
    env = dict(os.environ)
    if name in ('go-baseline','go-restored'):
        env['D44_EVIDENCE'] = str(out)
    p = subprocess.run(command, cwd=cwd, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    (out / (name + ".log")).write_text("$ " + " ".join(command) + "\n" + p.stdout + f"\nexit_code={p.returncode}\n")
    print(name, p.returncode, flush=True)
    if p.returncode != expect:
        raise RuntimeError(f"{name}: unexpected result; inspect retained log")
    if expect and ("--- FAIL:" not in p.stdout or "build failed" in p.stdout):
        raise RuntimeError(f"{name}: mutation did not fail an assertion")
    return p.stdout


run("go-baseline", ["go", "test", "-race", "-count=1", "-v", "./..."])
run("launcher", ["python3", "launcher_test.py"])
mutations = [
    ("wrong-key", "manifest.go", "if !verified {", "if false && !verified {", "TestWrongKeyRefuses"),
    ("tamper", "manifest.go", "if !verified {", "if false && !verified {", "TestTamperedRefuses"),
    ("tag", "manifest.go", "|| !digestRE.MatchString(m.Digest)", "|| false", "TestTagTargetRefuses"),
    ("cohort", "manifest.go", 'cohort(node, m.Salt) < m.Rollout', 'true || cohort(node, m.Salt) < m.Rollout', "TestCohortExcludes"),
    ("stale", "updater.go", 'now.Sub(s.Written) > time.Duration(s.Interval)*3*time.Second', 'now.Sub(s.Written) < time.Duration(s.Interval)*3*time.Second', "TestStatusBoundary"),
    ("rollback", "package.go", 'e != nil || !u.healthy(ni.Digest, m.Version, m.BuildSeq, tx.Started)', 'e != nil || false && !u.healthy(ni.Digest, m.Version, m.BuildSeq, tx.Started)', "TestUnhealthyRollsBack"),
    ("cap", "package.go", '>= attemptCap', '> attemptCap', "TestAttemptCapQuarantines"),
    ("restarts", "restarts.go", 's != u.watch[componentIndex(console)]', 'false && s != u.watch[componentIndex(console)]', "TestFreshActiveCrashLoopRollsBack"),
    ("startup-restarts", "restarts.go", 's.UnitRestarts != 0', 'false && s.UnitRestarts != 0', "TestStartupLoopBeforeFirstObservationRefused"),
    ("host-signature", "host_update.go", 'if !ok {', 'if false && !ok {', "TestHostExplicitSignatureDomain"),
    ("host-explicit", "host_update.go", '|| !m.UpdaterOnly', '|| false', "TestHostExplicitSignatureDomain"),
    ("host-fallback", "host_update.go", 'atomicFile(binary, old, 0755)', 'atomicFile(binary, candidate, 0755)', "TestHostBinaryStartupFallback"),
    ("host-separate", "host_update.go", 'if u.state.Active != nil {', 'if false && u.state.Active != nil {', "TestHostCannotSharePackageTransaction"),
    ("host-recovery-tick", "updater.go", 'if mode == "poll-updater" {', 'if false && mode == "poll-updater" {', "TestHostTickDefersAfterPackageRecovery"),
    ("host-artifact-hash", "host_update.go", 'if contentHash(b) != m.SHA256 {', 'if false && contentHash(b) != m.SHA256 {', "TestSignedHostPollDownloadsAndFallsBack"),
    ("poll-floor", "updater.go", 'if u.now().Before(u.state.NextPoll) {', 'if false && u.now().Before(u.state.NextPoll) {', "TestNoPathExistsAndFailedFetchFloor"),
    ("boot-recovery", "updater.go", 'if u.state.Active != nil {', 'if false && u.state.Active != nil {', "TestSIGKILLBootRecoveryOffline"),
]
receipts = []
for name, filename, before, after, test in mutations:
    with tempfile.TemporaryDirectory(prefix="d44-mutant-") as tmp:
        work = Path(tmp) / "release-system"
        shutil.copytree(source, work, ignore=shutil.ignore_patterns("__pycache__"))
        p = work / filename
        original = p.read_text()
        assert before in original, (name, before)
        p.write_text(original.replace(before, after))
        run("mutation-" + name, ["go", "test", "-count=1", "-run", "^" + test + "$", "-v"], work, 1)
        receipts.append({"control":name,"test":test,"mutant_exit":1,"source_sha256":hashlib.sha256(original.encode()).hexdigest()})
run("go-restored", ["go", "test", "-race", "-count=1", "-v", "./..."])
run("go-vet", ["go", "vet", "./..."])
(out / "mutations.json").write_text(json.dumps(receipts, indent=2) + "\n")
