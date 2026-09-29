#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Actual rootless Podman lifecycle; isolated temporary stores, no registry calls."""
import os
from pathlib import Path
import subprocess
import tempfile
ROOT=Path(__file__).resolve().parents[4]
SOURCE=ROOT/'Component/aviary/release-system'
with tempfile.TemporaryDirectory(prefix='public-updater-runtime-') as tmp:
    output=Path(tmp)
    env=dict(os.environ,CGO_ENABLED='0',GOTOOLCHAIN='local',GOPROXY='off',GOSUMDB='off')
    # Podman's rootless pause process is keyed by XDG_RUNTIME_DIR, not only
    # --root/--runroot. Concurrent fixtures must not reset each other's runtime.
    runtime=output/'runtime'; runtime.mkdir(mode=0o700)
    env['XDG_RUNTIME_DIR']=str(runtime)
    env.pop('DBUS_SESSION_BUS_ADDRESS',None)
    for name,folder,args in [
        ('brrdfeeder-release',SOURCE,['.']),
        ('brrdhouse',ROOT/'Component/brrdhouse',['.']),
        ('fixture-engine',SOURCE,['tests/fixtures/engine.go'])]:
        subprocess.run(['go','build','-mod=readonly','-trimpath','-buildvcs=false','-o',str(output/name),*args],
                       cwd=folder,env=env,check=True,timeout=240)
    env.update(D44_PODMAN='1',D44_FIXTURE_DIR=str(output))
    subprocess.run(['go','test','-count=1','-v','-run','^TestRealPodmanLifecycleAndPrune$'],
                   cwd=SOURCE,env=env,check=True,timeout=600)
