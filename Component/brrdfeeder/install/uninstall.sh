#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Embedded verbatim in brrdfeeder-install.sh. No network and no configurable roots.
set -euo pipefail
export LC_ALL=C
dry=${1:?internal: dry-run flag required}
adopt=${2:?internal: legacy adoption flag required}
audit=${3:-}
stage=${4:-confirm}
yes=${5:-0}
[[ $dry =~ ^[01]$ && $adopt =~ ^[01]$ ]] || exit 2
[[ $stage == plan || $stage == confirm ]] && [[ $yes =~ ^[01]$ ]] || exit 2
log() { printf '[brrdfeeder-uninstall] %s\n' "$*"; }
notice() { if declare -F log_event >/dev/null; then log_event NOTICE "$*"; else log "$*"; fi; }
die() {
  log "REFUSED: $*" >&2
  local reason="$*" path quoted kind owner empty=not-applicable
  if [[ $reason != *'type='* && $reason =~ (/[^[:space:]\;\,\)]+) ]]; then
    path=${BASH_REMATCH[1]}
    kind=$(stat -c %F -- "$path" 2>/dev/null) || kind=unavailable
    owner=$(stat -c %u:%g -- "$path" 2>/dev/null) || owner=unavailable
    if [[ -d $path && ! -L $path ]]; then
      if [[ -z $(find -P "$path" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null) ]]; then empty=empty; else empty=non-empty; fi
    elif [[ -f $path && ! -L $path ]]; then
      if [[ -s $path ]]; then empty=non-empty; else empty=empty; fi
    fi
    printf -v quoted %q "$path"
    log "Found: type=$kind owner=$owner emptiness=$empty. Inspect with: sudo stat -- $quoted" >&2
  fi
  log 'Next step: keep unverified data; run sudo brrdfeeder uninstall --dry-run to review the refusal. Correct only verified package ownership/state, then run sudo brrdfeeder uninstall again. If ownership is uncertain, share this refusal with support.' >&2
  exit 1
}
[[ $EUID == 0 ]] || die 'Run with sudo.'
exists() { [[ -e $1 || -L $1 ]]; }
act() {
  if (( dry )); then log "WOULD: $*";
  elif declare -F run_step >/dev/null; then run_step "uninstall/${1##*/}" "$@";
  else "$@"; fi
}
absent() { log "nothing to do: $* absent"; }

confirm_removal() {
  [[ $stage == confirm ]] || return 0
  notice 'BRRDfeeder removal plan:'
  notice '  Stop and remove the engine, console and updater services; restore recorded Bluetooth settings.'
  notice '  Delete configuration, credentials, GPS/status files and local sensor data.'
  notice '  Remove about 120 MB of engine images plus the console; reinstall downloads them again.'
  notice '  Remove the service accounts and this local brrdfeeder command.'
  notice '  Keep OS packages and diagnostic logs; other applications may need them.'
  notice '  Server enrollment is not removed. The same owner and hostname renew the same node ID with fresh credentials; a different owner or hostname can create a new node.'
  notice '  Radio soft-block state is not restored: the engine may have unblocked its adapter and systemd-rfkill may retain it. Bluetooth service settings are restored from the receipt.'
  if (( dry )); then notice 'Dry-run only: nothing will be removed. Full plan is in this run’s log.'; return; fi
  if (( yes )); then
    notice 'Removing BRRDfeeder… (usually about 30 s; large image stores can take longer)'
    log 'confirmation accepted through --yes'; return
  fi
  local terminal=${BRRDFEEDER_TTY_FD:-} answer
  if [[ ! $terminal =~ ^[0-9]+$ ]] || ! [[ -t $terminal ]]; then
    if ! { exec {terminal}<>/dev/tty; } 2>/dev/null; then
      die 'No terminal for confirmation. Run curl -fsSL https://get.cybrrd.com | bash -s uninstall in a terminal, or add --yes for automation. Nothing was removed.'
    fi
  fi
  notice 'Remove BRRDfeeder from this Pi? [y/N]'
  # A sudo use_pty terminal can exist yet have no keyboard behind it (legacy
  # curl | sudo bash). Do not let that shape strand removal indefinitely.
  if ! IFS= read -r -t 60 answer <&"$terminal"; then
    die 'Confirmation input unavailable or timed out after 60 seconds. Nothing was removed. Re-run: curl -fsSL https://get.cybrrd.com | bash -s uninstall (or add --yes for automation).'
  fi
  case $answer in y|Y|yes|YES)
    notice 'Removing BRRDfeeder… (usually about 30 s; large image stores can take longer)'
    log 'confirmation accepted through terminal';;
    *) die 'Uninstall cancelled; nothing was removed.';; esac
}

progress_phase() {
  (( ! dry )) || return 0
  if declare -F log_event >/dev/null; then log_event PHASE "uninstall"$'\t'"$*";
  else notice "$* …"; fi
}

