#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Source only after the runbook's preflight, registration and protected capture.
set -euo pipefail
p=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
h=/usr/local/libexec/brrdfeeder-release
l=${h}-launch
u=/var/lib/brrdfeeder-updater
c=/etc/brrdfeeder
new=$(python3 -B "$p/binding.py" candidate_sha256)
manifest_sha=${HELPER5_PREIMAGES_SHA256:?independently registered preimage hash required}
python3 -B "$p/binding.py" guard
inspect_live() { python3 -B "$p/fixture.py" "$@"; }
quiescent() {
  for unit in brrdfeeder-release-poll.timer brrdfeeder-host-update.timer; do
    [[ $(systemctl is-enabled "$unit" || :) == disabled ]]
    [[ $(systemctl is-active "$unit" || :) == inactive ]]
  done
  for unit in brrdfeeder-release-poll.service brrdfeeder-host-update.service brrdfeeder-updater.service; do
    [[ $(systemctl is-active "$unit" || :) == inactive ]]
  done
  inspect_live protected
}
helper_check() {
  [[ $("$h" self-test) == 'brrdfeeder-release self-test v1' ]]
  [[ $("$h" build-seq) == "$1" ]]
  sha256sum -c "$c/.updater-helper.sha256"
}
restore_baseline() {
  quiescent
  flock -n "$u/launch.lock" flock -n "$u/lock" \
    python3 -B "$p/helper-preimages.py" restore "$p/preimages" --manifest-sha256 "$manifest_sha"
  inspect_live baseline
  helper_check 4
}
normal_trial() {
  local label=$1 dir=$p/evidence/$1
  [[ $label =~ ^[a-z0-9-]+$ ]]
  quiescent
  inspect_live baseline
  helper_check 4
  mkdir -m 700 "$dir"
  inspect_live snapshot > "$dir/before.json"
  date -u +%FT%TZ > "$dir/start.txt"
  unshare --net -- strace -f -o "$dir/normal-network.trace" -e trace=network,execve \
    bash "$p/install.sh" --upgrade-helper --helper-file "$p/helper5" > "$dir/normal.log" 2>&1
  ! grep -E 'AF_INET6?' "$dir/normal-network.trace"
  inspect_live recovered committed
  helper_check 5
  inspect_live snapshot > "$dir/after-first.json"
  unshare --net -- strace -f -o "$dir/idempotent-network.trace" -e trace=network,execve \
    bash "$p/install.sh" --upgrade-helper --helper-file "$p/helper5" > "$dir/idempotent.log" 2>&1
  ! grep -E 'AF_INET6?' "$dir/idempotent-network.trace"
  inspect_live recovered committed
  helper_check 5
  inspect_live snapshot > "$dir/after-second.json"
  cmp "$dir/after-first.json" "$dir/after-second.json"
  date -u +%FT%TZ > "$dir/end.txt"
  printf 'PASS normal + idempotent; zero traced AF_INET/AF_INET6 calls\n' > "$dir/PASS"
}
fault_trial() {
  local phase=$1 label=$2 dir=$p/evidence/$2
  [[ $label =~ ^[a-z0-9-]+$ ]]
  quiescent
  inspect_live baseline
  helper_check 4
  mkdir -m 700 "$dir"
  B03_MODE=helper B03_PROFILE=live B03_PHASE="$phase" B03_GDB_OUT="$dir" \
    unshare --net -- gdb -nx -q -batch -x "$p/gdb-fsync.py" --args \
    "$p/helper5" upgrade-local-helper --sha256 "$new" > "$dir/gdb.log" 2>&1
  python3 -B - "$dir/result.json" "$phase" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]))
assert r['status']=='PASS' and r['native_arm64'] and r['killed']
assert r['profile']=='live' and r['phase']==sys.argv[2]
PY
  inspect_live phase "$phase"
  if [[ -e "$u/host-state.json" ]]; then cp "$u/host-state.json" "$dir/floor-at-stop.json"; else : > "$dir/floor-at-stop.json"; fi
  inspect_live snapshot > "$dir/after-kill.json"
}
boundary_trial() {
  local phase=$1 route=$2 label=$1-$2
  [[ $route == launcher || $route == preimages ]]
  restore_baseline
  fault_trial "$phase" "$label"
  if [[ $route == launcher ]]; then
    unshare --net -- "$l" self-test > "$p/evidence/$label/recover.log" 2>&1
    if [[ $phase == committed ]]; then inspect_live recovered committed; helper_check 5
    else inspect_live recovered; helper_check 4; fi
    inspect_live snapshot > "$p/evidence/$label/after-recovery.json"
  fi
  restore_baseline > "$p/evidence/$label/rollback.log" 2>&1
  inspect_live snapshot > "$p/evidence/$label/after-rollback.json"
  printf 'PASS %s %s + R06\n' "$phase" "$route" > "$p/evidence/$label/PASS"
}
after_boot() {
  local kind=$1 dir=$p/evidence/reboot-$1
  [[ $kind == interrupted || $kind == committed ]]
  [[ $(< /proc/sys/kernel/random/boot_id) != $(< "$dir/boot-id-before") ]]
  cat /proc/sys/kernel/random/boot_id > "$dir/boot-id-after"
  # Inspect actual boot execution before any manual recovery invocation.
  systemctl show brrdfeeder-release-recover.service \
    -p Result -p ExecMainStatus -p ExecMainCode -p ExecMainStartTimestampMonotonic -p InvocationID > "$dir/boot-service.txt"
  journalctl -b --no-pager -u brrdfeeder-release-recover.service > "$dir/boot-journal.txt"
  python3 -B - "$dir/boot-service.txt" <<'PY'
import sys
d=dict(x.strip().split('=',1) for x in open(sys.argv[1]) if '=' in x)
assert d['Result']=='success' and d['ExecMainStatus']=='0' and d['ExecMainCode']=='1'
assert int(d['ExecMainStartTimestampMonotonic'])>0 and d['InvocationID']
PY
  quiescent
  if [[ $kind == interrupted ]]; then
    grep -F 'restored previous executable locally; no network' "$dir/boot-journal.txt"
    inspect_live recovered
    helper_check 4
  else
    ! grep -F 'restored previous executable locally; no network' "$dir/boot-journal.txt"
    inspect_live recovered committed
    helper_check 5
  fi
  inspect_live snapshot > "$dir/after-boot.json"
  restore_baseline > "$dir/rollback-after-boot.log" 2>&1
  printf 'PASS %s boot + R06\n' "$kind" > "$dir/PASS"
}
