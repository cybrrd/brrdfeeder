#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Destructive fixture setup is confined to a fresh disposable container.
set -euo pipefail
[[ ${D12_INSTALLER_TEST_CONTAINER:-0} == 1 && $EUID -eq 0 && -f /run/.containerenv ]] || {
  echo 'STOP: use a throwaway root Podman container with --network=none' >&2; exit 2;
}
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
bootstrap="$script_dir/../deploy/bootstrap"
tmp=$(mktemp -d)
kit="$tmp/brrdfeeder-bootstrap"
mkdir -p "$kit" "$tmp/bin" "$tmp/rfkill/rfkill0"
cp "$bootstrap/"*.sh "$kit/"
cp "$script_dir/../deploy/quadlet/brrdfeeder-engine.container" "$kit/"
installer="$kit/brrdfeeder-install.sh"
export SYSTEMCTL_LOG="$tmp/systemctl.log"
export MV_LOG="$tmp/mv.log"
export PATH="$tmp/bin:$PATH"
cat > "$tmp/bin/systemctl" <<'EOF'
#!/bin/bash
echo "$*" >> "$SYSTEMCTL_LOG"
case "$1" in
  is-enabled) exit 1 ;;
  is-active) [[ ${*: -1} == brrdfeeder-engine.service && ${MOCK_ENGINE_ACTIVE:-0} == 1 ]] ;;
  restart) [[ ${FAIL_RELOAD:-0} != 1 ]] ;;
  daemon-reload) [[ ${FAIL_RELOAD:-0} != 1 ]] ;;
  *) exit 0 ;;
