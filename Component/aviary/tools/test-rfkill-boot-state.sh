#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
helper="${script_dir}/../deploy/bootstrap/brrdfeeder-rfkill-boot-state.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/sys/rfkill7" "$tmp/state" "$tmp/bin"
printf 'bluetooth\n' > "$tmp/sys/rfkill7/type"
printf '1\n' > "$tmp/sys/rfkill7/soft"
printf '1\n' > "$tmp/state/platform-test:bluetooth"
cat > "$tmp/bin/rfkill-fixture" <<EOF
#!/usr/bin/env bash
[[ \$* == 'unblock bluetooth' ]]
printf '0\\n' > '$tmp/sys/rfkill7/soft'
EOF
chmod +x "$tmp/bin/rfkill-fixture"

RFKILL_SETTLE_SECONDS=0 "$helper" --sysfs-root "$tmp/sys" --state-dir "$tmp/state" \
  --rfkill-command "$tmp/bin/rfkill-fixture"
[[ $(<"$tmp/sys/rfkill7/soft") == 0 ]]
[[ $(<"$tmp/state/platform-test:bluetooth") == 0 ]]
RFKILL_SETTLE_SECONDS=0 "$helper" --verify --sysfs-root "$tmp/sys" --state-dir "$tmp/state"
dry_output=$(RFKILL_SETTLE_SECONDS=0 "$helper" --dry-run --sysfs-root "$tmp/sys" --state-dir "$tmp/state")
grep -q 'would run:' <<<"$dry_output"

printf 'PASS rfkill fixture: soft=0 restore=0 verify=idempotent dry-run=no-write\n'