audit_legacy_account() {
  python3 - "$1" "$2" <<'ACCOUNT_AUDIT_EOF'
#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Read-only legacy-account audit. No new packages, writes or network calls."""
import gzip
import os
from pathlib import Path
import platform
import re
import sqlite3
import stat
import struct
import subprocess
import sys

LOG = Path('/var/log')
WTMPDB = Path('/var/lib/wtmpdb')
LASTLOG2 = Path('/var/lib/lastlog')
PROC = Path('/proc')
LIMIT = 64*1024*1024
HISTORY_ROOTS = (Path('/var/log'), Path('/var/lib'))


def history_files():
    paths = set(LOG.glob('wtmp*')) | set(WTMPDB.glob('wtmp*.db')) | set(LASTLOG2.glob('lastlog2*.db'))
    seen = set()
    result = []
    for path in sorted(paths):
        if path.name.endswith(('-wal','-shm','-journal')): continue
        if path.is_symlink():
            try:
                real = path.resolve(strict=True)
                info = real.stat()
            except (OSError, RuntimeError) as error:
                raise ValueError('login history alias cannot resolve safely: '+str(path)) from error
            if not any(real.is_relative_to(root) for root in HISTORY_ROOTS):
                raise ValueError('login history alias target is outside /var/log and /var/lib: '+str(path))
            if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
                raise ValueError('login history alias target must be regular, root-owned and not group/other-writable: '+str(path))
        else:
            real = path
            info = real.stat()
            if not stat.S_ISREG(info.st_mode):
                raise ValueError('login history has an unexpected file type: '+str(path))
        # Validate every alias before deduplication, even if another path has
        # already selected this inode. A safe Trixie alias is not a second DB.
        key = (info.st_dev, info.st_ino)
        if key not in seen:
            seen.add(key)
            result.append((real, info))
    return result


def unchanged_file(before, after):
    return all(getattr(before, field) == getattr(after, field) for field in
               ('st_dev','st_ino','st_mode','st_uid','st_gid','st_size','st_mtime_ns','st_ctime_ns'))


def history(user, uid):
    # Debian bookworm's binary records and trixie's replacements, without
    # requiring last/lastlog/lastlog2 packages. Read SQL into memory: even a
    # read-only on-disk SQLite connection can create WAL shared-memory files.
    for path, selected in history_files():
        for suffix in ('-wal','-journal'):
            side = Path(str(path)+suffix)
            if side.exists() and side.stat().st_size:
                raise ValueError('login history is being updated; retry after its database transaction completes')
        before = path.stat()
        if path.is_symlink() or not unchanged_file(selected, before):
            raise ValueError('login history changed after path validation')
        opener = gzip.open if path.suffix=='.gz' else open
        with opener(path,'rb') as stream: data=stream.read(LIMIT+1)
        after = path.stat()
        for suffix in ('-wal','-journal'):
            side = Path(str(path)+suffix)
            if side.exists() and side.stat().st_size:
                raise ValueError('login history is being updated; retry after its database transaction completes')
        if len(data)>LIMIT or not unchanged_file(before, after):
            raise ValueError('login history is too large or changed during inspection; cannot safely recognise this account')
        if not data: continue
        if data.startswith(b'SQLite format 3\x00'):
            with sqlite3.connect(':memory:') as db:
                db.deserialize(data)
                db.execute('PRAGMA trusted_schema=OFF')
                if path.name.startswith('lastlog2'):
                    found=db.execute('SELECT 1 FROM Lastlog2 WHERE Name=? AND COALESCE(Time,0) != 0 LIMIT 1',(user,)).fetchone()
                else:
                    found=db.execute('SELECT 1 FROM wtmp WHERE User=? LIMIT 1',(user,)).fetchone()
        else:
            arch=platform.machine()
            # glibc utmp: x86_64's TIME64_COMPAT32 layout is 384 bytes;
            # Linux aarch64 uses 64-bit session/time fields and 400 bytes.
            size={'x86_64':384,'aarch64':400,'arm64':400}.get(arch)
            if not size or len(data)%size: raise ValueError('login history binary format is not recognised on this host')
            found=any(struct.unpack_from('@h',data,n)[0]==7 and data[n+44:n+76].split(b'\0',1)[0]==user.encode()
                      for n in range(0,len(data),size))
        if found: raise ValueError('login history exists for '+user+'; it may be a human account, so it will not be removed')
    path=LOG/'lastlog'
    if path.exists() or path.is_symlink():
        if path.is_symlink() or not path.is_file(): raise ValueError('login history lastlog path is unsafe')
        width={'x86_64':4,'aarch64':8,'arm64':8}.get(platform.machine())
        if not width: raise ValueError('login history lastlog format is unknown on this host')
        size=width+32+256
        if path.stat().st_size%size: raise ValueError('login history lastlog size is invalid')
        with path.open('rb') as stream:
            stream.seek(uid*size); timestamp=stream.read(width)
        if timestamp and int.from_bytes(timestamp,sys.byteorder):
            raise ValueError('login history exists for '+user+'; it may be a human account, so it will not be removed')


def command(args, **identity):
    run=subprocess.run(args,text=True,capture_output=True,timeout=10,**identity)
    if run.returncode: raise ValueError('cannot verify service process ownership; local systemd inspection failed')
    return run.stdout.strip()


def processes(user, uid):
    owned=[]
    for path in PROC.iterdir():
        if not path.name.isdigit(): continue
        try:
            text=(path/'status').read_text()
            ids=next(line.split()[1:] for line in text.splitlines() if line.startswith('Uid:'))
            if str(uid) not in ids: continue
            groups=[line.split(':',2)[2] for line in (path/'cgroup').read_text().splitlines() if line.startswith('0::')]
            if len(groups)!=1: raise ValueError('cannot verify process ownership without its unified systemd cgroup')
            owned.append((path,groups[0]))
        except FileNotFoundError: continue # process exited during observation
    if not owned: return
    if user=='brrdfeeder':
        expected='/system.slice/brrdfeeder-engine.service'
        actual=command(['systemctl','show','brrdfeeder-engine.service','-p','ControlGroup','--value'])
        infrastructure={}
    else:
        base=f'/user.slice/user-{uid}.slice/user@{uid}.service'
        expected=base+'/app.slice/brrdhouse.service'
        # A read-only audit must not open a PAM session through runuser (which
        # can itself write login/audit records during --dry-run).
        actual=command(['systemctl','--user','show','brrdhouse.service','-p','ControlGroup','--value'],
                       user=user,group=user,extra_groups=[],
                       env=dict(os.environ,XDG_RUNTIME_DIR=f'/run/user/{uid}',
                                DBUS_SESSION_BUS_ADDRESS=f'unix:path=/run/user/{uid}/bus'))
        infrastructure={
            base+'/init.scope':{'/usr/lib/systemd/systemd','/lib/systemd/systemd','/usr/lib/systemd/systemd-executor'},
            base+'/app.slice/dbus.service':{'/usr/bin/dbus-daemon','/usr/bin/dbus-broker','/usr/bin/dbus-broker-launch'},
            base+'/session.slice/dbus.service':{'/usr/bin/dbus-daemon','/usr/bin/dbus-broker','/usr/bin/dbus-broker-launch'},
        }
    if actual and actual!=expected: raise ValueError('unexpected package service cgroup; cannot safely recognise its processes')
    for path,group in owned:
        if actual==expected and (group==expected or group.startswith(expected+'/')): continue
        try:
            if group in infrastructure and str((path/'exe').resolve(strict=True)) in infrastructure[group]: continue
            # Exit races do not turn a completed process into a false refusal.
            if not path.exists(): continue
        except FileNotFoundError:
            if not path.exists(): continue
        raise ValueError('unrelated process '+path.name+' runs as '+user+' outside BRRDfeeder services; stop or review it before removal')


def main():
    if len(sys.argv)!=3 or sys.argv[1] not in ('brrdfeeder','brrdhouse') or not re.fullmatch(r'[0-9]+',sys.argv[2]):
        raise ValueError('unexpected account identity')
    user,uid=sys.argv[1],int(sys.argv[2])
    if uid<100: raise ValueError('UID is below the service-account safety floor')
    history(user,uid)
    processes(user,uid)
    print('account audit: no recorded login history; no unrelated process for '+user)


if __name__=='__main__':
    try: main()
    except Exception as error:
        print('REFUSED: '+(str(error) if type(error) is ValueError else 'cannot read/validate login history or process ownership ('+type(error).__name__+')'),file=sys.stderr)
        sys.exit(1)
ACCOUNT_AUDIT_EOF
}

