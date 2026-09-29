#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Run on the host after the engine installer creates its service account,
# before enabling the status writer. Never run this inside the console image.
set -euo pipefail
fail() { printf 'BRRDhouse status provisioning: %s\n' "$*" >&2; exit 1; }
[[ $# -eq 0 ]] || fail 'No arguments accepted; the public status path is fixed.'
[[ $(id -u) -eq 0 ]] || fail 'Run as root to provision service-owned storage.'
readonly directory=/var/lib/brrdfeeder-status
record=$(getent passwd brrdfeeder) || fail 'Install the engine service account first.'
[[ $record != *$'\n'* ]] || fail 'Ambiguous brrdfeeder account.'
IFS=: read -r name _ service_uid service_gid _ service_home service_shell <<< "$record"
system_uid_max=$(awk '$1 == "SYS_UID_MAX" {print $2; exit}' /etc/login.defs)
system_uid_max=${system_uid_max:-999}
[[ $name == brrdfeeder && $service_uid =~ ^[0-9]+$ && $service_gid =~ ^[0-9]+$ && $system_uid_max =~ ^[0-9]+$ ]] || fail 'Invalid service identity.'
[[ $service_uid -gt 0 && $service_uid -le $system_uid_max && $service_gid -gt 0 && $service_home == /* && $service_home != / ]] || fail 'Require a non-root system account with a dedicated home.'
case "$service_shell" in */nologin|*/false) ;; *) fail 'Refusing a login account.';; esac
[[ $(id -u brrdfeeder) == "$service_uid" && $(id -g brrdfeeder) == "$service_gid" ]] || fail 'Inconsistent service identity.'
group_record=$(getent group "$service_gid") || fail 'Cannot resolve service group.'
IFS=: read -r group_name _ _ members <<< "$group_record"
[[ $group_name == brrdfeeder && ( -z $members || $members == brrdfeeder ) ]] || fail 'Require the dedicated brrdfeeder group.'
[[ ! -L $directory ]] || fail 'Public status directory must not be a symlink.'
if [[ -e $directory ]]; then
  [[ -d $directory ]] || fail 'Public status path is not a directory.'
  owner=$(stat -c %u "$directory")
  [[ $owner == 0 || $owner == "$service_uid" ]] || fail 'Directory belongs to another account.'
  # Do not expose an existing credentials/configuration directory. Do not
  # recursively chown, follow links, or repair arbitrary contents as root.
  shopt -s nullglob dotglob
  for entry in "$directory"/*; do
    [[ ( $entry == "$directory/status.json" || $entry == "$directory/startup.json" ) && -f $entry && ! -L $entry ]] || fail 'Directory contains unexpected entries; review it before provisioning.'
    [[ $(stat -c %a "$entry") == 644 && $(stat -c %h "$entry") == 1 ]] || fail 'Existing status must be a single-link regular file, mode 0644.'
  done
fi
install -d -m 0755 -o "$service_uid" -g "$service_gid" "$directory"
[[ $(stat -c '%u:%g:%a' "$directory") == "$service_uid:$service_gid:755" ]] || fail 'Directory ownership/mode verification failed.'
printf 'Provisioned %s owner=%s:%s mode=0755; engine RW, console RO.\n' "$directory" "$service_uid" "$service_gid"
printf 'Next: follow README unit setup to mount it in the engine and enable node.status_file.\n'
