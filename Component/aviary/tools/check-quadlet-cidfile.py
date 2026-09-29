#!/usr/bin/env python3
"""Assert the generated BRRDfeeder unit passes --cidfile to `podman run`.

WHY THIS EXISTS
---------------
Step 4 derives the running image digest at start. The host-side helper
(`brrdfeeder-image-identity resolve`) reads the container id for THIS systemd
invocation out of `%t/%N.cid`. If `podman run` is never told to write that file,
the helper resolves nothing, the engine reports `image_digest=unknown` and
refuses updates.

On 2026-09-21 that happened in the field. podman 5.4.2's quadlet generator emits
`--cidfile` on its own; **podman 5.7.0 moved to name-based container
identification and emits none.** Nothing else looked wrong -- the helper was
installed, ExecStartPost was correct, the runtime directory had the right owner
and mode. An invisible dependency had moved underneath a configuration that
still read as correct.

A container NAME cannot substitute: it survives across invocations, so it cannot
identify the container this invocation started.

WHAT IT CHECKS
--------------
That the rendered ExecStart contains a `--cidfile=` argument.

WHY IT IS SPLIT IN TWO
----------------------
The assertion is a pure function over rendered unit text, separate from
rendering. That is what makes the negative control possible on ANY podman
version -- on 5.4.2 the generator supplies `--cidfile` itself, so stripping our
line does not produce a failing render and could not prove the check works.
Feeding the assertion a unit that lacks the flag proves it can fail.

Exit 0 pass, 1 fail, 2 could not run (never silently pass).
"""
from __future__ import annotations
import os, shutil, subprocess, sys, tempfile
from pathlib import Path

GENERATORS = ("/usr/libexec/podman/quadlet", "/usr/lib/podman/quadlet")


def assert_cidfile(unit_text: str) -> tuple[bool, str]:
    """Pure assertion over rendered unit text. Returns (ok, reason)."""
    execs = [l for l in unit_text.splitlines() if l.startswith("ExecStart=")]
    if not execs:
        return False, "no ExecStart= line in the rendered unit"
    for line in execs:
        if "--cidfile=" in line:
            return True, "ExecStart passes --cidfile"
    return False, ("ExecStart does not pass --cidfile -- the step-4 identity "
                   "handoff cannot resolve a container id for this invocation")


def render(quadlet: Path) -> str:
    gen = next((g for g in GENERATORS if Path(g).is_file()), None)
    if gen is None:
        raise FileNotFoundError("no quadlet generator found; cannot render")
    with tempfile.TemporaryDirectory() as td:
        shutil.copy(quadlet, Path(td) / quadlet.name)
        # QUADLET_UNIT_DIRS is the ONLY way to point the generator at our copy.
        # A positional directory is IGNORED -- it silently reads the system dirs
        # and renders the INSTALLED unit instead, which makes any with/without
        # comparison tautological. Verified the hard way, 2026-09-21.
        env = {**os.environ, "QUADLET_UNIT_DIRS": td}
        out = subprocess.run([gen, "-dryrun", "-no-kmsg-log"],
                             capture_output=True, text=True, timeout=60, env=env)
        if not out.stdout.strip():
            raise RuntimeError(f"generator produced nothing: {out.stderr[:300]}")
        if quadlet.name.removesuffix(".container") not in out.stdout:
            raise RuntimeError("generator did not render the requested unit; "
                               "QUADLET_UNIT_DIRS was not honoured")
        return out.stdout


# A rendered unit that lacks the flag -- podman 5.7.0's shape. The negative control.
WITHOUT_CIDFILE = (
    "[Service]\n"
    "ExecStop=/usr/bin/podman rm -v -f -i brrdfeeder-engine\n"
    "ExecStart=/usr/bin/podman run --name brrdfeeder-engine --replace --rm "
    "--network host --sdnotify=conmon -d localhost/brrdfeeder-engine:verified\n"
)


def main() -> int:
    # 1 -- NEGATIVE CONTROL FIRST. An assertion never seen to fail is not evidence.
    ok, _ = assert_cidfile(WITHOUT_CIDFILE)
    if ok:
        print("FAIL: the check PASSED a unit with no --cidfile. The check is broken.")
        return 1
    ok, _ = assert_cidfile("[Service]\nWantedBy=default.target\n")
    if ok:
        print("FAIL: the check PASSED a unit with no ExecStart. The check is broken.")
        return 1
    print("negative controls: both refused as expected -- the check can fail")

    # 2 -- the real template
    quadlet = Path(sys.argv[1]) if len(sys.argv) > 1 else (
        Path(__file__).resolve().parents[1] / "deploy/quadlet/brrdfeeder-engine.container")
    if not quadlet.is_file():
        print(f"CANNOT RUN: quadlet not found at {quadlet}")
        return 2
    try:
        rendered = render(quadlet)
    except Exception as exc:
        print(f"CANNOT RUN: {exc}")
        return 2

    ok, reason = assert_cidfile(rendered)
    print(f"{'PASS' if ok else 'FAIL'}: {quadlet.name}: {reason}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