# Validate every ancestor, including dangling links, before destructive operations.
# Never use rm -rf, follow a symlink, cross a mount, or expand a user-selected root.
safe_path() {
  local p=$1 cursor=$1
  [[ $p == /* && $p != / && $(realpath -m -- "$p") == "$p" ]] || die "unexpected path resolution: $p"
  while [[ $cursor != / ]]; do
    [[ ! -L $cursor ]] || die "symlink: $cursor"
    cursor=${cursor%/*}; cursor=${cursor:-/}
  done
}
safe_file() {
  safe_path "$1"
  if exists "$1"; then
    [[ -f $1 && $(stat -c %h -- "$1") == 1 && $(stat -c %u -- "$1") == 0 ]] || die "not a root-owned single-link file: $1"
  fi
}
no_mounts() {
  local root=$1 point
  while IFS= read -r point; do
    [[ $point != "$root" && $point != "$root/"* ]] || die "mounted filesystem at $point; stop/unmount its owner first"
  done < <(findmnt -rn -o TARGET)
}
safe_tree() {
  safe_path "$1"
  [[ ! -e $1 || -d $1 ]] || die "not a directory: $1"
  no_mounts "$1"
}

refuse_entry() {
  local path=$1 reason=$2 kind owner empty=not-applicable quoted
  kind=$(stat -c %F -- "$path" 2>/dev/null) || kind=unavailable
  owner=$(stat -c %u:%g -- "$path" 2>/dev/null) || owner=unavailable
  if [[ -d $path && ! -L $path ]]; then
    if [[ -z $(find -P "$path" -mindepth 1 -maxdepth 1 -print -quit) ]]; then empty=empty; else empty=non-empty; fi
  elif [[ -f $path && ! -L $path ]]; then
    if [[ -s $path ]]; then empty=non-empty; else empty=empty; fi
  fi
  printf -v quoted %q "$path"
  die "$reason: $path (type=$kind owner=$owner emptiness=$empty). Next step: sudo stat -- $quoted; review and move any confirmed unrelated content outside the service home before retrying sudo brrdfeeder uninstall."
}

service_home_residue() {
  local root=$1 entry relative
  safe_tree "$root"
  # These are directory-only skeletons made by service-user/systemd/Podman
  # initialization. The engine is rootful: no rootless storage DATA belongs here.
  while IFS= read -r -d '' entry; do
    relative=${entry#/var/lib/brrdfeeder/}
    case $relative in .config|.config/systemd|.config/systemd/user|.local|.local/share|.local/share/containers|.cache) ;;
      *) refuse_entry "$entry" 'unrecognised service-home residue';; esac
    [[ -d $entry && ! -L $entry && $(stat -c %u "$entry") == "${uid[brrdfeeder]:-absent}" ]] \
      || refuse_entry "$entry" 'unsafe service-home residue'
  done < <(find -P "$root" -xdev -print0)
}

declare -A uid=() gid=()
for user in brrdfeeder brrdhouse; do
  home=/var/lib/$user
  safe_tree "$home"
  safe_file "/etc/brrdfeeder/.installer-created-$user"
  if record=$(getent passwd "$user"); then
    [[ $record != *$'\n'* ]] || die "ambiguous identity: $user"
    IFS=: read -r name _ id group _ account_home shell <<< "$record"
    [[ $name == "$user" ]] || die "$user account name is inconsistent; refusing another identity"
    [[ $id =~ ^[0-9]+$ && $id -ge 100 ]] || die "$user UID is below 100 or invalid; system identities must not be removed"
    [[ $group =~ ^[0-9]+$ && $group -ge 100 ]] || die "$user primary group is a protected system group"
    [[ $account_home == "$home" ]] || die "$user home is not $home; it may contain unrelated personal data"
    [[ $shell == /usr/sbin/nologin ]] || die "$user has a login shell; it may be a human account"
    if [[ $user == brrdfeeder ]]; then
      max=$(awk '$1=="SYS_UID_MAX" {print $2; exit}' /etc/login.defs); max=${max:-999}
      [[ $max =~ ^[0-9]+$ && $id -le $max ]] || die 'brrdfeeder is not a system account'
    fi
    [[ $(getent passwd | awk -F: -v n="$id" '$3==n {c++} END {print c+0}') == 1 ]] || die "shared UID: $id"
    groups=$(id -nG "$user") || die "cannot inspect $user group memberships"
    for privilege in sudo adm wheel root; do
      [[ " $groups " != *" $privilege "* ]] || die "$user belongs to privileged group $privilege; it may be a human administrator"
    done
    [[ $(getent group "$group") == "$user:x:$group:" ]] || die "non-dedicated primary group for $user; other identities must not be affected"
    [[ $(passwd -S "$user" | awk '{print $2}') == L ]] || die "$user password is not locked; it may be a human account"
    receipt=/etc/brrdfeeder/.installer-created-$user
    expected="v1:$user:$id:$group:$home:$shell"
    if [[ -s $receipt ]]; then
      [[ $(stat -c %a "$receipt") == 600 && $(<"$receipt") == "$expected" ]] || die "account receipt mismatch: $user"
    else
      if [[ -f $receipt ]]; then
        [[ $(stat -c %a "$receipt") == 600 ]] || die "unsafe empty account receipt: $user"
        [[ $(stat -c %u /etc/brrdfeeder) == 0 ]] || die 'unsafe account receipt directory owner'
        receipt_mode=$(stat -c %a /etc/brrdfeeder)
        (( (8#$receipt_mode & 0022) == 0 )) || die 'writable account receipt directory'
        log "interrupted empty account receipt: $user; requiring full legacy identity audit"
      fi
      audit_legacy_account "$user" "$id" || die "$user cannot be safely recognised; see the named login history or unrelated process check above"
      if (( adopt )); then log "EXPLICIT LEGACY ADOPTION requested for $user; all safety checks still apply"; fi
      log "legacy account recognised: $user uid=$id gid=$group; no creation receipt, profile/history/process checks passed"
    fi
    uid[$user]=$id; gid[$user]=$group
    [[ ! -d $home || $(stat -c %u "$home") == "$id" ]] || die "wrong home owner: $home"
    safe_path "/run/user/$id"
    [[ ! -e /run/user/$id || ( -d /run/user/$id && $(stat -c %u "/run/user/$id") == "$id" ) ]] || die "wrong user-runtime owner: $user"
    if [[ ! -f $receipt && $stage == confirm ]]; then notice "recognised $user as a BRRDfeeder service account"; fi
  else
    absent "account $user"
    [[ ! -d $home ]] || die "orphan home $home without its account; refuse to guess ownership"
    if getent group "$user" >/dev/null; then die "orphan group $user; refuse to guess ownership"; fi
  fi
done

safe_file /usr/local/sbin/brrdfeeder
if exists /usr/local/sbin/brrdfeeder; then
  grep -qF '# BRRDfeeder local product command — self-contained recovery, no download.' /usr/local/sbin/brrdfeeder \
    || { [[ ! -s /usr/local/sbin/brrdfeeder && -n ${uid[brrdfeeder]:-} && -f /etc/brrdfeeder/.installer-created-brrdfeeder ]]; } \
    || die 'Unrelated local brrdfeeder command; it will not be removed'
fi

files=(/etc/brrdfeeder/config.yaml /etc/brrdfeeder/brrdhouse.container
  /usr/local/libexec/brrdfeeder-host-memory
  /etc/systemd/system/brrdfeeder-memory.service /etc/systemd/system/brrdfeeder-memory.timer
  /etc/brrdfeeder/updater.json /etc/brrdfeeder/.updater-helper.sha256
  /usr/local/libexec/brrdfeeder-release
  /usr/local/libexec/brrdfeeder-release-launch
  /usr/local/libexec/brrdfeeder-release.previous /usr/local/libexec/brrdfeeder-release.previous.sha256
  /etc/systemd/system/brrdfeeder-release-recover.service
  /etc/systemd/system/brrdfeeder-host-update.service /etc/systemd/system/brrdfeeder-host-update.timer
  /etc/systemd/system/brrdfeeder-release-poll.service /etc/systemd/system/brrdfeeder-release-poll.timer
  /etc/brrdfeeder/secrets/brrdfeeder.creds /etc/brrdfeeder/secrets/oauth_refresh.token
  /etc/brrdfeeder/.installer-created-brrdfeeder /etc/brrdfeeder/.installer-created-brrdhouse
  /etc/containers/systemd/brrdfeeder-engine.container
  /etc/systemd/system/brrdfeeder-updater.path /etc/systemd/system/brrdfeeder-updater.service
  /etc/udev/rules.d/99-cybrrd-brrdfeeder.rules
  /etc/systemd/journald.conf.d/99-brrdfeeder.conf /etc/systemd/journald.conf.d/99-brrdfeeder-open.conf
  /etc/chrony/conf.d/10-brrdfeeder.conf /etc/chrony/conf.d/10-pack.conf
  /usr/local/libexec/brrdfeeder-image-identity /usr/local/libexec/brrdfeeder-provision-status
  /usr/local/libexec/brrdfeeder-gps-seed
  /usr/local/libexec/brrdfeeder-gps-runtime
  /etc/systemd/system/brrdfeeder-gps-runtime.service
  /etc/systemd/system/brrdfeeder-gps-runtime.timer
  /usr/local/libexec/brrdfeeder-bluetooth /etc/brrdfeeder/.bluetooth-prior.json
  /usr/local/bin/brrdfeeder-updater.sh /run/brrdfeeder-engine.cid /run/brrdfeeder-engine.service.cid)
shopt -s nullglob dotglob
for directory in /etc/brrdfeeder /etc/brrdfeeder/secrets /usr/local/sbin /usr/local/libexec /etc/chrony/conf.d /etc/udev/rules.d /etc/systemd/journald.conf.d /etc/containers/systemd; do
  for temporary in "$directory"/.brrdfeeder-atomic-*; do
    [[ ${temporary##*/} =~ ^\.brrdfeeder-atomic-[a-z0-9_]{8}$ ]] || die "unexpected atomic temporary: $temporary"
    safe_file "$temporary"
    files+=("$temporary")
  done
