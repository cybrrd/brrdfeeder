#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
gate="${script_dir}/bt-dongle-gate.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

printf 'Bus 001 Device 004: ID 0bda:876e Realtek Semiconductor Corp. Bluetooth Radio\n' > "$tmp/accepted.lsusb"
printf 'Bus 001 Device 004: ID 0bda:a728 Realtek Semiconductor Corp. Bluetooth Radio\n' > "$tmp/accepted-a728.lsusb"
printf 'Bluetooth: hci0: loading rtl_bt/rtl8761bu_fw.bin\n' > "$tmp/firmware.dmesg"
printf 'Bus 001 Device 004: ID 0bda:8771 Realtek Semiconductor Corp. Bluetooth Radio\n' > "$tmp/rejected.lsusb"
printf 'Bus 001 Device 004: ID 0bda:c123 Realtek Semiconductor Corp. Bluetooth Radio\n' > "$tmp/unknown.lsusb"
cat > "$tmp/probe.py" <<'PY'
#!/usr/bin/env python3
print("hci0: LE features=0018000000000000 coded_phy=True extended_adv=True")
print("PROBE OK")
PY

run_gate() {
  BT_GATE_ALLOW_NONROOT_TEST=1 "$gate" --bench --hci-index 0 --probe "$tmp/probe.py" \
    --lsusb-file "$1" --dmesg-file "$tmp/firmware.dmesg"
}

pass_output=$(run_gate "$tmp/accepted.lsusb")
grep -q '^PASS 0bda:876e hci0 ' <<<"$pass_output"
pass_a728_output=$(run_gate "$tmp/accepted-a728.lsusb")
grep -q '^PASS 0bda:a728 hci0 ' <<<"$pass_a728_output"

set +e
reject_output=$(run_gate "$tmp/rejected.lsusb" 2>&1)
reject_rc=$?
unknown_output=$(run_gate "$tmp/unknown.lsusb" 2>&1)
unknown_rc=$?
dry_output=$("$gate" --dry-run 2>&1)
dry_rc=$?
set -e

[[ $reject_rc -ne 0 ]] && grep -q 'STOP unclassified Realtek PID 0bda:8771' <<<"$reject_output"
[[ $unknown_rc -ne 0 ]] && grep -q -- '--- dmesg btusb/btrtl lines ---' <<<"$unknown_output" && grep -q 'STOP unclassified' <<<"$unknown_output"
[[ $dry_rc -ne 0 ]] && grep -q 'does not touch the adapter' <<<"$dry_output"

printf 'PASS bt-dongle-gate fixtures: accept=876e,a728 stop=8771 stop=unknown dry-run=no-touch\n'
