#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
flush="${script_dir}/../deploy/bootstrap/brrdfeeder-blackbox-flush.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/out"
install -m 0600 /dev/null "$tmp/lock"

for i in $(seq 1 200); do
  printf 'kernel-A-%03d bounded power evidence\n' "$i"
done > "$tmp/kernel-a.log"
for i in $(seq 1 200); do
  printf 'engine-A-%03d bounded power evidence\n' "$i"
done > "$tmp/engine-a.log"
printf 'kernel-A-control \033[31mRED\tTAB\n' >> "$tmp/kernel-a.log"
sed 's/-A-/-B-/' "$tmp/kernel-a.log" > "$tmp/kernel-b.log"
sed 's/-A-/-B-/' "$tmp/engine-a.log" > "$tmp/engine-b.log"

run_flush() {
  BLACKBOX_LOCK_FILE="$tmp/lock" "$flush" --output-dir "$tmp/out" --boot-id "$1" \
    --kernel-source "$2" --engine-source "$3"
}

run_flush boot-A "$tmp/kernel-a.log" "$tmp/engine-a.log"
grep -q 'kernel-A-200' "$tmp/out/current.log"
if LC_ALL=C grep -q $'\033' "$tmp/out/current.log"; then
  printf 'FAIL blackbox retained ESC control byte\n' >&2
  exit 1
fi
grep -Fq $'RED\tTAB' "$tmp/out/current.log"
before_hash=$(sha256sum "$tmp/out/current.log" | awk '{print $1}')

# A hostile pre-placed symlink must be rejected without opening/truncating it.
printf 'do-not-truncate\n' > "$tmp/sensitive"
ln -s "$tmp/sensitive" "$tmp/symlink-lock"
set +e
BLACKBOX_LOCK_FILE="$tmp/symlink-lock" "$flush" --output-dir "$tmp/out" --boot-id boot-A \
  --kernel-source "$tmp/kernel-b.log" --engine-source "$tmp/engine-b.log" > "$tmp/symlink-run.log" 2>&1
symlink_rc=$?
set -e
[[ $symlink_rc -ne 0 ]]
grep -q 'REJECT lock must be a pre-created regular file' "$tmp/symlink-run.log"
grep -qx 'do-not-truncate' "$tmp/sensitive"

# Deterministic hard-stop window: the old current snapshot has been rotated to
# previous-boot.log, but the new current snapshot has not been renamed in.
BLACKBOX_TEST_MODE=1 BLACKBOX_LOCK_FILE="$tmp/lock" "$flush" \
  --output-dir "$tmp/out" --boot-id boot-B \
  --kernel-source "$tmp/kernel-b.log" --engine-source "$tmp/engine-b.log" \
  --test-pause-after-rotate 30 > "$tmp/killed-run.log" 2>&1 &
killed_pid=$!
for _ in $(seq 1 100); do
  grep -q 'TEST_PAUSE after rotate' "$tmp/killed-run.log" 2>/dev/null && break
  sleep 0.02
done
grep -q 'TEST_PAUSE after rotate' "$tmp/killed-run.log"
kill -0 "$killed_pid"
kill -KILL "$killed_pid"
wait "$killed_pid" 2>/dev/null || true

[[ ! -e "$tmp/out/current.log" ]]
after_hash=$(sha256sum "$tmp/out/previous-boot.log" | awk '{print $1}')
[[ $after_hash == "$before_hash" ]]
grep -q 'kernel-A-200' "$tmp/out/previous-boot.log"
[[ -f "$tmp/out/.current.log.new" ]]

run_flush boot-B "$tmp/kernel-b.log" "$tmp/engine-b.log"
grep -q 'kernel-B-200' "$tmp/out/current.log"
mtime_before=$(stat -c %Y "$tmp/out/current.log")
sleep 1
unchanged_output=$(run_flush boot-B "$tmp/kernel-b.log" "$tmp/engine-b.log")
mtime_after=$(stat -c %Y "$tmp/out/current.log")
[[ $mtime_after == "$mtime_before" ]]
grep -q 'UNCHANGED' <<<"$unchanged_output"

run_flush boot-C "$tmp/kernel-a.log" "$tmp/engine-a.log"
grep -q '^BOOT_ID=boot-B$' "$tmp/out/previous-boot.log"
grep -q 'kernel-B-200' "$tmp/out/previous-boot.log"
grep -q '^BOOT_ID=boot-C$' "$tmp/out/current.log"
file_count=$(find "$tmp/out" -maxdepth 1 -type f -name '*.log' | wc -l)
total_bytes=$(( $(stat -c %s "$tmp/out/current.log") + $(stat -c %s "$tmp/out/previous-boot.log") ))
[[ $file_count -eq 2 && $total_bytes -le 1048576 ]]

printf 'PASS blackbox hard-stop: killed_pid=%d prior_hash=%s readable=yes\n' "$killed_pid" "$after_hash"
printf 'PASS blackbox bounds: files=%d total_bytes=%d unchanged_write=no previous_boot=retained controls=stripped tab=kept symlink_lock=rejected\n' "$file_count" "$total_bytes"