done
for temporary in /usr/local/sbin/.brrdfeeder.*; do
  [[ $temporary =~ /\.brrdfeeder\.[A-Za-z0-9]{8}$ ]] || die "unexpected local command temporary: $temporary"
  files+=("$temporary")
done
for backup in /etc/containers/systemd/brrdfeeder-engine.container.bak.*; do
  [[ $backup =~ \.bak\.[0-9]{8}-[0-9]{6}$ ]] || die "unrecognised backup: $backup"
  files+=("$backup")
done
for temporary in /etc/brrdfeeder/.bluetooth-*; do
  [[ $temporary =~ /\.bluetooth-[a-z0-9_]{8}$ ]] || continue
  files+=("$temporary")  # mkstemp residue from interrupted atomic helper writes
done
for file in "${files[@]}"; do safe_file "$file"; done
if [[ -f /usr/local/libexec/brrdfeeder-release ]]; then
  [[ -f /etc/brrdfeeder/.updater-helper.sha256 ]] || die 'missing updater helper ownership receipt'
  [[ $(< /etc/brrdfeeder/.updater-helper.sha256) =~ ^[a-f0-9]{64}'  /usr/local/libexec/brrdfeeder-release'$ ]] || die 'invalid updater helper receipt'
  sha256sum --check --status /etc/brrdfeeder/.updater-helper.sha256 || die 'updater helper hash differs from installer receipt'
fi
if [[ -f /usr/local/libexec/brrdfeeder-release.previous ]]; then
  [[ -f /usr/local/libexec/brrdfeeder-release.previous.sha256 ]] || die 'missing retained updater checksum'
  prior_hash=$(< /usr/local/libexec/brrdfeeder-release.previous.sha256)
  [[ $prior_hash =~ ^[a-f0-9]{64}$ ]] || die 'invalid retained updater checksum'
  [[ $(sha256sum /usr/local/libexec/brrdfeeder-release.previous | cut -d' ' -f1) == "$prior_hash" ]] || die 'retained updater checksum mismatch'
