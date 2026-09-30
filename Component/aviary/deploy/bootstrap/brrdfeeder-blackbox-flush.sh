#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Persist a tiny BRRDfeeder forensic tail while journald itself stays volatile.
set -euo pipefail

output_dir=/var/lib/brrdfeeder/blackbox
kernel_source=
engine_source=
boot_id=
dry_run=0
test_pause=0
max_snapshot_bytes=262144
max_lines=200
max_line_bytes=512

usage() {
  cat <<'EOF'
Usage: brrdfeeder-blackbox-flush.sh [options]
  --output-dir DIR       destination (default /var/lib/brrdfeeder/blackbox)
  --kernel-source FILE   test/fixture source instead of the kernel journal
  --engine-source FILE   test/fixture source instead of the engine journal
  --boot-id ID           test boot id instead of /proc boot_id
  --dry-run              print the plan; read and write nothing

The test-only --test-pause-after-rotate SECONDS option is accepted only when
BLACKBOX_TEST_MODE=1. Production callers must not set that environment variable.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output-dir) output_dir=${2:?missing output directory}; shift 2 ;;
    --kernel-source) kernel_source=${2:?missing kernel source}; shift 2 ;;
    --engine-source) engine_source=${2:?missing engine source}; shift 2 ;;
    --boot-id) boot_id=${2:?missing boot id}; shift 2 ;;
    --dry-run) dry_run=1; shift ;;
    --test-pause-after-rotate) test_pause=${2:?missing pause}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'REJECT unknown option: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ $dry_run -eq 1 ]]; then
  printf '[blackbox] dry-run: read last %d kernel + %d engine lines\n' "$max_lines" "$max_lines"
  printf '[blackbox] dry-run: cap lines at %d bytes and snapshot at %d bytes\n' "$max_line_bytes" "$max_snapshot_bytes"
  printf '[blackbox] dry-run: update only changed content in %s; keep current.log + previous-boot.log; syncfs once\n' "$output_dir"
  exit 0
fi

if [[ $test_pause != 0 && ${BLACKBOX_TEST_MODE:-0} != 1 ]]; then
  printf 'REJECT test pause requires BLACKBOX_TEST_MODE=1\n' >&2
  exit 2
fi
[[ $test_pause =~ ^[0-9]+$ ]] || { printf 'REJECT pause must be whole seconds\n' >&2; exit 2; }

if [[ -z $boot_id ]]; then
  read -r boot_id < /proc/sys/kernel/random/boot_id
fi
[[ $boot_id =~ ^[A-Za-z0-9._:-]+$ ]] || { printf 'REJECT invalid boot id\n' >&2; exit 2; }

for source in "$kernel_source" "$engine_source"; do
  [[ -z $source || -r $source ]] || { printf 'REJECT unreadable source: %s\n' "$source" >&2; exit 2; }
done

tmp=$(mktemp "${TMPDIR:-/tmp}/brrdfeeder-blackbox.XXXXXX")
next=
cleanup() {
  rm -f "$tmp"
  [[ -z $next ]] || rm -f "$next"
}
trap cleanup EXIT

truncate_lines() {
  LC_ALL=C awk -v max="$max_line_bytes" '{
    # Strip C0/DEL controls but retain horizontal tab (octal 011).
    gsub(/[\001-\010\013-\037\177]/, "");
    if (length($0) > max) print substr($0, 1, max - 3) "...";
    else print;
  }'
}

{
  printf 'BRRDFEEDER_BLACKBOX_V1\n'
  printf 'BOOT_ID=%s\n' "$boot_id"
  printf '%s\n' '--- kernel tail (last 200) ---'
  if [[ -n $kernel_source ]]; then
    tail -n "$max_lines" "$kernel_source" | truncate_lines
  else
    journalctl -k -n "$max_lines" --no-pager --output=short-iso 2>&1 | truncate_lines || true
  fi
  printf '%s\n' '--- engine tail (last 200) ---'
  if [[ -n $engine_source ]]; then
    tail -n "$max_lines" "$engine_source" | truncate_lines
  else
    journalctl -n "$max_lines" --no-pager --output=short-iso \
      _SYSTEMD_UNIT=brrdfeeder-engine.service + \
      _SYSTEMD_USER_UNIT=brrdfeeder-engine.service 2>&1 | truncate_lines || true
  fi
} > "$tmp"

snapshot_bytes=$(stat -c %s "$tmp")
if (( snapshot_bytes > max_snapshot_bytes )); then
  printf 'REJECT snapshot is %d bytes; hard cap is %d\n' "$snapshot_bytes" "$max_snapshot_bytes" >&2
  exit 1
fi

current="$output_dir/current.log"
previous="$output_dir/previous-boot.log"
if [[ -f $current ]] && cmp -s "$tmp" "$current"; then
  printf '[blackbox] UNCHANGED bytes=%d; no persistent write\n' "$snapshot_bytes"
  exit 0
fi

install -d -m 0700 "$output_dir"
lock_file=${BLACKBOX_LOCK_FILE:-${output_dir}/.flush.lock}
if [[ -L $lock_file || ! -f $lock_file ]]; then
  printf 'REJECT lock must be a pre-created regular file: %s\n' "$lock_file" >&2
  exit 1
fi
[[ $(stat -c %a "$lock_file") == 600 ]] || { printf 'REJECT lock mode must be 0600: %s\n' "$lock_file" >&2; exit 1; }
# Production's lock lives in the root-owned 0700 output directory and is
# created by the installer. Append-open avoids truncation; the explicit symlink
# refusal prevents test overrides from turning this into a privileged write.
exec 9>>"$lock_file"
flock -x 9

# Another timer invocation may have committed while this one waited.
if [[ -f $current ]] && cmp -s "$tmp" "$current"; then
  printf '[blackbox] UNCHANGED-after-lock bytes=%d; no persistent write\n' "$snapshot_bytes"
  exit 0
fi

# A fixed staging name keeps repeated hard stops bounded to one candidate.
# The exclusive lock prevents concurrent writers from sharing it.
next="$output_dir/.current.log.new"
install -m 0600 "$tmp" "$next"
old_boot_id=
if [[ -f $current ]]; then
  old_boot_id=$(sed -n 's/^BOOT_ID=//p' "$current" | head -1)
fi
if [[ -n $old_boot_id && $old_boot_id != "$boot_id" ]]; then
  rm -f "$previous"
  mv "$current" "$previous"
  if (( test_pause > 0 )); then
    printf '[blackbox] TEST_PAUSE after rotate pid=%d seconds=%d\n' "$$" "$test_pause"
    # Keep the pause inside this process so SIGKILL does not leave a child
    # holding the test harness's output descriptor open.
    pause_until=$((SECONDS + test_pause))
    while (( SECONDS < pause_until )); do :; done
  fi
fi
mv -f "$next" "$current"
next=

# One syncfs call commits file data and directory metadata together. This is the
# only forced flush per changed snapshot; unchanged content does no SD write.
sync -f "$output_dir"

total_bytes=$(stat -c %s "$current")
if [[ -f $previous ]]; then
  total_bytes=$((total_bytes + $(stat -c %s "$previous")))
fi
if (( total_bytes > 1048576 )); then
  printf 'REJECT blackbox total %d exceeds 1 MiB invariant\n' "$total_bytes" >&2
  exit 1
fi
printf '[blackbox] PASS boot=%s bytes=%d total=%d files<=2 syncfs=1\n' "$boot_id" "$snapshot_bytes" "$total_bytes"
