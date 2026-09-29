#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Self-heal: if invoked via `sh script` (dash on Debian-derived) the bash
# shebang is ignored. Re-exec under bash so bash-specific features
# (set -u + EUID + [[ ]] + process substitution) work.
if [ -z "${BASH_VERSION:-}" ]; then
  exec bash "$0" "$@"
fi

# brrdfeeder-install.sh — legacy device naming and service setup
#
# Sets up device symlinks and a user service, or refreshes an existing Quadlet.
# Requires administrator approval and sudo.
#
# Purpose:
#   One privileged invocation configures:
#     * Stable, vendor-agnostic device symlinks via udev
#                   (/dev/cybrrd_gps, /dev/cybrrd_ble)
#     * A systemd --user unit (brrdfeeder-engine.service) so the
#                   engine survives reboot / power-cycle / SSH disconnect
#
# Substrate-truth this addresses:
#   * 2026-06-04 lightning + 2026-06-06 storm both flipped USB CDC-ACM
#     enumeration order between u-blox GPS and Nordic BLE
#   * The engine
#     needs systemd-managed lifecycle (Restart=always, TimeoutStopSec=10)
#     so a SIGTERM-survivor incident cannot strand the platform offline
#
# Scope:
#   * Refreshes an existing rootful Quadlet, preserving its Image= verbatim.
#     Image swaps remain a separate roll-engine.sh operation.
#   * Does NOT touch the engine binary itself (binary is already built +
#     setcap'd; we only manage process lifecycle and device naming)
#   * Only updates the selected config's legacy GPS device path.
#
# Idempotency: safe to re-run. Each step checks state before mutation.
#
# Usage:
#   sudo bash /home/synth/brrdfeeder-install.sh           # apply
#   sudo bash /home/synth/brrdfeeder-install.sh --dry-run # plan-only
#   sudo bash /home/synth/brrdfeeder-install.sh --verify  # check state only

set -euo pipefail

# ----------------------------------------------------------------------
# Constants (vendor-AGNOSTIC abstraction layer)
# ----------------------------------------------------------------------
readonly TARGET_USER="synth"
readonly TARGET_HOME="/home/synth"

# Vendor-agnostic symlink names — these are the substrate-stable paths
# that engine code / Quadlet volume mounts will reference. When we
# swap GPS vendor, only the udev ATTRS{} change; the
# symlinks (and therefore every downstream consumer) stay stable.
readonly GPS_SYMLINK="cybrrd_gps"
readonly BLE_SYMLINK="cybrrd_ble"

# u-blox 7 — substrate-empirically observed 2026-06-06 via lsusb
readonly UBLOX_VENDOR="1546"
readonly UBLOX_PRODUCT="01a7"

# Nordic Semiconductor nRF52 Connectivity — substrate-empirically observed
readonly NORDIC_VENDOR="1915"
readonly NORDIC_PRODUCT="c00a"

readonly UDEV_RULES_FILE="/etc/udev/rules.d/99-cybrrd-brrdfeeder.rules"
readonly SYSTEMD_UNIT_FILE="${TARGET_HOME}/.config/systemd/user/brrdfeeder-engine.service"
readonly ENGINE_BINARY="${TARGET_HOME}/brrdfeeder-src/engine/target/release/engine"
readonly QUADLET_FILE="/etc/containers/systemd/brrdfeeder-engine.container"
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
readonly SCRIPT_DIR
readonly BLACKBOX_SOURCE="${SCRIPT_DIR}/brrdfeeder-blackbox-flush.sh"
readonly RFKILL_BOOT_SOURCE="${SCRIPT_DIR}/brrdfeeder-rfkill-boot-state.sh"
readonly BLACKBOX_INSTALL="/usr/local/libexec/brrdfeeder-blackbox-flush"
readonly IDENTITY_SOURCE="${SCRIPT_DIR}/brrdfeeder-image-identity.sh"
readonly IDENTITY_INSTALL="/usr/local/libexec/brrdfeeder-image-identity"
readonly BLACKBOX_DIR="/var/lib/brrdfeeder/blackbox"
readonly BLACKBOX_LOCK="${BLACKBOX_DIR}/.flush.lock"
readonly BLACKBOX_SERVICE="/etc/systemd/system/brrdfeeder-blackbox.service"
readonly BLACKBOX_TIMER="/etc/systemd/system/brrdfeeder-blackbox.timer"

# Parse flags
DRY_RUN=0
VERIFY_ONLY=0
RESTART_ENGINE=0
CONFIG_FILE=""
RECEIPT=""
AUDIT_LOG=""
AUDIT_PID=""
ORIGINAL_ARGS="$*"
while [[ $# -gt 0 ]]; do
  arg=$1
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --verify)  VERIFY_ONLY=1 ;;
    --restart-engine) RESTART_ENGINE=1 ;;
    --config)
      [[ $# -ge 2 && -n $2 ]] || { echo 'FATAL: --config requires a path' >&2; exit 2; }
      CONFIG_FILE=$2
      shift
      ;;
    --audit-log=*) AUDIT_LOG="${arg#*=}" ;;
    --audit-log)
      echo "FATAL: --audit-log requires a path. Example: --audit-log=/home/synth/brrdfeeder-install-audit.log" >&2
      exit 2
      ;;
    *) echo "Unknown flag: $arg"; exit 2 ;;
  esac
  shift
done

# A preview never creates an audit file (including its parent directories).
if [[ $VERIFY_ONLY -eq 1 || $DRY_RUN -eq 1 ]]; then
  [[ -z $AUDIT_LOG ]] || echo "[read-only] audit output stays on stdout: $AUDIT_LOG"
  AUDIT_LOG=""
fi

# ----------------------------------------------------------------------
# Audit-log setup (Gemini directive 2026-06-07 — BRRDfeeder hobbyist
# distribution requires comprehensive substrate-state capture)
# ----------------------------------------------------------------------
audit_begin() {
if [[ -n "$AUDIT_LOG" ]]; then
  AUDIT_DIR="$(dirname "$AUDIT_LOG")"
  [[ -d "$AUDIT_DIR" ]] || mkdir -p "$AUDIT_DIR"
  # Open audit log fd 3; mirror stdout+stderr through tee so the user sees
  # output live AND it's captured for forensic replay.
  exec 3>&1 4>&2
  exec > >(tee -a "$AUDIT_LOG") 2>&1
  AUDIT_PID=$!

  cat <<HEADER
==========================================================================
BRRDfeeder install script — AUDIT LOG
==========================================================================
Started:   $(date -Iseconds)
Host:      $(hostname)
Script:    $0
Args:      $ORIGINAL_ARGS
PID:       $$
==========================================================================

--- uname -a ---
$(uname -a)

--- /etc/os-release ---
$(cat /etc/os-release 2>/dev/null || echo "(unavailable)")

--- /proc/cpuinfo (board identity lines only) ---
$(grep -E "^(Model|Hardware|Revision|Serial|processor[[:space:]]+: 0)" /proc/cpuinfo 2>/dev/null || echo "(no Pi-style identity lines)")

--- lsusb ---
$(lsusb 2>&1)

--- df -h ---
$(df -h 2>&1)

--- free -m ---
$(free -m 2>&1)

--- mount (existing tmpfs landings) ---
$(mount | grep -E "^tmpfs|on /run|on /tmp" 2>&1)

==========================================================================
BEGIN EXECUTION TRACE
==========================================================================
HEADER

  # Enable command tracing with timestamp + file:line context.
  PS4='+ [$(date +%H:%M:%S)] ${BASH_SOURCE##*/}:${LINENO}: '
  set -x
fi
}