fi
for pair in \
  '/usr/local/libexec/brrdfeeder-host-memory|Read-only host observations; only stop mode writes a durable OOM receipt.' \
  '/etc/systemd/system/brrdfeeder-memory.service|BRRDfeeder read-only host memory observations' \
  '/etc/systemd/system/brrdfeeder-memory.timer|BRRDfeeder periodic host memory observations' \
  '/etc/containers/systemd/brrdfeeder-engine.container|Installed by brrdfeeder-install.sh.' \
  '/etc/brrdfeeder/brrdhouse.container|Description=BRRDhouse intrinsic read-only LAN status console' \
  '/etc/systemd/system/brrdfeeder-updater.path|# BRRDfeeder signed-update watcher.|#185 Drop 2' \
  '/etc/systemd/system/brrdfeeder-updater.service|# BRRDfeeder signed-update service.|#185 Drop 2' \
  '/etc/systemd/system/brrdfeeder-release-poll.service|BRRDfeeder release convergence' \
  '/etc/systemd/system/brrdfeeder-release-poll.timer|BRRDfeeder pull-only periodic' \
  '/etc/systemd/system/brrdfeeder-release-recover.service|Recover interrupted BRRDfeeder package transaction' \
  '/etc/systemd/system/brrdfeeder-host-update.service|Separate signed BRRDfeeder host-updater release' \
  '/etc/systemd/system/brrdfeeder-host-update.timer|Separate BRRDfeeder host-updater poll timer' \
  '/usr/local/libexec/brrdfeeder-release-launch|Stable host supervisor, intentionally NOT replaced' \
  '/etc/udev/rules.d/99-cybrrd-brrdfeeder.rules|cyBRRD BRRDfeeder substrate-stable device naming.' \
  '/etc/systemd/journald.conf.d/99-brrdfeeder.conf|# BRRDfeeder — protect MicroSD card' \
  '/etc/systemd/journald.conf.d/99-brrdfeeder-open.conf|BRRDfeeder Open tier' \
  '/etc/chrony/conf.d/10-pack.conf|Pack-canonical chrony override' \
  '/etc/chrony/conf.d/10-brrdfeeder.conf|# BRRDfeeder clock correction' \
  '/usr/local/libexec/brrdfeeder-image-identity|Host-side Quadlet lifecycle helper.' \
  '/usr/local/libexec/brrdfeeder-provision-status|BRRDhouse status provisioning:' \
  '/usr/local/libexec/brrdfeeder-gps-seed|BRRDfeeder GPS location seed' \
  '/usr/local/libexec/brrdfeeder-gps-runtime|BRRDfeeder GPS runtime transport' \
  '/etc/systemd/system/brrdfeeder-gps-runtime.service|BRRDfeeder GPS runtime transport' \
  '/etc/systemd/system/brrdfeeder-gps-runtime.timer|BRRDfeeder GPS runtime transport' \
  '/usr/local/libexec/brrdfeeder-bluetooth|BRRDfeeder Bluetooth ownership:' \
  '/usr/local/bin/brrdfeeder-updater.sh|# BRRDfeeder signed-update installer.|#185 Drop 2'; do
  file=${pair%%|*}; signature=${pair#*|}
  # Accept the current marker or its exact legacy marker; never bypass ownership.
  [[ ! -f $file ]] || grep -qF -- "${signature%%|*}" "$file" \
    || grep -qF -- "${signature#*|}" "$file" || die "unrecognised package file: $file"
done
for backup in /etc/containers/systemd/brrdfeeder-engine.container.bak.*; do
  grep -qF 'Installed by brrdfeeder-install.sh.' "$backup" || die "unrecognised backup content: $backup"
done
for tree in /etc/brrdfeeder /var/lib/brrdfeeder-status /run/brrdfeeder-identity /var/lib/brrdfeeder-updater /var/lib/brrdfeeder-memory /run/brrdfeeder-memory; do
  safe_tree "$tree"
done
[[ ! -d /etc/brrdfeeder || $(stat -c %u /etc/brrdfeeder) == 0 ]] || die 'config tree is not root-owned'
[[ ! -d /run/brrdfeeder-identity || $(stat -c %u /run/brrdfeeder-identity) == 0 ]] || die 'wrong runtime identity owner'
safe_tree /run/brrdfeeder-gps
if [[ -d /run/brrdfeeder-gps ]]; then
  [[ $(stat -c %u:%a /run/brrdfeeder-gps) == 0:700 ]] || die 'unsafe GPS runtime directory'
  for entry in /run/brrdfeeder-gps/*; do
    case ${entry##*/} in
      device|device.new) [[ $(stat -c %u:%h "$entry") == 0:1 && ( ( ! -L $entry && -c $entry ) || ( -L $entry && $(readlink "$entry") == /dev/null ) ) ]] || die "unsafe GPS runtime device: $entry";;
      lock|state.json|state.new) safe_file "$entry";;
      *) die "unexpected GPS runtime entry: $entry";;
    esac
  done
fi
for tree in /var/lib/brrdfeeder-memory /run/brrdfeeder-memory; do
  [[ ! -d $tree || $(stat -c %u "$tree") == 0 ]] || die 'wrong memory observer owner'
  for entry in "$tree"/*; do
    case ${entry##*/} in events.json|host.json|.lock|.memory-*) safe_file "$entry";;
      *) die "unrecognised memory state: $entry";; esac
  done
done
if [[ -d /var/lib/brrdfeeder-status ]]; then
  owner=$(stat -c %u /var/lib/brrdfeeder-status)
  [[ $owner == 0 || $owner == "${uid[brrdfeeder]:-absent}" ]] || die 'wrong status-directory owner'
fi
for entry in /etc/brrdfeeder/*; do
  case ${entry##*/} in config.yaml|secrets|brrdhouse.container|.installer-created-brrdfeeder|.installer-created-brrdhouse|.bluetooth-prior.json|updater.json|.updater-helper.sha256) ;;
    .release-config-*|.config-status-*) safe_file "$entry";;
    .brrdfeeder-atomic-*) [[ ${entry##*/} =~ ^\.brrdfeeder-atomic-[a-z0-9_]{8}$ ]] && safe_file "$entry" || die "unrecognised atomic temporary: $entry";;
    .bluetooth-*) [[ ${entry##*/} =~ ^\.bluetooth-[a-z0-9_]{8}$ ]] && safe_file "$entry" || die "unrecognised Bluetooth temporary: $entry";;
    *) die "unrecognised config-tree entry: $entry";; esac
done
safe_tree /etc/brrdfeeder/secrets
for entry in /etc/brrdfeeder/secrets/*; do
  case ${entry##*/} in brrdfeeder.creds|oauth_refresh.token) safe_file "$entry";;
    .brrdfeeder-atomic-*) [[ ${entry##*/} =~ ^\.brrdfeeder-atomic-[a-z0-9_]{8}$ ]] && safe_file "$entry" || die "unrecognised atomic temporary: $entry";;
    *) die "unrecognised secret: $entry";; esac
done
for entry in /var/lib/brrdfeeder/*; do
  case ${entry##*/} in policy_state.json|pending_update.json|pending_update.failed|update_outcome.json|update_transaction.json|release_currency.json|*.tmp|.update-*|.pending-*) ;;
    upward) safe_tree "$entry"; continue;;
    .config|.local|.cache) service_home_residue "$entry"; continue;;
    *) refuse_entry "$entry" 'unrecognised engine state';; esac
  [[ -f $entry && ! -L $entry ]] || refuse_entry "$entry" 'unsafe engine state'
done
for entry in /var/lib/brrdfeeder-updater/*; do
  case ${entry##*/} in state.json|lock|launch.lock|host-state.json|host-transaction.json|pending_update.json|pending.failed.json|rejected.json|.update-*) safe_file "$entry";;
    rejected-pending-*) [[ -d $entry ]] && safe_tree "$entry" || die "invalid rejected-pending record: $entry";;
    *) die "unrecognised updater state: $entry";; esac
done
for entry in /var/lib/brrdfeeder/upward/*; do
  case ${entry##*/} in operational.json|operational.tmp|operational.lock|red.json|red.tmp|red.lock) ;;
    *) die "unrecognised upward state: $entry";; esac
  [[ -f $entry && ! -L $entry ]] || die "unsafe upward state: $entry"
