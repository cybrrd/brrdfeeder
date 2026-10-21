#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
set -euo pipefail
[[ ${1:-} == true || ${1:-} == false ]] || { echo 'usage: prove-console.sh true|false'; exit 2; }
root=$(git rev-parse --show-toplevel)
proof_dir=$(mktemp -d /tmp/drill-console.XXXXXXXX)
export GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off CGO_ENABLED=0
(cd "$root/Component/brrdhouse" && go build -mod=readonly -trimpath -buildvcs=false -o "$proof_dir/brrdhouse" .)
python3 - "$root" "$proof_dir" <<'PY'
import json, pathlib, sys
root, proof = map(pathlib.Path, sys.argv[1:])
(proof / 'overlay.json').write_text(json.dumps({'Replace': {
    str(root / 'Component/aviary/release-system/drill_console_test.go'):
    str(root / 'docs/drill-0.8.31/console_health_test.go')}}))
PY
cd "$root/Component/aviary/release-system"
DRILL_CONSOLE_BINARY="$proof_dir/brrdhouse" DRILL_EXPECT_HEALTHY="$1" \
  go test -mod=readonly -overlay "$proof_dir/overlay.json" -count=1 -v -run '^TestDrillConsoleProcess$' .