# ----------------------------------------------------------------------
# Persistent black-box diagnostics. previous-boot.log is retained across
# every flush in the current boot; before the first flush after reboot,
# current.log itself is still the prior boot's last durable snapshot.
# ----------------------------------------------------------------------
print_previous_blackbox() {
  local candidate="" current_boot saved_boot
  current_boot=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true)
  if [[ -r "${BLACKBOX_DIR}/previous-boot.log" ]]; then
    candidate="${BLACKBOX_DIR}/previous-boot.log"
  elif [[ -r "${BLACKBOX_DIR}/current.log" ]]; then
    saved_boot=$(sed -n 's/^BOOT_ID=//p' "${BLACKBOX_DIR}/current.log" | head -1)
    [[ -n "$saved_boot" && "$saved_boot" != "$current_boot" ]] && candidate="${BLACKBOX_DIR}/current.log"
  fi
  if [[ -n "$candidate" ]]; then
    echo "source: $candidate"
    tail -80 "$candidate"
  else
    echo "(no previous-boot black-box tail)"
  fi
}

# ----------------------------------------------------------------------
# Audit-log footer (trap on EXIT — fires on success, failure, or early
# fatal exit) — captures post-flight substrate state regardless of why
# the script exited.
# ----------------------------------------------------------------------
audit_footer() {
  local rc=${1:-$?}
  if [[ -n "${AUDIT_LOG:-}" ]]; then
    { set +x; } 2>/dev/null
    echo "=========================================================================="
    echo "END EXECUTION TRACE"
    echo "=========================================================================="
    echo "Exit status:    $rc"
    echo "Completed:      $(date -Iseconds)"
    echo
    # Post-flight state only meaningful if we got past pre-flight (TARGET_UID is set then)
    if [[ -n "${TARGET_UID:-}" ]]; then
      if [[ ${SUBSTRATE:-legacy} == quadlet ]]; then
        echo "--- systemctl status brrdfeeder-engine.service ---"
        systemctl status brrdfeeder-engine.service --no-pager 2>&1 | head -25 || true
        echo "--- journalctl -u brrdfeeder-engine.service -n 10 ---"
        journalctl -u brrdfeeder-engine.service -n 10 --no-pager 2>&1 || true
      else
      echo "--- systemctl --user status brrdfeeder-engine.service ---"
      sudo -u "$TARGET_USER" \
        XDG_RUNTIME_DIR="/run/user/$TARGET_UID" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$TARGET_UID/bus" \
        systemctl --user status brrdfeeder-engine.service --no-pager 2>&1 | head -25 || true
      echo
      echo "--- journalctl --user -u brrdfeeder-engine.service -n 10 ---"
      sudo -u "$TARGET_USER" \
        XDG_RUNTIME_DIR="/run/user/$TARGET_UID" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$TARGET_UID/bus" \
        journalctl --user -u brrdfeeder-engine.service -n 10 --no-pager 2>&1 || true
      fi
      echo
      echo "--- /dev/cybrrd_* symlinks ---"
      ls -la /dev/cybrrd_* 2>&1 || true
      echo
      echo "--- mount | grep ${TARGET_HOME:-/home/synth}/capture ---"
      mount | grep -E "tmpfs on ${TARGET_HOME:-/home/synth}/capture" || echo "(not tmpfs-mounted — Standard/Commercial tier or no Step 1.5)"
      echo
      for dropin in /etc/systemd/journald.conf.d/99-brrdfeeder.conf /etc/systemd/journald.conf.d/99-brrdfeeder-open.conf; do
        echo "--- $dropin ---"
        cat "$dropin" 2>/dev/null || echo "(not installed)"
      done
      echo
      echo "--- BRRDfeeder black box: previous-boot tail ---"
      print_previous_blackbox
      echo
      echo "--- Bluetooth rfkill restore state ---"
      for state in /var/lib/systemd/rfkill/*:bluetooth /var/lib/systemd/rfkill/bluetooth; do
        [[ -e "$state" ]] && printf '%s=' "$state" && cat "$state"
      done
    else
      echo "(post-flight state capture skipped — script exited before pre-flight completed)"
    fi
    echo
    echo "=========================================================================="
    echo "Audit log saved to: $AUDIT_LOG"
    echo "=========================================================================="
  fi
}
trap audit_footer EXIT

# ----------------------------------------------------------------------
# Substrate-truthful output helpers
# ----------------------------------------------------------------------
say()   { echo "[brrdfeeder-install] $*"; }
gate()  { echo "[brrdfeeder-install] >>> $*"; }
ok()    { echo "[brrdfeeder-install]  OK $*"; }
warn()  { echo "[brrdfeeder-install]  !! $*" >&2; }
fatal() { echo "[brrdfeeder-install] FATAL $*" >&2; exit 1; }

# Parse the top-level node.storage_class scalar from config.yaml.
# Returns "ephemeral" (BRRDfeeder tier) or "persistent" (Standard/Commercial)
# or empty string if absent. Defaults to "persistent" for backward
# compat with cardinal Standard installs.
get_storage_class() {
  local cfg="$1"
  [[ -f "$cfg" ]] || { echo ""; return; }
  awk '
    /^node:/                                { in_node=1; next }
    in_node && /^[a-z]/                     { in_node=0 }
    in_node && /^[[:space:]]+storage_class:/ {
      val=$2; gsub(/["'"'"'[:space:]]/,"",val); print val; exit
    }
  ' "$cfg"
}

run() {
  if [[ $DRY_RUN -eq 1 || $VERIFY_ONLY -eq 1 ]]; then
    echo "[dry-run]  + $*"
  else
    "$@"
  fi
}

# Publish complete files only: a failed write/chmod/chown/rename leaves the old
# target intact. The temporary lives on the target filesystem for atomic rename.
atomic_write() {
  local target=$1 content=$2 owner=${3:-root} group=${4:-root} temporary
  if [[ $DRY_RUN -eq 1 || $VERIFY_ONLY -eq 1 ]]; then
    say "[dry-run] would atomically write $target"
    return
  fi
  temporary=$(mktemp "${target}.installer.XXXXXX") || fatal "cannot stage $target"
  if ! { printf '%s\n' "$content" > "$temporary" &&
         chmod 0644 "$temporary" && chown "$owner:$group" "$temporary" &&
         mv -fT -- "$temporary" "$target"; }; then
    rm -f -- "$temporary"
    fatal "atomic write failed; original target retained: $target"
  fi
}

# Snapshot all potential file mutations before the first host change. Receipts
# are private because configs, sudoers and retained journals may be sensitive.
# Group databases are evidence only: restoring them wholesale could erase
# unrelated account changes. ROLLBACK prints the narrow manual inverse instead.
receipt_begin() {
  local path index=0 state
  RECEIPT="/var/lib/brrdfeeder-deploy/$(date -u +%Y%m%dT%H%M%S.%N)-$$/installer"
  if [[ $DRY_RUN -eq 1 || $VERIFY_ONLY -eq 1 ]]; then
    say "[dry-run] would create receipt: $RECEIPT"
    RECEIPT=""
    return
  fi
  umask 077
  install -d -m 0700 "$RECEIPT" "$RECEIPT/files"
  say "receipt: $RECEIPT"
  getent group dialout > "$RECEIPT/dialout.before"
  id "$TARGET_USER" > "$RECEIPT/user.before"
  for path in /etc/sudoers /etc/sudoers.d/*; do
    [[ -f $path ]] || continue
    cp -a --parents "$path" "$RECEIPT/"
  done
  : > "$RECEIPT/FILES"
  for path in "${MUTATION_PATHS[@]}"; do
    index=$((index + 1))
    state=absent
    if [[ -e $path || -L $path ]]; then
      [[ -f $path && ! -L $path ]] || fatal "refusing non-regular mutation target: $path"
      cp -a -- "$path" "$RECEIPT/files/$index"
      state=$(sha256sum "$path" | cut -d ' ' -f 1)
    fi
    printf '%s\t%s\t%s\n' "$index" "$path" "$state" >> "$RECEIPT/FILES"
  done
  for state in enabled active; do
    if systemctl "is-$state" brrdfeeder-blackbox.timer >/dev/null 2>&1; then
      printf 'yes\n' > "$RECEIPT/timer.$state"
    else
      printf 'no\n' > "$RECEIPT/timer.$state"
    fi
  done
  cat > "$RECEIPT/ROLLBACK.sh" <<'ROLLBACK'
#!/usr/bin/env bash
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo 'Run rollback as root' >&2; exit 1; }
receipt=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# Stop writers before restoring the black-box files and configuration.
systemctl stop brrdfeeder-blackbox.timer brrdfeeder-blackbox.service || true
systemctl disable brrdfeeder-blackbox.timer || true
while IFS=$'\t' read -r index path before; do
  if [[ $before == absent ]]; then
    rm -f -- "$path"
  else
    cp -a --remove-destination -- "$receipt/files/$index" "$path"
  fi
done < "$receipt/FILES"
systemctl daemon-reload
udevadm control --reload-rules
udevadm trigger --subsystem-match=tty --action=change
udevadm trigger --subsystem-match=misc --sysname-match=rfkill --action=change
systemctl restart systemd-journald
if [[ $(<"$receipt/timer.enabled") == yes ]]; then systemctl enable brrdfeeder-blackbox.timer; fi
if [[ $(<"$receipt/timer.active") == yes ]]; then systemctl start brrdfeeder-blackbox.timer; fi
echo 'Files restored. Engine was NOT restarted; inspect restored unit and perform a health-gated restart.'
echo 'Manual runtime undo: inspect/unmount the capture tmpfs if newly mounted; restore Bluetooth soft-block policy if needed; review linger/session changes. Empty directories and receipt logs are retained.'
if ! grep -Eq '(^|[:,])synth(,|$)' "$receipt/dialout.before"; then
  echo 'Manual group undo, ONLY if appropriate now: gpasswd -d synth dialout; restart the user manager/session.'
fi
echo 'Sudoers was not modified; its private pre-install copy is evidence only.'
echo 'For the legacy user service, also run systemctl --user daemon-reload in the synth session.'
ROLLBACK
  chmod 0700 "$RECEIPT/ROLLBACK.sh"
}

receipt_finish() {
  local index path before after action
  [[ -n $RECEIPT && -f $RECEIPT/FILES ]] || return 0
  printf 'path\taction\tsha256_before\tsha256_after\n' > "$RECEIPT/MANIFEST"
  while IFS=$'\t' read -r index path before; do
    after=absent
    [[ ! -f $path ]] || after=$(sha256sum "$path" | cut -d ' ' -f 1)
    action=unchanged
    if [[ $before != "$after" ]]; then
      action=replace
      [[ $before != absent ]] || action=create
      [[ $after != absent ]] || action=remove
    fi
    printf '%s\t%s\t%s\t%s\n' "$path" "$action" "$before" "$after" >> "$RECEIPT/MANIFEST"
  done < "$RECEIPT/FILES"
  getent group dialout > "$RECEIPT/dialout.after"
  printf '/etc/group:dialout\tmanual-membership\t%s\t%s\n' \
    "$(sha256sum "$RECEIPT/dialout.before" | cut -d ' ' -f 1)" \
    "$(sha256sum "$RECEIPT/dialout.after" | cut -d ' ' -f 1)" >> "$RECEIPT/MANIFEST"
  say "receipt: $RECEIPT (rollback: sudo bash $RECEIPT/ROLLBACK.sh)"
}

# Drain the audit writer before hashing the final log. Final receipt messages
# go to the original stdout, so the recorded after-hash stays accurate.
installer_exit() {
  local rc=$?
  trap - EXIT
  set +e
  audit_footer "$rc"
  if [[ -n $AUDIT_PID ]]; then
    exec 1>&3 2>&4
    wait "$AUDIT_PID" || rc=1
  fi
  receipt_finish || rc=1
  exit "$rc"
}
trap installer_exit EXIT

# ----------------------------------------------------------------------
# Pre-flight substrate-truth checks
# ----------------------------------------------------------------------
gate "Pre-flight"

[[ $EUID -eq 0 ]] || fatal "Administrator privileges required. Run: sudo bash $0"

id "$TARGET_USER" >/dev/null 2>&1 || fatal "User '$TARGET_USER' does not exist on this host."
TARGET_UID=$(id -u "$TARGET_USER")
TARGET_GID=$(id -g "$TARGET_USER")
ok "target user $TARGET_USER (uid=$TARGET_UID gid=$TARGET_GID)"

CONFIG_CANDIDATES=(/etc/brrdfeeder/config.yaml /home/synth/brrdfeeder/config.yaml /home/synth/config.yaml)
if [[ -n $CONFIG_FILE ]]; then
  CONFIG_REASON='explicit --config'
else
  CONFIG_REASON='first existing candidate in documented order'
  for candidate in "${CONFIG_CANDIDATES[@]}"; do
    if [[ -f $candidate ]]; then CONFIG_FILE=$candidate; break; fi
  done
fi
if [[ -z $CONFIG_FILE || ! -f $CONFIG_FILE ]]; then
  printf 'FATAL: config not found (requested=%s); candidates: %s\n' "$CONFIG_FILE" "${CONFIG_CANDIDATES[*]}" >&2
  exit 2
fi
# Paths are also rendered into Quadlet syntax and tab-delimited receipts.
[[ $CONFIG_FILE =~ ^/[a-zA-Z0-9_./-]+$ && ! -L $CONFIG_FILE ]] || { echo "FATAL: unsafe config path: $CONFIG_FILE" >&2; exit 2; }
CONFIG_PARENT=$(dirname -- "$CONFIG_FILE")
CONFIG_PARENT_MODE=$(stat -Lc %a -- "$CONFIG_PARENT")
# Refuse rather than warn: another local user must not replace a validated
# root-consumed config between the checks and the eventual bind/parse/write.
if (( (8#$CONFIG_PARENT_MODE & 0022) != 0 )); then
  echo "FATAL: config parent is group/world-writable: $CONFIG_PARENT (mode=$CONFIG_PARENT_MODE)" >&2
  exit 2
fi
say "selected config: $CONFIG_FILE ($CONFIG_REASON)"
SUBSTRATE=legacy
if [[ -f $QUADLET_FILE ]]; then
  SUBSTRATE=quadlet
  QUADLET_TEMPLATE="${SCRIPT_DIR}/../quadlet/brrdfeeder-engine.container"
  [[ -f $QUADLET_TEMPLATE ]] || QUADLET_TEMPLATE="${SCRIPT_DIR}/brrdfeeder-engine.container"
  [[ -r $QUADLET_TEMPLATE ]] || fatal "missing Quadlet template: $QUADLET_TEMPLATE"
  [[ $(grep -c '^Image=' "$QUADLET_FILE") == 1 ]] || fatal "existing Quadlet must contain exactly one Image= line"
  [[ $(grep -c '^Image=' "$QUADLET_TEMPLATE") == 1 ]] || fatal "template must contain exactly one Image= line"
  # Read the original line as data, never as shell or an awk replacement string.
  NEW_QUADLET=$(awk -v old="$QUADLET_FILE" -v config="$CONFIG_FILE" '
    BEGIN { while ((getline line < old) > 0) if (line ~ /^Image=/) image=line; close(old) }
    /^Image=/ { print image; next }
    /^Volume=\/etc\/brrdfeeder\/config.yaml:/ { sub(/^Volume=\/etc\/brrdfeeder\/config.yaml:/, "Volume=" config ":") }
    { print }
  ' "$QUADLET_TEMPLATE")
  say "substrate: rootful Quadlet; preserving running Image= verbatim"
  diff -u "$QUADLET_FILE" <(printf '%s\n' "$NEW_QUADLET") || [[ $? == 1 ]]
elif [[ -f ${TARGET_HOME}/.config/containers/systemd/brrdfeeder-engine.container ]]; then
  echo 'FATAL: rootless Quadlet detected; refresh is not supported by this rootful fleet installer' >&2
  exit 2
else
  for candidate in /etc/containers/systemd/*.container; do
    [[ -f $candidate ]] || continue
    if LC_ALL=C grep -qi 'brrdfeeder' "$candidate"; then
      echo "FATAL: refusing legacy mode; possible renamed BRRDfeeder Quadlet: $candidate" >&2
      exit 2
    fi
  done
  if systemctl is-active --quiet brrdfeeder-engine.service; then
    echo 'FATAL: refusing legacy mode; system brrdfeeder-engine.service is active' >&2
    exit 2
  fi
  warn 'DEPRECATED: legacy bare-metal user-service path; fleet nodes use rootful Quadlets'
  # The historical binary reads config.yaml relative to WorkingDirectory=%h.
  # Do not imply that --config can relocate the legacy engine's runtime input.
  if [[ $CONFIG_FILE != "$TARGET_HOME/config.yaml" ]]; then
    echo "FATAL: legacy engine requires --config $TARGET_HOME/config.yaml; selected $CONFIG_FILE" >&2
    exit 2
  fi
fi

STORAGE_CLASS="$(get_storage_class "$CONFIG_FILE")"
[[ -z "$STORAGE_CLASS" ]] && STORAGE_CLASS="persistent"
case "$STORAGE_CLASS" in
  ephemeral|persistent) ;;
  *) SAFE_STORAGE_CLASS=$(printf '%s' "$STORAGE_CLASS" | LC_ALL=C tr -d '[:cntrl:]')
     fatal "node.storage_class must be ephemeral|persistent, got '$SAFE_STORAGE_CLASS'" ;;
esac
ok "config node.storage_class=$STORAGE_CLASS"

if [[ $SUBSTRATE == legacy ]]; then
[[ -x "$ENGINE_BINARY" ]] || fatal "Engine binary missing or not executable: $ENGINE_BINARY"
ok "engine binary present + executable"

if ! getcap "$ENGINE_BINARY" | grep -q "cap_net_admin"; then
  warn "engine binary missing capabilities — capture may fail. (Outside this script's scope; fix with: sudo setcap cap_net_admin,cap_net_raw,cap_sys_time=ep $ENGINE_BINARY)"
else
  ok "engine binary has cap_net_admin + cap_net_raw set"
fi
fi

# Verify hardware is actually attached (substrate-truth before claiming we can name it)
HAVE_UBLOX=0
HAVE_NORDIC=0
if lsusb -d "${UBLOX_VENDOR}:${UBLOX_PRODUCT}" >/dev/null 2>&1; then
  HAVE_UBLOX=1
  ok "u-blox 7 detected on USB (${UBLOX_VENDOR}:${UBLOX_PRODUCT})"
else
  warn "u-blox 7 NOT detected on USB. The udev rule will still be installed (will fire when device is attached)."
fi
if lsusb -d "${NORDIC_VENDOR}:${NORDIC_PRODUCT}" >/dev/null 2>&1; then
  HAVE_NORDIC=1
  ok "Nordic nRF52 Connectivity detected on USB (${NORDIC_VENDOR}:${NORDIC_PRODUCT})"
else
  warn "Nordic nRF52 NOT detected on USB. The udev rule will still be installed (will fire when device is attached)."
fi

if [[ $VERIFY_ONLY -eq 1 ]]; then
  gate "Verify-only mode — checking current substrate state"
  if [[ -f "$UDEV_RULES_FILE" ]]; then ok "udev rules file present: $UDEV_RULES_FILE"; else warn "udev rules file ABSENT"; fi
  if [[ -L "/dev/$GPS_SYMLINK" ]]; then ok "/dev/$GPS_SYMLINK -> $(readlink -f /dev/$GPS_SYMLINK)"; else warn "/dev/$GPS_SYMLINK MISSING"; fi
  if [[ -L "/dev/$BLE_SYMLINK" ]]; then ok "/dev/$BLE_SYMLINK -> $(readlink -f /dev/$BLE_SYMLINK)"; else warn "/dev/$BLE_SYMLINK MISSING"; fi
  if [[ $SUBSTRATE == quadlet ]]; then
    ok "rootful Quadlet present: $QUADLET_FILE"
  else
    if [[ -f "$SYSTEMD_UNIT_FILE" ]]; then ok "systemd user-unit file present"; else warn "systemd user-unit file ABSENT"; fi
  fi
  if ! id -nG "$TARGET_USER" | tr ' ' '\n' | grep -qx dialout; then
    warn "$TARGET_USER needs dialout membership (verify makes no change)"
  fi
  if loginctl show-user "$TARGET_USER" 2>/dev/null | grep -q "Linger=yes"; then ok "user-linger is ON for $TARGET_USER"; else warn "user-linger is OFF for $TARGET_USER"; fi

  # Step 1.5 — BRRDfeeder-tier hardening state
  say "config node.storage_class = $STORAGE_CLASS"
  if [[ -f "/etc/systemd/journald.conf.d/99-brrdfeeder.conf" ]]; then
    ok "journald drop-in present (BRRDfeeder hardening installed)"
  elif [[ -f "/etc/systemd/journald.conf.d/99-brrdfeeder-open.conf" ]]; then
    ok "legacy journald drop-in present (BRRDfeeder hardening installed; re-run migrates it)"
  else
    say "journald drop-in absent (Standard/Commercial — disk-persistent journal)"
  fi
  if mount | grep -qE "tmpfs on ${TARGET_HOME}/capture type tmpfs"; then
    ok "${TARGET_HOME}/capture is tmpfs-mounted (BRRDfeeder hardening — RAM-backed PCAP)"
  else
    say "${TARGET_HOME}/capture is disk-backed (Standard/Commercial)"
  fi
  if [[ -x "$BLACKBOX_INSTALL" ]]; then ok "black-box flush helper installed"; else warn "black-box flush helper ABSENT"; fi
  if systemctl is-enabled brrdfeeder-blackbox.timer >/dev/null 2>&1; then ok "black-box timer enabled"; else warn "black-box timer not enabled"; fi
  echo "--- BRRDfeeder black box: previous-boot tail ---"
  print_previous_blackbox
  if [[ -x "$RFKILL_BOOT_SOURCE" ]]; then
    "$RFKILL_BOOT_SOURCE" --verify || warn "Bluetooth rfkill boot state is not ready"
  else
    warn "rfkill boot-state helper ABSENT: $RFKILL_BOOT_SOURCE"
  fi
  # READ-ONLY BOUNDARY: verify MUST exit here, before receipts, group changes,
  # writes, service actions or rfkill mutation. Keep new mutations below this.
  exit 0
fi

[[ -x $RFKILL_BOOT_SOURCE ]] || fatal "missing companion helper: $RFKILL_BOOT_SOURCE"
[[ $STORAGE_CLASS != ephemeral || -r $BLACKBOX_SOURCE ]] || fatal "missing companion helper: $BLACKBOX_SOURCE"
MUTATION_PATHS=("$CONFIG_FILE" "$UDEV_RULES_FILE" /etc/fstab
  /etc/systemd/journald.conf.d/99-brrdfeeder.conf
  /etc/systemd/journald.conf.d/99-brrdfeeder-open.conf
  "$BLACKBOX_INSTALL" "$BLACKBOX_SERVICE" "$BLACKBOX_TIMER"
  "$BLACKBOX_LOCK" "$BLACKBOX_DIR/current.log" "$BLACKBOX_DIR/previous-boot.log")
if [[ $SUBSTRATE == quadlet ]]; then
  [[ -r $IDENTITY_SOURCE ]] || fatal "missing companion helper: $IDENTITY_SOURCE"
  MUTATION_PATHS+=("$QUADLET_FILE" "$IDENTITY_INSTALL")
else
  MUTATION_PATHS+=("$SYSTEMD_UNIT_FILE")
fi
[[ -z $AUDIT_LOG ]] || MUTATION_PATHS+=("$AUDIT_LOG")
for state in /var/lib/systemd/rfkill/*:bluetooth /var/lib/systemd/rfkill/bluetooth; do
  [[ ! -e $state ]] || MUTATION_PATHS+=("$state")
done
# Reject unsafe targets before creating even a partial receipt.
for target in "${MUTATION_PATHS[@]}"; do
  [[ $target == /* && $target != *$'\n'* && $target != *$'\t'* ]] || fatal "unsafe receipt path: $target"
  [[ ! -L $target && ( ! -e $target || -f $target ) ]] || fatal "refusing unsafe mutation target: $target"
done
receipt_begin
audit_begin
if ! id -nG "$TARGET_USER" | tr ' ' '\n' | grep -qx dialout; then
  run usermod -a -G dialout "$TARGET_USER"
  warn "dialout membership requires a fresh user manager/session before legacy engine launch"
fi

# ----------------------------------------------------------------------
# Step 1 — Install udev rules (substrate-stable device naming)
# ----------------------------------------------------------------------
gate "Step 1 — udev rules"

NEW_UDEV_CONTENT=$(cat <<EOF
# /etc/udev/rules.d/99-cybrrd-brrdfeeder.rules
#
# cyBRRD BRRDfeeder substrate-stable device naming.
# Installed by brrdfeeder-install.sh on $(date -Iseconds).
#
# Vendor-AGNOSTIC abstraction: downstream config + Quadlet volume mounts
# reference /dev/cybrrd_gps and /dev/cybrrd_ble. Vendor swap = update
# this file's ATTRS{} only; symlinks (and engine, and Quadlet) stay stable.
#
# MODE 0660 + GROUP dialout — UID 1001:20 in the rootful container;
# bare-metal service user must belong to dialout. No world-writable devices.

# u-blox 7 GPS / GNSS receiver
SUBSYSTEM=="tty", ATTRS{idVendor}=="${UBLOX_VENDOR}", ATTRS{idProduct}=="${UBLOX_PRODUCT}", SYMLINK+="${GPS_SYMLINK}", GROUP="dialout", MODE="0660"

# Nordic Semiconductor nRF52 Connectivity (BLE)
SUBSYSTEM=="tty", ATTRS{idVendor}=="${NORDIC_VENDOR}", ATTRS{idProduct}=="${NORDIC_PRODUCT}", SYMLINK+="${BLE_SYMLINK}", GROUP="dialout", MODE="0660"
KERNEL=="rfkill", SUBSYSTEM=="misc", GROUP="dialout", MODE="0660"
EOF
)

if [[ -f "$UDEV_RULES_FILE" ]] && diff -q <(echo "$NEW_UDEV_CONTENT") "$UDEV_RULES_FILE" >/dev/null 2>&1; then
  ok "udev rules already current — no change"
else
  if [[ -f "$UDEV_RULES_FILE" ]]; then
    say "existing udev rules covered by installer receipt"
  fi
  if [[ $DRY_RUN -eq 1 ]]; then
    say "[dry-run] would write:"
    printf '  %s\n' "${NEW_UDEV_CONTENT//$'\n'/$'\n  '}"
  else
    atomic_write "$UDEV_RULES_FILE" "$NEW_UDEV_CONTENT"
    ok "wrote $UDEV_RULES_FILE"
  fi
fi

gate "Step 1b — reload + trigger udev"
run udevadm control --reload-rules
run udevadm trigger --subsystem-match=tty --action=change
run udevadm trigger --subsystem-match=misc --sysname-match=rfkill --action=change
run udevadm settle --timeout=5
ok "udev rules applied"

# ----------------------------------------------------------------------
# Step 1c — make Bluetooth soft-unblocked now and at the next boot
# ----------------------------------------------------------------------
# systemd-rfkill restores the ID_PATH:type state files at device add. Writing
# "0" (unblocked) is narrower than masking restore globally: Wi-Fi and any
# intentionally blocked radio retain their independent saved policy.
gate "Step 1c — Bluetooth rfkill boot state"

[[ -x "$RFKILL_BOOT_SOURCE" ]] || fatal "missing companion helper: $RFKILL_BOOT_SOURCE"
if [[ $DRY_RUN -eq 1 ]]; then
  "$RFKILL_BOOT_SOURCE" --dry-run || warn "Bluetooth rfkill dry-run could not find an attached controller"
else
  set +e
  "$RFKILL_BOOT_SOURCE"
  RFKILL_RC=$?
  set -e
  if [[ $RFKILL_RC -eq 3 ]]; then
    warn "Bluetooth controller absent; boot state was not changed"
  elif [[ $RFKILL_RC -ne 0 ]]; then
    fatal "Bluetooth rfkill boot-state provisioning failed (exit $RFKILL_RC)"
  fi
fi

# ----------------------------------------------------------------------
# Step 1.5 — BRRDfeeder hardware protection (MicroSD wear mitigation)
# ----------------------------------------------------------------------
#
# Substrate-truth (probed 2026-06-07 on cardinal):
#  * engine produces ~47 MB/day of stdout to its log file (compile-time
#    path `capture/validation.pcap` is hardcoded relative to WorkingDir)
#  * cardinal-class deployments run on eMMC/NVMe and tolerate this
#  * BRRDfeeder runs on MicroSD which would die in 30-90 days
#
# Four-layer mitigation activates when config has node.storage_class:
# "ephemeral" (BRRDfeeder). Standard/Commercial (storage_class: "persistent"
# or unset) skips this step entirely.
#
#   Layer 1 — journald drop-in: Storage=volatile, RuntimeMaxUse=200M
#             (system + user logs land in /run/log/journal tmpfs)
#   Layer 2 — tmpfs MOUNT AT /home/synth/capture/ so the engine's
#             hardcoded relative capture/validation.pcap path lands
#             in RAM. 50M cap; the kernel ring-buffers the tmpfs as
#             needed.
#   Layer 3 — systemd unit StandardOutput=journal (set in Step 4)
#             routes the engine's stdout through journald's volatile
#             store. NO append-to-file on SD.
#   Layer 4 — a 60 s bounded black-box timer persists only changed last-tail
#             content as current + previous-boot (<=1 MiB, one syncfs).

gate "Step 1.5 — BRRDfeeder hardware protection (storage_class)"

say "config-declared node.storage_class=$STORAGE_CLASS"

if [[ "$STORAGE_CLASS" == "ephemeral" ]]; then
  # Layer 1 — journald drop-in (99-brrdfeeder.conf since the 2026-09-25
  # naming change; the legacy 99-brrdfeeder-open.conf is recognized by its
  # own marker and migrated away on re-run, never blindly removed).
  JOURNALD_DROPIN_DIR="/etc/systemd/journald.conf.d"
  JOURNALD_DROPIN="${JOURNALD_DROPIN_DIR}/99-brrdfeeder.conf"
  JOURNALD_DROPIN_LEGACY="${JOURNALD_DROPIN_DIR}/99-brrdfeeder-open.conf"
  JOURNALD_DROPIN_MARKER="# BRRDfeeder — protect MicroSD card from journald write wear."
  JOURNALD_DROPIN_LEGACY_MARKER="BRRDfeeder Open tier"
  run install -d -m 0755 "$JOURNALD_DROPIN_DIR"

  NEW_JOURNALD=$(cat <<'JEOF'
# BRRDfeeder — protect MicroSD card from journald write wear.
# Installed by brrdfeeder-install.sh when node.storage_class is "ephemeral".
# Forces journald to RAM (/run/log/journal). System logs are NOT persistent.
[Journal]
Storage=volatile
RuntimeMaxUse=200M
SystemMaxUse=0
JEOF
)
  # Marker-gated recognition: a file at either name carrying neither marker is
  # not ours and must stop the run rather than be overwritten or removed.
  if [[ -f "$JOURNALD_DROPIN_LEGACY" ]] && ! grep -qF -- "$JOURNALD_DROPIN_LEGACY_MARKER" "$JOURNALD_DROPIN_LEGACY"; then
    fatal "unrecognised legacy journald drop-in: $JOURNALD_DROPIN_LEGACY carries no BRRDfeeder marker; review it and remove it yourself"
  fi
  if [[ -f "$JOURNALD_DROPIN" ]] && ! grep -qF -- "$JOURNALD_DROPIN_MARKER" "$JOURNALD_DROPIN" && ! diff -q <(echo "$NEW_JOURNALD") "$JOURNALD_DROPIN" >/dev/null 2>&1; then
    fatal "unrecognised journald drop-in: $JOURNALD_DROPIN carries no BRRDfeeder marker; review it and remove it yourself"
  fi
  JOURNALD_CHANGED=0
  if [[ -f "$JOURNALD_DROPIN" ]] && diff -q <(echo "$NEW_JOURNALD") "$JOURNALD_DROPIN" >/dev/null 2>&1; then
    ok "journald drop-in already current — no change"
  else
    if [[ $DRY_RUN -eq 1 ]]; then
      say "[dry-run] would write $JOURNALD_DROPIN"
    else
      atomic_write "$JOURNALD_DROPIN" "$NEW_JOURNALD"
      JOURNALD_CHANGED=1
      ok "journald Storage=volatile applied (logs in /run/log/journal tmpfs)"
    fi
  fi
  if [[ -f "$JOURNALD_DROPIN_LEGACY" ]]; then
    if [[ $DRY_RUN -eq 1 ]]; then
      say "[dry-run] would migrate legacy drop-in: remove $JOURNALD_DROPIN_LEGACY (renamed to $JOURNALD_DROPIN)"
    else
      rm -f -- "$JOURNALD_DROPIN_LEGACY"
      JOURNALD_CHANGED=1
      ok "legacy journald drop-in migrated away: $JOURNALD_DROPIN_LEGACY → $JOURNALD_DROPIN"
    fi
  fi

  # Reload only after migration, so an old override cannot remain loaded.
  if [[ $JOURNALD_CHANGED -eq 1 ]]; then systemctl restart systemd-journald; fi

  # Layer 2 — tmpfs at /home/synth/capture
  CAPTURE_DIR="${TARGET_HOME}/capture"
  FSTAB_LINE="tmpfs ${CAPTURE_DIR} tmpfs size=50M,uid=${TARGET_UID},gid=${TARGET_GID},mode=0755 0 0"

  if [[ ! -d "$CAPTURE_DIR" ]]; then
    run install -d -o "$TARGET_USER" -g "$(id -gn $TARGET_USER)" -m 0755 "$CAPTURE_DIR"
  fi

  if grep -qE "^tmpfs[[:space:]]+${CAPTURE_DIR}[[:space:]]" /etc/fstab 2>/dev/null; then
    ok "fstab tmpfs entry for $CAPTURE_DIR already present"
  else
    if [[ $DRY_RUN -eq 1 ]]; then
      say "[dry-run] would append to /etc/fstab: $FSTAB_LINE"
    else
      echo "$FSTAB_LINE" >> /etc/fstab
      ok "fstab tmpfs entry appended"
    fi
  fi

  if mount | grep -qE "tmpfs on ${CAPTURE_DIR} type tmpfs"; then
    ok "$CAPTURE_DIR already tmpfs-mounted"
  else
    if [[ $DRY_RUN -eq 0 ]]; then
      # Use systemd's mount unit via mount -a (reads /etc/fstab) or
      # directly. systemd will respect this for next-boot too.
      mount "$CAPTURE_DIR" 2>/dev/null || mount -a
      ok "$CAPTURE_DIR mounted as tmpfs (50M RAM cap, owner=$TARGET_USER)"
    fi
  fi

  # Layer 4 — bounded persistent black box. Journald stays volatile; once per
  # minute this saves only changed content, with two files and one syncfs.
  [[ -r "$BLACKBOX_SOURCE" ]] || fatal "BRRDfeeder tier requires companion helper: $BLACKBOX_SOURCE"
  if [[ -x "$BLACKBOX_INSTALL" ]] && cmp -s "$BLACKBOX_SOURCE" "$BLACKBOX_INSTALL"; then
    ok "black-box flush helper already current"
  else
    run install -D -o root -g root -m 0755 "$BLACKBOX_SOURCE" "$BLACKBOX_INSTALL"
    say "installed bounded black-box flush helper"
  fi

  NEW_BLACKBOX_SERVICE=$(cat <<EOF
[Unit]
Description=BRRDfeeder bounded persistent black-box flush
After=systemd-journald.service

[Service]
Type=oneshot
ExecStart=${BLACKBOX_INSTALL} --output-dir ${BLACKBOX_DIR}
UMask=0077
Nice=10
IOSchedulingClass=idle
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
ReadWritePaths=${BLACKBOX_DIR}
IPAddressDeny=any
EOF
)
  NEW_BLACKBOX_TIMER=$(cat <<'EOF'
[Unit]
Description=Flush BRRDfeeder black box every 60 seconds

[Timer]
OnBootSec=60s
OnUnitActiveSec=60s
AccuracySec=1s
RandomizedDelaySec=0
Persistent=false
Unit=brrdfeeder-blackbox.service

[Install]
WantedBy=timers.target
EOF
)
  BLACKBOX_UNITS_CHANGED=0
  for target in "$BLACKBOX_SERVICE" "$BLACKBOX_TIMER"; do
    if [[ $target == "$BLACKBOX_SERVICE" ]]; then content=$NEW_BLACKBOX_SERVICE; else content=$NEW_BLACKBOX_TIMER; fi
    if [[ -f "$target" ]] && diff -q <(printf '%s\n' "$content") "$target" >/dev/null 2>&1; then
      ok "$(basename "$target") already current"
    elif [[ $DRY_RUN -eq 1 ]]; then
      say "[dry-run] would write $target"
      BLACKBOX_UNITS_CHANGED=1
    else
      atomic_write "$target" "$content"
      BLACKBOX_UNITS_CHANGED=1
      ok "wrote $target"
    fi
  done
  run install -d -o root -g root -m 0700 "$BLACKBOX_DIR"
  if [[ -L "$BLACKBOX_LOCK" || ( -e "$BLACKBOX_LOCK" && ! -f "$BLACKBOX_LOCK" ) ]]; then
    fatal "refusing unsafe black-box lock path: $BLACKBOX_LOCK"
  elif [[ -f "$BLACKBOX_LOCK" ]]; then
    run chown root:root "$BLACKBOX_LOCK"
    run chmod 0600 "$BLACKBOX_LOCK"
  else
    run install -o root -g root -m 0600 /dev/null "$BLACKBOX_LOCK"
  fi
  if [[ $BLACKBOX_UNITS_CHANGED -eq 1 ]]; then run systemctl daemon-reload; fi
  run systemctl enable --now brrdfeeder-blackbox.timer
  run systemctl start brrdfeeder-blackbox.service

  say "BRRDfeeder protection ACTIVE: volatile bulk logs + <=1 MiB two-file persistent black box"
else
  if systemctl is-enabled brrdfeeder-blackbox.timer >/dev/null 2>&1; then
    run systemctl disable --now brrdfeeder-blackbox.timer
    say "disabled BRRDfeeder black-box timer for persistent storage class"
  fi
  ok "storage_class=$STORAGE_CLASS — skipping ephemeral hardening (Standard/Commercial keeps disk-backed writes)"
fi

# Verify substrate-truth of the symlinks
if [[ $DRY_RUN -eq 0 ]]; then
  if [[ $HAVE_UBLOX -eq 1 ]]; then
    if [[ -L "/dev/$GPS_SYMLINK" ]]; then
      ok "/dev/$GPS_SYMLINK -> $(readlink -f /dev/$GPS_SYMLINK)"
    else
      warn "/dev/$GPS_SYMLINK was NOT created. Re-plug the u-blox or check rule syntax."
    fi
  fi
  if [[ $HAVE_NORDIC -eq 1 ]]; then
    if [[ -L "/dev/$BLE_SYMLINK" ]]; then
      ok "/dev/$BLE_SYMLINK -> $(readlink -f /dev/$BLE_SYMLINK)"
    else
      warn "/dev/$BLE_SYMLINK was NOT created. Re-plug the Nordic or check rule syntax."
    fi
  fi
fi

# ----------------------------------------------------------------------
# Step 2 — Update config.yaml to use the stable symlink
# ----------------------------------------------------------------------
gate "Step 2 — config.yaml GPS path"

if [[ -f "$CONFIG_FILE" ]]; then
  CURRENT_GPS=$(grep -E '^\s*device:' "$CONFIG_FILE" | head -1 | awk -F'"' '{print $2}')
  TARGET_PATH="/dev/${GPS_SYMLINK}"
  if [[ "$CURRENT_GPS" == "$TARGET_PATH" ]]; then
    ok "config.yaml already pointed at $TARGET_PATH"
  else
    say "config.yaml currently: $CURRENT_GPS  ->  switching to $TARGET_PATH"
    if [[ $DRY_RUN -eq 0 ]]; then
      sed -i "s|^\(\s*device:\s*\)\".*\"|\1\"${TARGET_PATH}\"|" "$CONFIG_FILE"
      if [[ $SUBSTRATE == legacy ]]; then chown "$TARGET_USER:$(id -gn "$TARGET_USER")" "$CONFIG_FILE"; fi
      ok "config.yaml updated"
    fi
  fi
else
  warn "config.yaml not found at $CONFIG_FILE — skipping device-path update"
fi

# Container nodes never enter the legacy user-service/linger/process handoff.
if [[ $SUBSTRATE == quadlet ]]; then
  gate 'Step 3 — rootful Quadlet refresh (image preserved)'
  if [[ ! -x $IDENTITY_INSTALL ]] || ! cmp -s "$IDENTITY_SOURCE" "$IDENTITY_INSTALL"; then
    run install -D -o root -g root -m 0755 "$IDENTITY_SOURCE" "$IDENTITY_INSTALL"
  fi
  if cmp -s "$QUADLET_FILE" <(printf '%s\n' "$NEW_QUADLET"); then
    ok 'Quadlet already current — no change'
  elif [[ $DRY_RUN -eq 1 ]]; then
    say "[dry-run] would install rendered non-root Quadlet: $QUADLET_FILE"
    run systemctl daemon-reload
  else
    atomic_write "$QUADLET_FILE" "$NEW_QUADLET"
    ok "installed $QUADLET_FILE"
    run systemctl daemon-reload
  fi
  if [[ $RESTART_ENGINE -eq 1 ]]; then
    run systemctl restart brrdfeeder-engine.service
    if [[ $DRY_RUN -eq 0 ]]; then systemctl is-active --quiet brrdfeeder-engine.service; fi
    say 'operator health gate still required: nats] connected + heartbeat within 180 s'
    exit 0
  fi
  say 'restart required: engine was not restarted; use --restart-engine or an operator health-gated restart'
  [[ $DRY_RUN -eq 1 ]] && exit 0
  exit 3
fi

# ----------------------------------------------------------------------
# Step 3 — Enable user-linger (so systemd --user persists across logout)
# ----------------------------------------------------------------------
gate "Step 3 — user-linger"

if loginctl show-user "$TARGET_USER" 2>/dev/null | grep -q "Linger=yes"; then
  ok "user-linger already enabled for $TARGET_USER"
else
  run loginctl enable-linger "$TARGET_USER"
  ok "user-linger enabled for $TARGET_USER (systemd-user units will now survive reboot + logout)"
fi

# ----------------------------------------------------------------------
# Step 4 — Install systemd --user unit for brrdfeeder-engine
# ----------------------------------------------------------------------
gate "Step 4 — systemd --user unit (brrdfeeder-engine.service)"

UNIT_DIR="$(dirname "$SYSTEMD_UNIT_FILE")"
[[ -d "$UNIT_DIR" ]] || run install -d -o "$TARGET_USER" -g "$(id -gn $TARGET_USER)" -m 0755 "$UNIT_DIR"

NEW_UNIT_CONTENT=$(cat <<'EOF'
[Unit]
Description=cyBRRD BRRDfeeder edge engine (Sentinel + Hunter + Green Protocol publisher)
Documentation=https://github.com/cybrrd/brrdfeeder
After=network-online.target
Wants=network-online.target

[Service]
Type=simple

# Working directory matches the canonical engine_restart pattern in
# infra_state.json (`cd ~ && setsid nohup ./brrdfeeder-src/...`)
WorkingDirectory=%h

# Green Protocol Phase 1 — 1 Hz AirspaceState publisher on
# cybrrd.green.airspace.<node_id>. Default off in the binary; lit here.
Environment="BRRDFEEDER_ENABLE_AIRSPACE_PUBLISHER=true"

# Engine reads config.yaml from $HOME/config.yaml by default; no override
# needed. NATS creds are referenced by config.yaml.

ExecStart=%h/brrdfeeder-src/engine/target/release/engine

# Restart discipline per ADR 0010 + [[sigterm-survivor-lesson]]:
# bounded restart cadence, bounded shutdown wait, SIGKILL after timeout.
Restart=always
RestartSec=5
TimeoutStopSec=10
KillMode=mixed
KillSignal=SIGTERM

# Forensic clarity: engine output goes through journald.
# Why not append:%h/engine.log? Substrate-truth probed 2026-06-07:
# the engine produces ~47 MB/day at INFO level. On BRRDfeeder-tier MicroSD
# that destroys the card. On Standard eMMC/NVMe it's tolerable but
# journald-captured logs are still more substrate-truthful (queryable
# via `journalctl --user -u brrdfeeder-engine.service`, ring-buffered,
# automatic rotation). Step 1.5 forces journald to RAM on BRRDfeeder via the
# 99-brrdfeeder.conf drop-in (legacy 99-brrdfeeder-open.conf is migrated).
StandardOutput=journal
StandardError=journal
LimitCORE=0
SyslogIdentifier=brrdfeeder-engine

# Capability inheritance: engine binary has cap_net_admin,cap_net_raw,cap_sys_time=ep
# via setcap. Those file capabilities are inherited automatically on
# exec(). Do NOT add AmbientCapabilities here — user-mode systemd cannot
# grant ambient caps (it runs unprivileged), and the unit will fail with
# exit code 218/CAPABILITIES. File caps are the substrate-truthful path.

[Install]
WantedBy=default.target
EOF
)

LEGACY_UNIT_CHANGED=0
if [[ -f "$SYSTEMD_UNIT_FILE" ]] && diff -q <(echo "$NEW_UNIT_CONTENT") "$SYSTEMD_UNIT_FILE" >/dev/null 2>&1; then
  ok "systemd unit already current — no change"
else
  LEGACY_UNIT_CHANGED=1
  if [[ -f "$SYSTEMD_UNIT_FILE" ]]; then
    say "existing legacy unit covered by installer receipt"
  fi
  if [[ $DRY_RUN -eq 1 ]]; then
    say "[dry-run] would write unit to $SYSTEMD_UNIT_FILE"
  else
    atomic_write "$SYSTEMD_UNIT_FILE" "$NEW_UNIT_CONTENT" "$TARGET_USER" "$(id -gn "$TARGET_USER")"
    ok "wrote $SYSTEMD_UNIT_FILE"
  fi
fi

# ----------------------------------------------------------------------
# Step 5 — Stop existing bare-process engine, then enable + start the unit
# ----------------------------------------------------------------------
gate "Step 5 — handoff bare engine -> systemd-managed engine"

# If a bare engine process is running (started manually pre-install),
# stop it cleanly so the systemd unit can take ownership of the
# capture interface + GPS serial port.
BARE_PID=$(pgrep -f "brrdfeeder-src/engine/target/release/engine" || true)
if [[ -n "$BARE_PID" ]]; then
  say "found bare engine PID $BARE_PID — stopping for handoff"
  if [[ $DRY_RUN -eq 0 ]]; then
    kill -TERM "$BARE_PID" || true
    # Per [[sigterm-survivor-lesson]] — verify on the shared resource, not just ps -p
    for ((i=0; i<10; i++)); do
      if kill -0 "$BARE_PID" 2>/dev/null; then sleep 1; else break; fi
    done
    if kill -0 "$BARE_PID" 2>/dev/null; then
      warn "engine PID $BARE_PID did not exit on SIGTERM after 10s — sending SIGKILL"
      kill -KILL "$BARE_PID" || true
      sleep 1
    fi
    ok "bare engine stopped"
  fi
else
  ok "no bare engine running"
fi

# Reload + enable + start as the target user via systemctl --user
SYSTEMCTL_USER="sudo -u $TARGET_USER XDG_RUNTIME_DIR=/run/user/$TARGET_UID DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$TARGET_UID/bus systemctl --user"

if [[ $LEGACY_UNIT_CHANGED -eq 1 ]]; then run bash -c "$SYSTEMCTL_USER daemon-reload"; fi
run bash -c "$SYSTEMCTL_USER enable brrdfeeder-engine.service"
run bash -c "$SYSTEMCTL_USER restart brrdfeeder-engine.service"
ok "brrdfeeder-engine.service enabled + started"

# ----------------------------------------------------------------------
# Step 6 — Verify
# ----------------------------------------------------------------------
gate "Step 6 — verification"

if [[ $DRY_RUN -eq 1 ]]; then
  say "[dry-run] skipping live verification"
  exit 0
fi

sleep 5

# is-active
if eval "$SYSTEMCTL_USER is-active brrdfeeder-engine.service" >/dev/null; then
  ok "brrdfeeder-engine.service is ACTIVE"
else
  warn "brrdfeeder-engine.service is NOT active. journal: "
  eval "$SYSTEMCTL_USER status brrdfeeder-engine.service --no-pager -l" || true
  exit 1
fi

# Process verification — exclude OUR pgrep itself from the match
NEW_PID=$(pgrep -f "brrdfeeder-src/engine/target/release/engine" | xargs -I{} sh -c 'cat /proc/{}/comm 2>/dev/null | grep -q "^engine$" && echo {}' | head -1 || true)
if [[ -n "$NEW_PID" ]]; then
  ok "engine PID $NEW_PID running under systemd"
else
  warn "no engine PID found — service may be in restart backoff. Inspect: systemctl --user status brrdfeeder-engine.service"
fi

# Quick log probe for GPS + Green Protocol startup. Logs now flow through
# journald (per Step 4's StandardOutput=journal). Query the systemd-user
# journal scoped to the unit so we don't pick up unrelated noise.
sleep 3
RECENT_LOG=$(sudo -u "$TARGET_USER" \
  XDG_RUNTIME_DIR="/run/user/$TARGET_UID" \
  DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$TARGET_UID/bus" \
  journalctl --user -u brrdfeeder-engine.service --since=now-2min --no-pager 2>/dev/null || echo "")

if echo "$RECENT_LOG" | grep -q "GPS reached Healthy"; then
  ok "GPS reached Healthy (journald)"
else
  warn "GPS Healthy marker not yet in journal — engine may still be waiting for fix (up to 120s) or in restart loop"
fi
if echo "$RECENT_LOG" | grep -q "green-tick.*airspace publisher ENABLED"; then
  ok "Green Protocol publisher enabled (journald)"
fi
say "engine logs available via:  journalctl --user -u brrdfeeder-engine.service -f"

# Reboot survival hint
gate "Automatic startup"
say "user-linger is ON, unit is enabled for default.target. The engine will"
say "auto-start on reboot via systemd --user with Restart=always."
say "To inspect at any time: ssh cm4-saker 'systemctl --user status brrdfeeder-engine.service'"
say "To re-run this script safely (idempotent): sudo bash $0"
say "To verify state without changes:           sudo bash $0 --verify"
say
say "Device naming and automatic service startup are configured."