done
for entry in /var/lib/brrdfeeder-status/* /run/brrdfeeder-identity/*; do
  [[ -f $entry && ! -L $entry ]] || die "unsafe status/runtime entry: $entry"
done
for unit in /etc/systemd/system/brrdfeeder-engine.service /etc/systemd/system/brrdhouse.service; do
  ! exists "$unit" || die "unexpected native override of generated unit: $unit"
done
for unit in brrdfeeder-gps-runtime.service brrdfeeder-gps-runtime.timer brrdfeeder-engine.service brrdfeeder-memory.service brrdfeeder-memory.timer brrdfeeder-updater.path brrdfeeder-updater.service brrdfeeder-release-poll.timer brrdfeeder-release-poll.service brrdfeeder-release-recover.service brrdfeeder-host-update.service brrdfeeder-host-update.timer; do
  ! exists "/etc/systemd/system/$unit.d" || die "unexpected unit overrides: $unit.d"
done
console_links=()
for link in /etc/containers/systemd/users/*/brrdhouse.container; do
  safe_path "${link%/*}"
  [[ -L $link && $(readlink "$link") == /etc/brrdfeeder/brrdhouse.container ]] || die "unexpected console Quadlet link: $link"
  if [[ -n ${uid[brrdhouse]:-} ]]; then
    [[ $link == /etc/containers/systemd/users/"${uid[brrdhouse]}"/brrdhouse.container ]] || die "console link belongs to another UID: $link"
  fi
  console_links+=("$link")
done
for user in brrdfeeder brrdhouse; do
  safe_file "/var/lib/systemd/linger/$user"
  # Default Debian useradd creates no spool. Never silently orphan an unexpected one.
  ! exists "/var/mail/$user" || die "unexpected mail spool /var/mail/$user; review it before removing the account"
done
for device in /dev/cybrrd_gps /dev/cybrrd_ble; do
  [[ ! -e $device || -L $device ]] || die "not a udev symlink: $device"
done