esac
EOF
cat > "$tmp/bin/mv" <<'EOF'
#!/bin/bash
source_path=${@: -2:1}
target_path=${*: -1}
if [[ $source_path == *.installer.* ]]; then
  [[ ${source_path%/*} == "${target_path%/*}" ]] || exit 90
  printf '%s -> %s\n' "$source_path" "$target_path" >> "$MV_LOG"
  if [[ $target_path == "${FAIL_ATOMIC_TARGET:-}" ]]; then
    printf 'truncated staged write\n' > "$source_path"
    exit 77
  fi
fi
exec /bin/mv "$@"
EOF
for command in udevadm mount loginctl lsusb sudo; do
  printf '#!/bin/bash\nexit 0\n' > "$tmp/bin/$command"
done
printf '#!/bin/bash\nexit 1\n' > "$tmp/bin/pgrep"
chmod +x "$tmp/bin/"*
# Exercise the real helper with simulated sysfs but the real fixture state dir.
mv "$kit/brrdfeeder-rfkill-boot-state.sh" "$kit/rfkill-real.sh"
printf 'bluetooth\n' > "$tmp/rfkill/rfkill0/type"
printf '0\n' > "$tmp/rfkill/rfkill0/soft"
cat > "$kit/brrdfeeder-rfkill-boot-state.sh" <<EOF
#!/bin/bash
exec env RFKILL_SETTLE_SECONDS=0 bash "$kit/rfkill-real.sh" "\$@" --sysfs-root "$tmp/rfkill" --rfkill-command /bin/true
EOF
chmod +x "$kit/"*.sh
useradd -m operator
export BRRDFEEDER_LEGACY_USER=operator
mkdir -p /etc/brrdfeeder /home/operator/brrdfeeder /etc/containers/systemd /etc/udev/rules.d /var/lib/systemd/rfkill /etc/systemd/journald.conf.d
quadlet=/etc/containers/systemd/brrdfeeder-engine.container
# shellcheck disable=SC2016 # Deliberate literal dollar and trailing spaces.
printf '[Container]\nImage=localhost/fake:retain-$literal  \nUser=0\n' > "$quadlet"
printf '1\n' > /var/lib/systemd/rfkill/platform-fixture:bluetooth
printf 'old udev\n' > /etc/udev/rules.d/99-cybrrd-brrdfeeder.rules
# A legacy-node drop-in: the pre-rename installer wrote exactly this content.
printf '# BRRDfeeder Open tier — protect MicroSD card from journald write wear.\n# Installed by brrdfeeder-install.sh when node.storage_class is "ephemeral".\n# Forces journald to RAM (/run/log/journal). System logs are NOT persistent.\n[Journal]\nStorage=volatile\nRuntimeMaxUse=200M\nSystemMaxUse=0\n' > /etc/systemd/journald.conf.d/99-brrdfeeder-open.conf
configs=(/etc/brrdfeeder/config.yaml /home/operator/brrdfeeder/config.yaml /home/operator/config.yaml)
check_rc() {
  local want=$1; shift
  set +e
  "$@" > "$tmp/output" 2>&1
  local got=$?
  set -e
  if [[ $got != "$want" ]]; then cat "$tmp/output"; echo "FAIL exit=$got expected=$want"; exit 1; fi
}
snapshot() {
  # Include every persistent namespace the installer touches, including account
  # files and receipt creation. /tmp holds only test evidence and fake commands.
  find /etc /home/operator /usr/local/libexec /var/lib/brrdfeeder /var/lib/brrdfeeder-deploy /var/lib/systemd /var/lib/AccountsService \
    -type f -exec sha256sum {} + 2>/dev/null | sort || true
}
check_rc 2 bash "$installer" --dry-run
for config in "${configs[@]}"; do grep -Fq "$config" "$tmp/output"; done
echo 'PASS missing configs: exit=2, all candidates named'
for ((i=2; i>=0; i--)); do
  cp "$bootstrap/config.yaml.mobile.template" "${configs[$i]}"
  check_rc 0 bash "$installer" --dry-run
  grep -Fq "selected config: ${configs[$i]} (first existing" "$tmp/output"
done
check_rc 0 bash "$installer" --config /home/operator/config.yaml --dry-run
grep -Fq 'selected config: /home/operator/config.yaml (explicit --config)' "$tmp/output"
grep -Fq 'Volume=/home/operator/config.yaml:/etc/brrdfeeder/config.yaml:ro,Z' "$tmp/output"
check_rc 2 bash "$installer" --config /nonexistent --dry-run
check_rc 2 bash "$installer" --config
echo 'PASS config priority: etc > home/brrdfeeder > home; explicit selection and bind source; invalid explicit rejected'

for mode in 0775 0757 0777; do
  chmod "$mode" /etc/brrdfeeder
  snapshot > "$tmp/unsafe.before"
  for flag in --verify --dry-run ''; do
    if [[ -n $flag ]]; then check_rc 2 bash "$installer" "$flag"; else check_rc 2 bash "$installer"; fi
    grep -Fq 'config parent is group/world-writable: /etc/brrdfeeder' "$tmp/output"
  done
  snapshot > "$tmp/unsafe.after"
  cmp "$tmp/unsafe.before" "$tmp/unsafe.after"
done
for mode in 0700 0755; do
  chmod "$mode" /etc/brrdfeeder
  check_rc 0 bash "$installer" --verify
done
echo 'PASS config-parent guard: group/world write rejected in verify/dry/apply with no mutation; 0700/0755 accepted'

snapshot > "$tmp/before"
getent group dialout > "$tmp/group.before"
check_rc 0 bash "$installer" --verify --audit-log="$tmp/must-not-create/audit.log"
snapshot > "$tmp/after"
getent group dialout > "$tmp/group.after"
cmp "$tmp/before" "$tmp/after"
cmp "$tmp/group.before" "$tmp/group.after"
[[ ! -e $tmp/must-not-create && ! -d /var/lib/brrdfeeder-deploy ]]
check_rc 0 bash "$installer" --dry-run --audit-log="$tmp/must-not-create/audit.log"
grep -Fq 'would create receipt:' "$tmp/output"
grep -Fq 'would install rendered non-root Quadlet' "$tmp/output"
grep -Fq 'MODE="0660"' "$tmp/output"
grep -Fq 'brrdfeeder-blackbox.timer' "$tmp/output"
grep -Fq 'Bluetooth rfkill boot state' "$tmp/output"
snapshot > "$tmp/after"
cmp "$tmp/before" "$tmp/after"
[[ ! -e $tmp/must-not-create ]]
echo 'PASS verify and dry-run: all touched namespaces and dialout unchanged; no audit or receipt written'

sed -i 's/"ephemeral"/"ephemreal"/' /etc/brrdfeeder/config.yaml
check_rc 1 bash "$installer" --dry-run
grep -Fq "got 'ephemreal'" "$tmp/output"
[[ ! -d /var/lib/brrdfeeder-deploy ]]
cp "$bootstrap/config.yaml.mobile.template" /etc/brrdfeeder/config.yaml
echo 'PASS storage typo aborts by value before mutation'
sed -i $'s/"ephemeral"/"ephem\033real"/' /etc/brrdfeeder/config.yaml
check_rc 1 bash "$installer" --dry-run
grep -Fq "got 'ephemreal'" "$tmp/output"
if LC_ALL=C grep -q $'\033' "$tmp/output"; then echo 'FAIL storage-class control byte leaked'; exit 1; fi
cp "$bootstrap/config.yaml.mobile.template" /etc/brrdfeeder/config.yaml
echo 'PASS storage-class fatal names sanitized value without ESC'
cp "$quadlet" "$tmp/valid-quadlet"
printf 'Image=localhost/second:ambiguous\n' >> "$quadlet"
check_rc 1 bash "$installer" --dry-run
grep -Fq 'exactly one Image=' "$tmp/output"
cp "$tmp/valid-quadlet" "$quadlet"
[[ ! -d /var/lib/brrdfeeder-deploy ]]
echo 'PASS ambiguous Image rejected before mutation; Quadlet mode needs no bare-metal binary'

# Group membership is intentionally manual on rollback; prove its receipt and
# then restore the test account's membership using the documented inverse.
check_rc 3 bash "$installer" --audit-log=/home/operator/installer-audit.log
receipt=$(sed -n 's/^\[brrdfeeder-install\] receipt: \([^ ]*\).*/\1/p' "$tmp/output" | head -1)
[[ -x $receipt/ROLLBACK.sh && -f $receipt/MANIFEST ]]
audit_hash=$(sha256sum /home/operator/installer-audit.log | cut -d ' ' -f 1)
grep -Fq "$audit_hash" "$receipt/MANIFEST"
[[ $(stat -c %a "$receipt") == 700 && $(stat -c %a "$receipt/ROLLBACK.sh") == 700 ]]
grep -Fq 'manual-membership' "$receipt/MANIFEST"
id -nG operator | tr ' ' '\n' | grep -qx dialout
bash "$receipt/ROLLBACK.sh" > "$tmp/rollback"
grep -Fq 'gpasswd -d operator dialout' "$tmp/rollback"
gpasswd -d operator dialout >/dev/null
echo 'PASS group addition recorded; private receipt; final audit hash matches; rollback prints narrow manual inverse'

# Exact restore proof covers replacement, creation, rfkill, and partial apply.
usermod -a -G dialout operator
file_snapshot() {
  find /etc /home/operator /usr/local/libexec /var/lib/brrdfeeder /var/lib/systemd \
    -type f -exec sha256sum {} + 2>/dev/null | sort || true
}
file_snapshot > "$tmp/install.before"
: > "$SYSTEMCTL_LOG"
check_rc 3 bash "$installer"
receipt=$(sed -n 's/^\[brrdfeeder-install\] receipt: \([^ ]*\).*/\1/p' "$tmp/output" | head -1)
grep '^Image=' "$quadlet" > "$tmp/image"
# shellcheck disable=SC2016
printf 'Image=localhost/fake:retain-$literal  \n' > "$tmp/expected-image"
cmp "$tmp/image" "$tmp/expected-image"
# shellcheck disable=SC2016
sed 's/^Image=.*/Image=localhost\/fake:retain-$literal  /' "$kit/brrdfeeder-engine.container" > "$tmp/expected-quadlet"
cmp "$quadlet" "$tmp/expected-quadlet"
[[ ! -e /home/operator/.config/systemd/user/brrdfeeder-engine.service ]]
if grep -Fq 'restart brrdfeeder-engine.service' "$SYSTEMCTL_LOG"; then echo 'FAIL unintended restart'; exit 1; fi
[[ $(< /var/lib/systemd/rfkill/platform-fixture:bluetooth) == 0 ]]
grep -Fq $'/etc/containers/systemd/brrdfeeder-engine.container\treplace\t' "$receipt/MANIFEST"
echo 'PASS apply exit=3: exact image line retained; remainder equals template; no legacy service or engine restart'
# Re-applying unchanged content must not replace the Quadlet inode/mtime.
quadlet_stamp=$(stat -c '%i:%y' "$quadlet")
: > "$SYSTEMCTL_LOG"
check_rc 3 bash "$installer"
[[ $(stat -c '%i:%y' "$quadlet") == "$quadlet_stamp" ]]
grep -Fq 'Quadlet already current' "$tmp/output"
if grep -Fq 'daemon-reload' "$SYSTEMCTL_LOG"; then echo 'FAIL unchanged units reloaded systemd'; exit 1; fi
echo 'PASS unchanged Quadlet is not reinstalled; unchanged units trigger no daemon-reload'
bash "$receipt/ROLLBACK.sh" > "$tmp/rollback"
file_snapshot > "$tmp/install.after"
diff -u "$tmp/install.before" "$tmp/install.after"
echo 'PASS rollback: file hashes match pre-install, including removed new files and restored rfkill state'

check_rc 1 env FAIL_RELOAD=1 bash "$installer"
receipt=$(sed -n 's/^\[brrdfeeder-install\] receipt: \([^ ]*\).*/\1/p' "$tmp/output" | head -1)
[[ -s $receipt/MANIFEST ]]
bash "$receipt/ROLLBACK.sh" > "$tmp/rollback"
file_snapshot > "$tmp/install.after"
diff -u "$tmp/install.before" "$tmp/install.after"
echo 'PASS partial failure preserves receipt and rollback restores files'

check_rc 0 bash "$installer" --dry-run --restart-engine
grep -Fq 'systemctl restart brrdfeeder-engine.service' "$tmp/output"
# The fake supervisor reports active for this explicit-restart proof only.
: > "$SYSTEMCTL_LOG"
check_rc 0 env MOCK_ENGINE_ACTIVE=1 bash "$installer" --restart-engine
grep -Fq 'restart brrdfeeder-engine.service' "$SYSTEMCTL_LOG"
receipt=$(sed -n 's/^\[brrdfeeder-install\] receipt: \([^ ]*\).*/\1/p' "$tmp/output" | head -1)
bash "$receipt/ROLLBACK.sh" > "$tmp/rollback"
echo 'PASS explicit restart flag alone permits engine restart (supervisor simulated)'

sed -i 's|/dev/cybrrd_gps|/dev/ttyACM9|' /home/operator/config.yaml
etc_hash=$(sha256sum /etc/brrdfeeder/config.yaml)
selected_hash=$(sha256sum /home/operator/config.yaml)
check_rc 3 bash "$installer" --config /home/operator/config.yaml
grep -Fq 'device: "/dev/cybrrd_gps"' /home/operator/config.yaml
[[ $(sha256sum /etc/brrdfeeder/config.yaml) == "$etc_hash" ]]
grep -Fq 'Volume=/home/operator/config.yaml:/etc/brrdfeeder/config.yaml:ro,Z' "$quadlet"
receipt=$(sed -n 's/^\[brrdfeeder-install\] receipt: \([^ ]*\).*/\1/p' "$tmp/output" | head -1)
bash "$receipt/ROLLBACK.sh" > "$tmp/rollback"
[[ $(sha256sum /home/operator/config.yaml) == "$selected_hash" ]]
echo 'PASS Step 2 edits only selected config; alternate bind rendered and rollback restores selected file'

# A short/failed staging write must leave each existing destination byte-intact.
# Fault injection lives in the test's mv shim, never in production installer.
for target in /etc/udev/rules.d/99-cybrrd-brrdfeeder.rules \
  /etc/systemd/journald.conf.d/99-brrdfeeder.conf \
  /etc/systemd/system/brrdfeeder-blackbox.service \
  /etc/systemd/system/brrdfeeder-blackbox.timer "$quadlet"; do
  mkdir -p "$(dirname "$target")"
  if [[ $target == "$quadlet" ]]; then
    cp "$tmp/valid-quadlet" "$target"
  elif [[ $target == /etc/systemd/journald.conf.d/* ]]; then
    # Marker-bearing so the kit recognizes the file as its own before the
    # staged write is faulted; a markerless file must be refused, not written.
    printf '# BRRDfeeder — protect MicroSD card from journald write wear.\npre-install sentinel\n' > "$target"
  else
    printf 'pre-install sentinel\n' > "$target"
  fi
  before_hash=$(sha256sum "$target")
  check_rc 1 env FAIL_ATOMIC_TARGET="$target" bash "$installer"
  grep -Fq "atomic write failed; original target retained: $target" "$tmp/output"
  [[ $(sha256sum "$target") == "$before_hash" ]]
  if compgen -G "${target}.installer.*" >/dev/null; then echo 'FAIL leftover failed staging file'; exit 1; fi
  receipt=$(sed -n 's/^\[brrdfeeder-install\] receipt: \([^ ]*\).*/\1/p' "$tmp/output" | head -1)
  bash "$receipt/ROLLBACK.sh" > "$tmp/rollback"
done
grep -Fq ' -> /etc/containers/systemd/brrdfeeder-engine.container' "$MV_LOG"
echo 'PASS five rootful targets: same-directory staging, truncated stage never replaces old bytes, failed stage cleaned'

mv "$quadlet" "$tmp/old-quadlet"
install -D -m 0755 /bin/true /home/operator/brrdfeeder-src/engine/target/release/engine
printf '[Container]\nImage=localhost/brrdfeeder-engine:renamed\n' > /etc/containers/systemd/renamed.container
check_rc 2 bash "$installer" --config /home/operator/config.yaml --dry-run
grep -Fq 'possible renamed BRRDfeeder Quadlet: /etc/containers/systemd/renamed.container' "$tmp/output"
rm /etc/containers/systemd/renamed.container
check_rc 2 env MOCK_ENGINE_ACTIVE=1 bash "$installer" --config /home/operator/config.yaml --dry-run
grep -Fq 'system brrdfeeder-engine.service is active' "$tmp/output"
[[ ! -e /home/operator/.config/systemd/user/brrdfeeder-engine.service ]]
echo 'PASS legacy guard rejects renamed rootful Quadlet and active system engine, with explicit home config'
check_rc 2 bash "$installer" --dry-run
grep -Fq 'legacy engine requires --config /home/operator/config.yaml' "$tmp/output"
check_rc 0 bash "$installer" --config /home/operator/config.yaml --dry-run
grep -Fq 'DEPRECATED: legacy bare-metal' "$tmp/output"
grep -Fq 'would write unit to /home/operator/.config/systemd/user/brrdfeeder-engine.service' "$tmp/output"
legacy_unit=/home/operator/.config/systemd/user/brrdfeeder-engine.service
mkdir -p "$(dirname "$legacy_unit")"
printf 'legacy unit sentinel\n' > "$legacy_unit"
legacy_hash=$(sha256sum "$legacy_unit")
check_rc 1 env FAIL_ATOMIC_TARGET="$legacy_unit" bash "$installer" --config /home/operator/config.yaml
grep -Fq "atomic write failed; original target retained: $legacy_unit" "$tmp/output"
[[ $(sha256sum "$legacy_unit") == "$legacy_hash" ]]
receipt=$(sed -n 's/^\[brrdfeeder-install\] receipt: \([^ ]*\).*/\1/p' "$tmp/output" | head -1)
bash "$receipt/ROLLBACK.sh" > "$tmp/rollback"
echo 'PASS legacy unit atomic-write failure retains original bytes too'
mkdir -p /home/operator/.config/containers/systemd
cp "$tmp/old-quadlet" /home/operator/.config/containers/systemd/brrdfeeder-engine.container
check_rc 2 bash "$installer" --dry-run
grep -Fq 'rootless Quadlet detected' "$tmp/output"
echo 'PASS legacy path plans user service with canonical config; wrong legacy config and rootless Quadlet fail closed'
echo 'PASS D12 installer fixture suite (no real radios, systemd, NATS or hosts)'
