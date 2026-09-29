#!/usr/bin/env bash
# Set Bluetooth soft-unblocked now and persist the same systemd-rfkill state.
set -euo pipefail

sysfs_root=/sys/class/rfkill
state_dir=/var/lib/systemd/rfkill
rfkill_command=rfkill
settle_seconds=${RFKILL_SETTLE_SECONDS:-6}
dry_run=0
verify_only=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) dry_run=1; shift ;;
    --verify) verify_only=1; shift ;;
    --sysfs-root) sysfs_root=${2:?missing sysfs root}; shift 2 ;;
    --state-dir) state_dir=${2:?missing state directory}; shift 2 ;;
    --rfkill-command) rfkill_command=${2:?missing rfkill command}; shift 2 ;;
    -h|--help)
      printf 'Usage: %s [--dry-run|--verify] [test path overrides]\n' "$0"
      exit 0
      ;;
    *) printf 'REJECT unknown option: %s\n' "$1" >&2; exit 2 ;;
  esac
done
[[ $settle_seconds =~ ^[0-9]+$ ]] || { printf 'REJECT invalid settle seconds\n' >&2; exit 2; }

bluetooth_dirs=()
for rf_type in "$sysfs_root"/rfkill*/type; do
  [[ -r $rf_type ]] || continue
  [[ $(<"$rf_type") == bluetooth ]] && bluetooth_dirs+=("${rf_type%/type}")
done
if [[ ${#bluetooth_dirs[@]} -eq 0 ]]; then
  printf 'STOP no Bluetooth rfkill index present; persisted state cannot be established\n' >&2
  exit 3
fi

if [[ $dry_run -eq 1 ]]; then
  printf 'DRY-RUN would run: %s unblock bluetooth\n' "$rfkill_command"
  printf 'DRY-RUN would atomically write 0 to existing %s/*:bluetooth state files\n' "$state_dir"
  for rf_dir in "${bluetooth_dirs[@]}"; do
    printf 'DRY-RUN Bluetooth index: %s current_soft=%s\n' "$(basename "$rf_dir")" "$(<"$rf_dir/soft")"
  done
  exit 0
fi

if [[ $verify_only -eq 0 ]]; then
  command -v "$rfkill_command" >/dev/null 2>&1 || { printf 'REJECT rfkill command missing: %s\n' "$rfkill_command" >&2; exit 1; }
  "$rfkill_command" unblock bluetooth
  sleep "$settle_seconds"
fi

mapfile -d '' state_files < <(find "$state_dir" -maxdepth 1 -type f \
  \( -name '*:bluetooth' -o -name bluetooth \) -print0 2>/dev/null)
[[ ${#state_files[@]} -gt 0 ]] || { printf 'REJECT systemd-rfkill has no Bluetooth restore-state file\n' >&2; exit 1; }

if [[ $verify_only -eq 0 ]]; then
  for state in "${state_files[@]}"; do
    tmp_state="${state}.brrdfeeder-new"
    printf '0\n' > "$tmp_state"
    chmod 0644 "$tmp_state"
    mv -f "$tmp_state" "$state"
  done
fi

for rf_dir in "${bluetooth_dirs[@]}"; do
  [[ -r $rf_dir/soft && $(<"$rf_dir/soft") == 0 ]] || { printf 'REJECT %s remains soft-blocked\n' "$(basename "$rf_dir")" >&2; exit 1; }
done
for state in "${state_files[@]}"; do
  [[ $(tr -d '\n' < "$state") == 0 ]] || { printf 'REJECT restore state remains blocked: %s\n' "$state" >&2; exit 1; }
done
printf 'PASS Bluetooth unblocked now and persisted across reboot indices=%d state_files=%d\n' "${#bluetooth_dirs[@]}" "${#state_files[@]}"