log 'Local uninstall policy on apply: delete config, credentials, status, engine state, package images and service accounts.'
# Validate before stopping/removing anything. No receipt means we never owned
# bluetoothd; never infer a previous state for legacy installs.
if [[ -f /etc/brrdfeeder/.bluetooth-prior.json ]]; then
  [[ -f /usr/local/libexec/brrdfeeder-bluetooth ]] || die 'Bluetooth receipt exists but restore helper is absent; restore the shipped helper before uninstall'
  for helper_path in /usr /usr/local /usr/local/libexec /usr/local/libexec/brrdfeeder-bluetooth; do
    [[ $(stat -c %u "$helper_path") == 0 ]] || die "unsafe Bluetooth helper path owner: $helper_path"
    helper_mode=$(stat -c %a "$helper_path")
    (( (8#$helper_mode & 0022) == 0 )) || die "writable Bluetooth helper path: $helper_path"
  done
  python3 /usr/local/libexec/brrdfeeder-bluetooth check || die 'invalid Bluetooth ownership receipt'
fi
log 'Packages are LEFT IN PLACE: podman, jq, curl, ca-certificates, chrony, usbutils, uidmap, dbus-user-session, fuse-overlayfs, python3, python3-yaml (and iw if present); other applications may depend on them.'
log 'No server contact: server enrollment remains. Reinstall with the same owner and hostname renews the same node ID with fresh credentials; a different pair can create a new node.'
log 'Image policy on apply: remove package images by default; reinstall will re-pull roughly 120 MB plus the console. No --keep-images: deleting the rootless account also deletes its private image store.'
log 'Existing external audit logs and shared system journals are retained; historical clock steps, overwritten pre-install files and server enrollment cannot be reversed locally.'
# The public installer owns default-on redacted logging, before this helper.
# Never open an unfiltered secondary audit sink from this internal entrypoint.
[[ -z $audit ]] || die 'Use brrdfeeder-install.sh --audit-log for redacted logging.'

user_command() {
  runuser -u brrdhouse -- sh -c 'cd /var/lib/brrdhouse && exec env XDG_RUNTIME_DIR=/run/user/"$(id -u)" DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/"$(id -u)"/bus "$@"' sh "$@"
}
manager() { if [[ $1 == user ]]; then shift; user_command systemctl --user "$@"; else shift; systemctl "$@"; fi; }
stop_unit() {
  local scope=$1 unit=$2 state
  if (( dry )); then log "WOULD stop/disable $scope unit $unit if present (generated units lose enablement when their Quadlet is removed)"; return; fi
  if [[ $scope == user && ( -z ${uid[brrdhouse]:-} || ! -S /run/user/${uid[brrdhouse]}/bus ) ]]; then absent "user manager/unit $unit"; return; fi
  state=$(manager "$scope" show "$unit" --property=LoadState --value) || die "cannot inspect $scope unit $unit"
  if [[ $state == not-found ]]; then absent "$scope unit $unit"; return; fi
  manager "$scope" stop "$unit" || die "cannot stop $unit"
  state=$(manager "$scope" show "$unit" --property=UnitFileState --value)
  if [[ $state == generated || $state == transient ]]; then
    log "$unit: generated; removal of Quadlet + daemon-reload revokes WantedBy"
  else manager "$scope" disable "$unit" || die "cannot disable $unit"; fi
}
reset_failed_unit() {
  local scope=$1 unit=$2
  if (( dry )); then log "WOULD reset-failed $scope unit $unit if failed"; return; fi
  if [[ $scope == user && ( -z ${uid[brrdhouse]:-} || ! -S /run/user/${uid[brrdhouse]}/bus ) ]]; then return; fi
  # A removed unit can remain not-found/failed. Query failure state rather than
  # LoadState; never reset unrelated units or treat an absent healthy unit as an error.
  if manager "$scope" is-failed --quiet "$unit"; then
    act manager "$scope" reset-failed "$unit" || die "cannot clear failed state for $unit"
  fi
}
confirm_removal
progress_phase 'Stopping services'
stop_unit system brrdfeeder-memory.timer
stop_unit system brrdfeeder-memory.service
stop_unit system brrdfeeder-release-poll.timer
stop_unit system brrdfeeder-gps-runtime.timer
stop_unit system brrdfeeder-gps-runtime.service
stop_unit system brrdfeeder-host-update.timer
stop_unit system brrdfeeder-host-update.service
stop_unit system brrdfeeder-release-poll.service
stop_unit system brrdfeeder-release-recover.service
stop_unit system brrdfeeder-updater.path
stop_unit system brrdfeeder-updater.service
stop_unit system brrdfeeder-engine.service
stop_unit user brrdhouse.service

# Restore only AFTER stopping the engine (its exclusive HCI owner), and BEFORE
# deleting the receipt/helper. Failures retain both for a safe retry.
progress_phase 'Restoring Bluetooth service settings'
if [[ -f /etc/brrdfeeder/.bluetooth-prior.json ]]; then
  if (( dry )); then
    python3 /usr/local/libexec/brrdfeeder-bluetooth restore --dry-run
  else
    act python3 /usr/local/libexec/brrdfeeder-bluetooth restore
  fi
else absent 'Bluetooth ownership receipt; bluetooth.service left unchanged'; fi

declare -A visited_files=()
remove_file() {
  [[ -z ${visited_files[$1]:-} ]] || return 0
  visited_files[$1]=1
  if exists "$1"; then
    act rm -- "$1"
    if (( ! dry )); then log "REMOVED path: $1"; fi
  else absent "$1"; fi
}
progress_phase 'Removing service definitions'
remove_file /etc/systemd/system/brrdfeeder-memory.service
remove_file /etc/systemd/system/brrdfeeder-memory.timer
for file in /etc/systemd/system/brrdfeeder-updater.path /etc/systemd/system/brrdfeeder-updater.service /etc/containers/systemd/brrdfeeder-engine.container /etc/systemd/system/brrdfeeder-release-poll.timer /etc/systemd/system/brrdfeeder-release-poll.service /etc/systemd/system/brrdfeeder-release-recover.service /etc/systemd/system/brrdfeeder-host-update.service /etc/systemd/system/brrdfeeder-host-update.timer; do remove_file "$file"; done
for file in "${console_links[@]}"; do remove_file "$file"; done
for file in /etc/systemd/system/brrdfeeder-gps-runtime.timer /etc/systemd/system/brrdfeeder-gps-runtime.service; do remove_file "$file"; done
[[ ${#console_links[@]} -gt 0 ]] || absent '/etc/containers/systemd/users/<console-uid>/brrdhouse.container'
remove_file /etc/brrdfeeder/brrdhouse.container
act systemctl daemon-reload
if [[ -n ${uid[brrdhouse]:-} && -S /run/user/${uid[brrdhouse]}/bus ]]; then act user_command systemctl --user daemon-reload;
else log 'nothing to do: no active console user manager to reload; teardown order requires Quadlet removal before account deletion'; fi
for unit in brrdfeeder-release-poll.timer brrdfeeder-release-poll.service brrdfeeder-release-recover.service brrdfeeder-host-update.timer brrdfeeder-host-update.service brrdfeeder-updater.path brrdfeeder-updater.service brrdfeeder-engine.service brrdfeeder-gps-runtime.timer brrdfeeder-gps-runtime.service; do reset_failed_unit system "$unit"; done
reset_failed_unit user brrdhouse.service

# Only package containers and exclusive package image IDs. Never force rmi/prune.
pod() {
  local scope=$1; shift
  local -a prefix=()
  case $1 in
    stop|rm|rmi|unmount|system)
      if declare -F run_step >/dev/null; then prefix=(run_step "uninstall/podman-$1"); fi;;
  esac
  if [[ $scope == user ]]; then "${prefix[@]}" user_command podman "$@";
  else "${prefix[@]}" podman "$@"; fi
}
remove_images() {
  local scope=$1 repository=$2 container=$3 id refs ref matched foreign users ids consumers cid used_image used_name
  if (( dry )); then log "WOULD remove $scope container $container, then local images/digests exclusively belonging to $repository (offline enumeration on apply; no Podman calls in dry-run)"; return; fi
  if ! command -v podman >/dev/null; then
    [[ ! -d /var/lib/brrdhouse/.local/share/containers/storage ]] || die 'Podman is missing but console storage exists; cannot safely inventory it'
    absent "podman/$scope container $container and images"; return
  fi
  if [[ $scope == user && -z ${uid[brrdhouse]:-} ]]; then absent "rootless owner/store for $repository"; return; fi
  if [[ $scope == user && ! -d /var/lib/brrdhouse ]]; then absent "rootless home/store for $repository"; return; fi
  if [[ $scope == user ]]; then
    safe_path "/run/user/${uid[brrdhouse]}"
    if [[ ! -d /run/user/${uid[brrdhouse]} ]]; then install -d -m 0700 -o "${uid[brrdhouse]}" -g "${gid[brrdhouse]}" "/run/user/${uid[brrdhouse]}"; fi
  fi
  if pod "$scope" container exists "$container"; then
    ref=$(pod "$scope" inspect "$container" --format '{{.ImageName}}')
    [[ $ref == "$repository"@sha256:* ]] || die "foreign container using package name $container: $ref"
    # Quadlet's --rm can remove a container concurrently with stop/cleanup.
    # Accept only a positively established absence, never a generic Podman error.
    for operation in stop rm; do
      if ! pod "$scope" "$operation" "$container"; then
        if pod "$scope" container exists "$container"; then
          die "cannot $operation residual container $container"
        else
          [[ $? == 1 ]] || die "cannot inspect residual container $container after $operation"
          absent "$scope container $container (already removed by its service)"
        fi
      fi
    done
  else
    [[ $? == 1 ]] || die "cannot inspect $scope container $container"
    absent "$scope container $container"
  fi
  ids=$(pod "$scope" images --no-trunc --format '{{.ID}}' | sort -u) || die "cannot inspect $scope image store"
  matched=0
  for id in $ids; do
    refs=$(pod "$scope" image inspect "$id" --format '{{range .RepoTags}}{{println .}}{{end}}{{range .RepoDigests}}{{println .}}{{end}}') || die "cannot inspect image $id"
    foreign=0; local ours=0
    while IFS= read -r ref; do
      [[ -n $ref ]] || continue
      case $ref in "$repository"@sha256:*|"$repository":*) ours=1;; *) foreign=1;; esac
    done <<< "$refs"
    (( ours )) || continue
    (( foreign == 0 )) || die "image $id has unrelated aliases; will not delete shared data: $refs"
    # Compare actual image IDs; Podman's ancestor filter can miss digest-only images.
    consumers=$(pod "$scope" ps -a --no-trunc --format '{{.ID}}') || die 'cannot list image consumers'
    users=''
    for cid in $consumers; do
      ref=$(pod "$scope" inspect "$cid" --format '{{.Image}} {{.Name}}') || die 'cannot inspect image consumer'
      read -r used_image used_name <<< "$ref"
      if [[ ${used_image#sha256:} == "${id#sha256:}" ]]; then users+="$used_name "; fi
    done
    [[ -z $users ]] || die "image $id is used by unrelated containers: $users"
    pod "$scope" rmi "$id" || die "cannot remove image $id (no force)"
    log "REMOVED image $id; references/digests: $refs"
    matched=1
  done
  (( matched )) || absent "$scope images for $repository"
  if [[ $scope == user ]]; then
    [[ -z $(pod user ps -a --format '{{.Names}}') && -z $(pod user images -q) && -z $(pod user volume ls -q) && -z $(pod user secret ls -q) ]] \
      || die 'console home contains unrelated containers/images/volumes/secrets; preserving account and home'
    # Unmount rootless overlay storage in its namespace before host tree deletion.
    pod user unmount --all || die 'cannot unmount console storage'
    pod user system migrate || die 'cannot stop rootless storage pause process'
  fi
}
progress_phase 'Removing package images (about 120 MB plus console)'

# Running rollback anchors protect images from prune. Stop only exact D44 names
# bearing the installer-owned helper mount; refuse collisions, never force a
# broad image/container removal. Validate before mutating the matched object.
remove_anchors() {
  local scope=$1 prefix=$2 names name mounts
  if (( dry )); then log "WOULD remove $scope rollback containers matching ${prefix}-rollback-<16 lowercase hex>, after verifying their /hold helper mount"; return; fi
  if ! command -v podman >/dev/null || [[ $scope == user && -z ${uid[brrdhouse]:-} ]]; then absent "$scope rollback containers"; return; fi
  names=$(pod "$scope" ps -a --filter "name=^${prefix}-rollback-" --format '{{.Names}}') || die 'cannot enumerate rollback anchors'
  [[ -n $names ]] || { absent "$scope rollback containers"; return; }
  while IFS= read -r name; do
    [[ $name =~ ^${prefix}-rollback-[a-f0-9]{16}$ ]] || die "unexpected rollback name: $name"
    mounts=$(pod "$scope" inspect --type container --format '{{json .Mounts}}' "$name") || die 'cannot inspect rollback anchor'
    jq -e 'any(.[]; .Source == "/usr/local/libexec/brrdfeeder-release" and .Destination == "/hold" and .RW == false)' <<< "$mounts" >/dev/null || die "unowned rollback anchor: $name"
    pod "$scope" rm -f "$name" || die "cannot remove rollback anchor $name"
    log "REMOVED $scope rollback anchor $name"
  done <<< "$names"
}
remove_anchors system brrdfeeder
remove_anchors user brrdhouse
remove_images system ghcr.io/cybrrd/brrdfeeder brrdfeeder-engine
remove_images user ghcr.io/cybrrd/brrdhouse brrdhouse

progress_phase 'Removing package configuration and data'
had_udev=0; exists /etc/udev/rules.d/99-cybrrd-brrdfeeder.rules && had_udev=1
had_chrony=0
if exists /etc/chrony/conf.d/10-brrdfeeder.conf || exists /etc/chrony/conf.d/10-pack.conf; then had_chrony=1; fi
had_journal=0; exists /etc/systemd/journald.conf.d/99-brrdfeeder.conf && had_journal=1; exists /etc/systemd/journald.conf.d/99-brrdfeeder-open.conf && had_journal=1
for file in "${files[@]}"; do
  # Account receipts remain until deletion of the accounts, allowing retry after failure.
  [[ $file != /etc/brrdfeeder/.installer-created-* ]] || continue
  remove_file "$file"
done
if (( had_udev )); then
  # Remove old names BEFORE triggering remaining rules. Local rules may own the
  # same aliases; let udev reassert them and never delete them after that point.
  for device in /dev/cybrrd_gps /dev/cybrrd_ble; do remove_file "$device"; done
  act udevadm control --reload-rules
  act udevadm trigger --subsystem-match=tty --action=change
  act udevadm trigger --subsystem-match=misc --sysname-match=rfkill --action=change
  act udevadm settle --timeout=10
else absent 'package udev rule; no reload required'; fi
(( ! had_chrony )) || act systemctl try-restart chrony.service
(( ! had_journal )) || act systemctl try-restart systemd-journald.service

remove_tree() {
  local path=$1
  if [[ -d $path ]]; then
    safe_tree "$path"
    if (( dry )); then log "WOULD delete tree (no symlink traversal or mounts): $path";
    else find -P "$path" -xdev -depth -delete; log "REMOVED tree: $path"; fi
  else absent "$path"; fi
}
for tree in /etc/brrdfeeder/secrets /var/lib/brrdfeeder-status /run/brrdfeeder-identity /run/brrdfeeder-gps /var/lib/brrdfeeder-updater /var/lib/brrdfeeder-memory /run/brrdfeeder-memory; do remove_tree "$tree"; done
progress_phase 'Removing service accounts'
for user in brrdhouse brrdfeeder; do
  if [[ -n ${uid[$user]:-} ]]; then
    # Disable linger/terminate AFTER stopping the workload and reloading its manager.
    act loginctl disable-linger "$user"
    act systemctl stop "user@${uid[$user]}.service" "user-runtime-dir@${uid[$user]}.service"
    if (( ! dry )) && pgrep -u "${uid[$user]}" >/dev/null; then die "processes still run as $user; account retained"; fi
    if [[ $user == brrdfeeder ]]; then
      for residue in /var/lib/brrdfeeder/.config /var/lib/brrdfeeder/.local /var/lib/brrdfeeder/.cache; do
        ! exists "$residue" || service_home_residue "$residue"
      done
    fi
    remove_tree "/var/lib/$user"
    act userdel "$user"  # never -r: home was separately checked, no guessed directories
    if getent group "$user" >/dev/null; then act groupdel "$user"; else absent "group $user"; fi
    if (( dry )); then log "WOULD remove account: $user uid=${uid[$user]} gid=${gid[$user]} (userdel would remove subuid/subgid entries)";
    else log "REMOVED account: $user uid=${uid[$user]} gid=${gid[$user]} (subuid/subgid entries removed by userdel)"; fi
  else
    absent "account/group/home/linger $user"
    # A stale regular linger entry cannot start a nonexistent account.
    remove_file "/var/lib/systemd/linger/$user"
  fi
  remove_file "/etc/brrdfeeder/.installer-created-$user"
done
remove_tree /etc/brrdfeeder

# Remove scaffolding only when empty, never recursively delete shared unit trees.
parents=(/etc/containers/systemd/users /etc/containers/systemd /etc/systemd/journald.conf.d /etc/chrony/conf.d)
if [[ -n ${uid[brrdhouse]:-} ]]; then parents=("/etc/containers/systemd/users/${uid[brrdhouse]}" "${parents[@]}"); fi
for link in "${console_links[@]}"; do parents=("${link%/*}" "${parents[@]}"); done
for dir in "${parents[@]}"; do
  safe_path "$dir"
  if [[ -d $dir ]]; then
    if (( dry )); then log "WOULD rmdir only if empty: $dir";
    elif rmdir -- "$dir" 2>/dev/null; then log "REMOVED empty directory: $dir";
    else log "LEFT shared nonempty directory: $dir"; fi
  else absent "$dir"; fi
done
progress_phase 'Finishing local cleanup'
act systemctl daemon-reload
remove_file /usr/local/sbin/brrdfeeder
if (( dry )); then log 'Dry-run complete: no managed state changed; only its diagnostic log is retained. Image digests will be inspected offline on apply.';
else notice 'BRRDfeeder removed. OS packages, logs and server enrollment were kept.'; fi
