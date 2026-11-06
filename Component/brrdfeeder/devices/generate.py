#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Embed the canonical read-only scanner/catalog in the self-contained host CLI."""
from pathlib import Path
import argparse
import sys

HERE = Path(__file__).resolve().parent
INSTALLER = HERE.parent/'install/brrdfeeder-install.sh'
START = '# BEGIN GENERATED DEVICE SCAN\n'
END = '# END GENERATED DEVICE SCAN\n'

def generated():
    code = 'REGISTRY_BYTES = '+repr((HERE/'supported-devices.json').read_bytes())+'\n'+(HERE/'scan.py').read_text()
    return START+'''if [[ ${1:-} == devices ]]; then
  set +x
  shift
  exec python3 -c "$(cat <<'DEVICE_SCAN_PY'
'''+code+'''DEVICE_SCAN_PY
)" "$@"
fi
'''+END

if __name__ == '__main__':
    parser = argparse.ArgumentParser(); parser.add_argument('--check',action='store_true');args=parser.parse_args()
    source=INSTALLER.read_text()
    if START in source:
        before,tail=source.split(START,1);_,after=tail.split(END,1)
    else:
        marker='# BRRDfeeder local product command — self-contained recovery, no download.'
        before,after=source.split(marker,1);after=marker+after
    desired=before+generated()+after
    if args.check:
        if source != desired: sys.exit('generated scanner is stale')
    else: INSTALLER.write_text(desired)
