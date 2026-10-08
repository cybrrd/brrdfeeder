#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Self-heal: if invoked via `sh script` (dash on Debian-derived) the bash
# shebang is ignored. Re-exec under bash so bash-specific features
# (set -u + EUID + [[ ]] + process substitution) work.
if [ -z "${BASH_VERSION:-}" ]; then
  exec bash "$0" "$@"
fi

# BRRDfeeder local product command — self-contained recovery, no download.
# Normalize before the logging supervisor so modes and headers remain accurate.
case "${1:-}" in
  uninstall) shift; set -- --uninstall "$@" ;;
  status) shift; set -- --status "$@" ;;
  support-bundle) shift; set -- --support-bundle "$@" ;;
esac
if [[ ${0##*/} == brrdfeeder && $# -eq 0 ]]; then set -- --status; fi
if [[ ${1:-} == --help || ${1:-} == -h ]]; then
  printf '%s\n' \
    'Usage: brrdfeeder [status|uninstall|support-bundle] [options]' \
    'Installer: brrdfeeder-install.sh [--image DIGEST --console-image DIGEST --console-listen IP:PORT --interface IFACE]' \
    '                           [--gps-usb-id VID:PID]   Explicit non-u-blox GPS opt-in (headless)' \
    '  --verify / --status       Check installed state' \
    '  --uninstall [--dry-run]   Remove package, or show the removal plan' \
    '  --yes                    Confirm removal for automation' \
    '  --support-bundle         Collect redacted local diagnostics' \
    '  --no-verbose             Quiet install/uninstall routine output (default: verbose)' \
    '                           Progress, warnings, errors, enrollment and summary remain visible.' \
    '                           The redacted log always contains all output.' \
    '  --audit-log=PATH          Override default log path' \
    '  --help                   Show this help'
  exit 0
fi

# brrdfeeder-install.sh — engine + intrinsic BRRDhouse package installer
#
# Installs engines as system-mode
#         (rootful) Quadlet containers pulled from
#         ghcr.io/cybrrd/brrdfeeder.
#
# Installed components:
#   * Host packages (podman, jq, curl, chrony)
#   * Clock correction so secure connections work after a reboot
#   * Stable USB device naming (udev → /dev/cybrrd_gps, /dev/cybrrd_ble)
#   * BRRDfeeder-tier MicroSD wear protection (journald Storage=volatile)
#   * Root-owned config tree (/etc/brrdfeeder + 0700 secrets/)
#   * System-mode Quadlet at /etc/containers/systemd/brrdfeeder-engine.container
#   * Separately pinned rootless BRRDhouse user service, with no capabilities
#   * Shipped status provisioner: service-owned 0755 directory, engine RW / console RO
#   * Service activation + verification
#
# Container privileges:
#   The engine MUST run as a rootful (system-mode) Quadlet. Rootless
#   containers cannot grant CAP_NET_ADMIN on the host network namespace,
#   which is required for Wi-Fi monitor mode + libpcap raw sockets.
#
# Clock-hardening canon (2026-06-09 test-node-2 incident):
#   Pi/CM4 boards have no battery RTC. A wall-clock that boots in the past
#   makes certificates appear not yet valid and the engine's connection
#   fails. chrony with `makestep 1.0 -1` steps the clock aggressively at
#   boot, before the engine's first TLS attempt.
#
# What this script does NOT do:
#   * Does NOT modify the engine container image (pulled from ghcr.io;
#     substrate of truth lives in github.com/cybrrd/brrdfeeder)
#   * Does NOT install or modify Tailscale (separate control-plane ritual)
#   * Does NOT mint NATS credentials locally: the existing OAuth device flow
#     obtains them from flock after owner approval (or uses pre-provisioned creds).
#   * Does NOT touch existing bare-binary deploys (the legacy
#     pattern stays operational until separately migrated)
#
# Idempotency: safe to re-run. Each step checks state before mutation.
# Legacy migration is limited to the dedicated service account's passwd home.
# Login-owned legacy installations require an explicit operator migration.
#
# Usage:
#   Fresh: supply --image and --console-image approved digest references,
#          --console-listen LAN-IP:8080 and --interface. GPS supplies position.
#          --latitude/--longitude are a paired optional expert override.
#          See README.md for the complete single-command example.
#   sudo bash brrdfeeder-install.sh                                 # re-run, preserve both pins
#   sudo bash brrdfeeder-install.sh --dry-run                       # plan-only
#   sudo bash brrdfeeder-install.sh --verify                        # check state only
#   sudo bash brrdfeeder-install.sh --no-verbose                    # quieter terminal, full log
#   sudo brrdfeeder status                                        # local, read-only
#   sudo brrdfeeder uninstall                                     # summary + confirmation
#   sudo brrdfeeder support-bundle                                # email-ready local archive
#   sudo bash brrdfeeder-install.sh --uninstall                     # local package + images removal
#   sudo bash brrdfeeder-install.sh --uninstall --dry-run           # plan; writes only its log
#   sudo bash brrdfeeder-install.sh --support-bundle                # redacted local archive; no upload
#   --yes skips confirmation for automation; --adopt-legacy-accounts never bypasses safety checks.
#   sudo bash brrdfeeder-install.sh --audit-log=/path/to/audit.log  # override default log path

set -euo pipefail
# Default logging starts before argument validation, including failed invocations.
# Never xtrace enrollment values into a diagnostic stream.
set +x
if [[ -z ${BRRDFEEDER_LOG_CHILD:-} ]]; then
  if ! command -v python3 >/dev/null 2>&1; then
    # No Python: fixed-text Bash diagnostic only; never echo argv or environment.
    RUN_ID=${BRRDFEEDER_RUN_ID:-}
    [[ $RUN_ID =~ ^[0-9a-fA-F]{8}$ ]] || printf -v RUN_ID '%04x%04x' "$RANDOM" "$RANDOM"
    printf '[brrdfeeder-install] run %s\n' "$RUN_ID"
    TZ=UTC printf -v LOG_STAMP '%(%Y-%m-%dT%H%M%SZ)T' -1
    LOG_MODE=install
    for LOG_ARG in "$@"; do
      case $LOG_ARG in --uninstall) LOG_MODE=uninstall;; --verify) LOG_MODE=verify;; esac
    done
    for LOG_ARG in "$@"; do [[ $LOG_ARG != --dry-run ]] || LOG_MODE=dryrun; done
    LOG_REASON=''
    LOG_PATH="/var/log/brrdfeeder/$LOG_MODE-$LOG_STAMP-$RUN_ID.log"
    umask 027
    if [[ -L /var/log/brrdfeeder ]] || ! install -d -m 0750 -o root -g root /var/log/brrdfeeder 2>/dev/null || [[ $(stat -c '%u:%g:%a' /var/log/brrdfeeder) != 0:0:750 ]] || ! (set -C; : > "$LOG_PATH") 2>/dev/null; then
      LOG_PATH="/tmp/brrdfeeder-$LOG_MODE-$LOG_STAMP-$RUN_ID-$$.log"
      LOG_REASON='Cannot create a root-owned 0750 log directory or exclusive log file (Python unavailable).'
      printf '[brrdfeeder-install] logging degraded to %s: %s\n' "$LOG_PATH" "$LOG_REASON"
      (set -C; : > "$LOG_PATH") 2>/dev/null || LOG_PATH=/dev/null
    fi
    {
      printf '==== BRRDFEEDER INSTALL LOG ====\nrun_id=%s\nmode=%s\n' "$RUN_ID" "$LOG_MODE"
      [[ -z ${LOG_REASON:-} ]] || printf 'Logging fallback: %s\n' "$LOG_REASON"
      printf '==== ENVIRONMENT ====\npython3=absent\n==== BOOTSTRAP ====\nnotes=not collected without redactor\n==== TRANSCRIPT ====\n'
      printf '%s [FAIL] step=preflight/python3 rc=127\n' "$LOG_STAMP"
      printf '==== RESULT ====\nresult=FAILED\nfailed_phase=preflight\nfailed_step=preflight/python3\nexit_code=127\nlast_output=\n    Install Python 3: sudo apt install python3 then re-run.\nlog=%s\n' "$LOG_PATH"
    } >> "$LOG_PATH"
    printf 'Install Python 3: sudo apt install python3 then re-run.\n'
    printf '[brrdfeeder-install] FAILED at preflight/python3  (run %s)\n[brrdfeeder-install] Log: %s\n[brrdfeeder-install] For support, run:  sudo bash brrdfeeder-install.sh --support-bundle\n[brrdfeeder-install]   then email the file it names. (run %s)\n' "$RUN_ID" "$LOG_PATH" "$RUN_ID"
    exit 127
  fi
  exec python3 -B -c "$(cat <<'INSTALL_LOG_PY'
#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Embedded stdlib-only supervisor. Raw command output never has a disk spool.

The child retains stdin. Its new session has no controlling terminal, so an
explicit display descriptor is inherited when available. Output is relayed to
that terminal, then redacted for the log.
Command records use a per-process nonce, not user-controlled output prefixes.
"""
import base64
import contextlib
from collections import deque
import datetime as dt
import functools
import grp
import hashlib
import io
import json
import os
from pathlib import Path
import pwd
import re
import secrets
import shlex
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import threading
import time

LOG_DIR = Path('/var/log/brrdfeeder')
CREDS = Path('/etc/brrdfeeder/secrets/brrdfeeder.creds')
SYS_NET = Path('/sys/class/net')
ENDPOINTS = [('ingest.cybrrd.com', 4222), ('hospitality.cybrrd.com', 443),
             ('ghcr.io', 443), ('globe.cybrrd.com', 443)]
ANSI = re.compile(r'\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-_]')


def now():
    return dt.datetime.now(dt.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')


class Redactor:
    def __init__(self):
        self.known = {}
        self.nats = False
        self.legacy_network = False

    def remember(self, kind, value):
        if value:
            self.known[value] = '[REDACTED:' + kind + ']'
            if kind == 'nats-creds':
                for line in value.splitlines():
                    if line.strip():
                        self.known[line] = '[REDACTED:nats-creds]'

    def text(self, value):
        value = re.sub(r'[\x00-\x08\x0b-\x1f\x7f]', '', ANSI.sub('', value))
        # Newlines in a known credential must be handled before splitting lines.
        for secret in sorted(self.known, key=len, reverse=True):
            value = value.replace(secret, self.known[secret])
        lines = []
        for line in value.split('\n'):
            # Old installer logs contained whole-host iw/ip inventories. Drop
            # those sections when exporting a bundle, including SSID/channel/MAC.
            if re.match(r'^(?:net|route|phys|wireless_devices)=', line):
                self.legacy_network = True
                lines.append('[REDACTED:unscoped-network-inventory]')
                continue
            if self.legacy_network:
                if re.match(r'^(?:[a-z_]+=|====|\d{4}-\d\d-\d\dT)', line):
                    self.legacy_network = False
                else:
                    continue
            # Also cover a truncated old inventory or an incidental tool error.
            if re.search(r'(?i)\b(?:ssid|essid|bssid)\b\s*[:= ]', line):
                lines.append('[REDACTED:wireless-network-name]')
                continue
            line = re.sub(r'(?i)\b[0-9a-f]{2}(?::[0-9a-f]{2}){5}\b', '[REDACTED:mac-address]', line)
            if re.search(r'-+BEGIN (?:NATS USER JWT|USER NKEY SEED)-+', line):
                self.nats = True
            if self.nats:
                end = re.search(r'-+END (?:NATS USER JWT|USER NKEY SEED)-+', line)
                lines.append('[REDACTED:nats-creds]')
                if end:
                    self.nats = False
                continue
            line = re.sub(r'(?i)(authorization\s*:\s*)[^\r\n]+', r'\1[REDACTED:token]', line)
            line = re.sub(r'(?i)\bbearer\s+[A-Za-z0-9._~+/=-]+', 'Bearer [REDACTED:token]', line)
            line = re.sub(r'(?i)((?:user_code|device_code|claim_code)["\x27]?\s*[:=]\s*["\x27]?)[^\s"\x27&,}<>]+',
                          r'\1[REDACTED:device-code]', line)
            line = re.sub(r'(?i)(code shown there matches:\s*)\S+', r'\1[REDACTED:device-code]', line)
            line = re.sub(r'\beyJ[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+){0,2}', '[REDACTED:secret]', line)
            line = re.sub(r'(?i)(\b[\w-]{0,64}(?:token|secret|key|password)["\x27]?\s*[:=]\s*["\x27]?)[A-Za-z0-9_+/=-]{32,}',
                          r'\1[REDACTED:secret]', line)
            # NKey seed echoed without its creds-file delimiters.
            line = re.sub(r'\bSU[A-Z2-7]{56}\b', '[REDACTED:nats-creds]', line)
            lines.append(line)
        return '\n'.join(lines)


def read_text(path, limit=131072, ends=False):
    # Diagnostic reads never follow symlinks or device files.
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise OSError('not a regular file')
        if ends and os.fstat(fd).st_size > limit:
            head = os.read(fd, limit//2)
            os.lseek(fd, -limit//2, os.SEEK_END)
            return (head+b'\n[truncated: middle omitted; header and RESULT tail retained]\n'+os.read(fd,limit//2)).decode('utf-8','replace')
        return os.read(fd, limit).decode('utf-8', 'replace')
    finally:
        os.close(fd)


def capture(argv, timeout=6, limit=98304):
    """Bound time AND memory. Drain excess bytes without storing them anywhere."""
    import selectors
    started = time.monotonic()
    try:
        p = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                             start_new_session=True, env={**os.environ, 'NO_COLOR': '1',
                             'SYSTEMD_COLORS': '0', 'SYSTEMD_PAGER': '', 'TERM': 'dumb'})
    except OSError as exc:
        return 127, str(exc), 0.0
    sel = selectors.DefaultSelector()
    sel.register(p.stdout, selectors.EVENT_READ)
    data = bytearray()
    truncated = False
    timed_out = False
    while sel.get_map():
        if time.monotonic() - started > timeout:
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            timed_out = True
            break
        for key, _ in sel.select(.1):
            chunk = os.read(key.fd, 8192)
            if not chunk:
                sel.unregister(key.fileobj)
                continue
            data.extend(chunk)
            if len(data) > limit:
                del data[:-limit]
                truncated = True
    p.stdout.close()
    sel.close()
    # SIGKILL cannot finish a kernel D-state task. Never wait without a deadline,
    # including after killing it, and never leave its pipe in the caller's relay.
    rc = p.poll()
    if not timed_out and rc is None:
        try:
            rc = p.wait(timeout=max(0, timeout-(time.monotonic()-started)))
        except subprocess.TimeoutExpired:
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            timed_out = True
    suffix = ('\n[truncated: byte cap]' if truncated else '') + ('\n[timeout]' if timed_out else '')
    return 124 if timed_out else rc, data.decode('utf-8', 'replace')+suffix, time.monotonic()-started


@functools.lru_cache(maxsize=1)
def power_check():
    """One detached firmware observation per invocation, shared with preflight."""
    rc, out, _ = capture(['vcgencmd', 'get_throttled'], timeout=5, limit=256)
    if rc == 124:
        return 'unknown', '', 'power check: unknown (firmware did not answer). Reboot the Pi before retrying the power check; installation can continue.'
    if rc == 127:
        return 'unavailable', '', 'power check: unavailable (firmware tool not installed).'
    match = re.fullmatch(r'throttled=(0x[0-9a-fA-F]{1,8})\s*', out)
    if rc or not match:
        return 'unknown', '', 'power check: unknown (firmware query failed). Reboot the Pi, then retry the power check.'
    return 'observed', match[1], ''


def reachability():
    lines = []
    for host, port in ENDPOINTS:
        # timeout bounds DNS as well as connect; no HTTP payload or uploaded data.
        rc, out, duration = capture(['timeout', '4', 'bash', '-c',
                                    'exec 3<>/dev/tcp/"$1"/"$2"', 'probe', host, str(port)], timeout=5)
        lines.append(f'{host}:{port} {"open" if rc == 0 else "unreachable"} rc={rc} dur={duration:.3f}s\n{out}')
    return '\n'.join(lines)


def designated_capture():
    """Fail-closed stdlib projection of the installer's literal interface key.

    No YAML evaluator/dependency during bootstrap. Complex/duplicate mappings
    omit wireless diagnostics. A config entry alone cannot claim built-in Wi-Fi:
    the sysfs device ancestry must also establish a USB adapter.
    """
    try:
        source = read_text('/etc/brrdfeeder/config.yaml')
        in_capture, interfaces = False, []
        for line in source.splitlines():
            if line and not line[0].isspace() and not line.startswith('#'):
                in_capture = bool(re.fullmatch(r'capture:\s*(?:#.*)?', line))
            elif in_capture:
                match = re.fullmatch(r'''  interface:\s*(["']?)([A-Za-z0-9_][A-Za-z0-9_.:-]{0,14})\1\s*(?:#.*)?''', line)
                if match:
                    interfaces.append(match[2])
        if len(interfaces) != 1:
            return None
        iface = interfaces[0]
        device = (SYS_NET/iface/'device').resolve(strict=True)
        for parent in [device, *device.parents]:
            try:
                vendor = read_text(parent/'idVendor', 32).strip()
                product = read_text(parent/'idProduct', 32).strip()
            except OSError:
                continue
            if re.fullmatch(r'[0-9a-fA-F]{4}', vendor) and re.fullmatch(r'[0-9a-fA-F]{4}', product):
                return iface
    except (OSError, ValueError):
        pass
    return None


def environment(probe=False):
    result = [f'collected={now()}', f'host={os.uname().nodename}', f'uid={os.geteuid()}']
    commands = [('uname', ['uname', '-a']), ('kernel', ['uname', '-r']), ('arch', ['uname', '-m']),
                ('memory', ['free', '-m']), ('disk', ['df', '-h']),
                ('podman', ['podman', '--version']),
                ('clock', ['timedatectl']), ('usb', ['lsusb'])]
    iface = designated_capture()
    if iface:
        commands += [('capture_address', ['ip', '-brief', 'address', 'show', 'dev', iface]),
                     ('capture_route', ['ip', 'route', 'show', 'dev', iface]),
                     ('capture_wireless', ['iw', 'dev', iface, 'info'])]
    else:
        result.append('capture_wireless=omitted (no verified configured USB capture adapter)')
    for label, path in [('os', '/etc/os-release'), ('uptime_s', '/proc/uptime'),
                        ('board', '/proc/device-tree/model'), ('cpu', '/proc/cpuinfo'),
                        ('dns_resolver', '/etc/resolv.conf')]:
        try:
            # /proc and /sys have virtual regular files; resolv.conf may be a
            # system-managed symlink, so explicitly report it instead of following.
            result.append(f'{label}=\n{read_text(path, 16384).replace(chr(0), "")}')
        except OSError as exc:
            result.append(f'{label}=unavailable: {exc}')
    for label, argv in commands:
        rc, out, duration = capture(argv)
        result.append(f'{label}= rc={rc} dur={duration:.3f}s\n{out}')
    state, value, message = power_check()
    result.append(f'power={state} {value} {message}')
    result.append('reach=\n'+reachability() if probe else 'reach=not probed (offline mode)')
    return '\n'.join(result)


class Log:
    def __init__(self, mode, run_id, override=''):
        self.warned = False
        self.note = ''
        self.mode, self.run_id = mode, run_id
        self.history = deque(maxlen=2000)
        filename = f'{mode}-{dt.datetime.now(dt.timezone.utc):%Y-%m-%dT%H%M%S.%fZ}-{run_id}.log'
        try:
            self.path = Path(override) if override else LOG_DIR/filename
            if not self.path.is_absolute():
                self.path = Path.cwd()/self.path
            self.safe_parent(self.path.parent)
            self.fd = os.open(self.path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o640)
            os.fchmod(self.fd, 0o640)
        except OSError as exc:
            self.fallback(exc)
        if mode != 'dryrun' and self.path.parent == LOG_DIR:
            try:
                latest = LOG_DIR/'install-latest.log'
                if latest.is_symlink():
                    latest.unlink()
                elif latest.exists():
                    raise OSError('latest is not a symlink')
                latest.symlink_to(self.path.name)
            except OSError as exc:
                self.note = f'{now()} [WARN] latest-log link unchanged: {exc}\n'

    @staticmethod
    def safe_parent(parent):
        for part in reversed([parent, *parent.parents]):
            if part.is_symlink():
                raise OSError('symlink log parent refused')
            if part.exists():
                info = part.stat()
                sticky_root = info.st_uid == 0 and info.st_mode & stat.S_ISVTX
                # Ubuntu uses root:syslog 0775 for /var/log. Trust only that
                # named system logging group, at this exact system ancestor;
                # never extend the exception to the product dir or overrides.
                system_log_parent = False
                if part == Path('/var/log') and os.geteuid() == 0 and info.st_uid == 0 and info.st_mode & 0o7777 == 0o775:
                    try:
                        system_log_parent = info.st_gid == grp.getgrnam('syslog').gr_gid
                    except KeyError:
                        pass
                if info.st_uid not in (0, os.geteuid()) or (info.st_mode & 0o022 and not sticky_root and not system_log_parent):
                    raise OSError(f'unsafe log parent {part} (mode {info.st_mode & 0o7777:04o}, uid {info.st_uid}, gid {info.st_gid})')
        parent.mkdir(mode=0o750, parents=True, exist_ok=True)
        if parent == LOG_DIR and os.geteuid() == 0:
            os.chown(parent, 0, 0, follow_symlinks=False)
            parent.chmod(0o750)
            info = parent.stat()
            if (info.st_uid, info.st_gid, info.st_mode & 0o7777) != (0, 0, 0o750):
                raise OSError('product log directory must be 0750 root:root')

    def fallback(self, reason):
        try:
            self.fd, path = tempfile.mkstemp(prefix=f'brrdfeeder-{self.mode}-{self.run_id}-', suffix='.log', dir='/tmp')
            self.path = Path(path)
            os.fchmod(self.fd, 0o640)
        except OSError:
            self.fd = None
            self.path = Path('/tmp/LOG-UNAVAILABLE')
        if not self.warned:
            detail = Redactor().text(str(reason)).replace('\n', ' ')[:500]
            message = f'[brrdfeeder-install] logging degraded to {self.path}: {detail}' + \
                      (' (no writable log file; retain terminal output)' if self.fd is None else '')
            print(message, flush=True)
            if self.fd is not None:
                try:
                    os.write(self.fd, (message+'\n').encode())
                except OSError:
                    pass
            self.warned = True

    def write(self, safe):
        self.history.append(safe)
        if self.fd is None:
            return
        try:
            data = safe.encode('utf-8', 'replace')
            while data:
                data = data[os.write(self.fd, data):]
        except OSError as exc:
            os.close(self.fd)
            self.fallback(exc)
            if self.fd is not None:
                try:
                    os.write(self.fd, ''.join(self.history).encode())
                except OSError:
                    os.close(self.fd)
                    self.fd = None


def bundle(run_id, redactor):
    if power_check()[0] == 'unknown':
        print('WARNING: '+power_check()[2], flush=True)
    members = {}
    manifest = [f'generated={now()}', f'run_id={run_id}', f'host={os.uname().nodename}',
                'redaction=applied before archive writes; no automatic upload',
                'caps=newest 10 install logs; 500 journal/container lines; 48 KiB per member']
    def add(name, raw):
        redactor.nats = False
        redactor.legacy_network = False
        safe = redactor.text(raw).encode('utf-8')
        if len(safe) > 49152:
            safe = safe[-49000:]
            safe = b'[truncated: tail retained]\n' + safe.decode('utf-8', 'replace').encode()
            manifest.append(f'truncated: {name} (48 KiB cap)')
        if '[truncated:' in safe.decode('utf-8', 'replace'):
            manifest.append(f'truncated: {name} (collector cap)')
        remaining = max(0, 1_500_000-sum(map(len, members.values())))
        if name != 'MANIFEST.txt' and len(safe) > remaining:
            safe = safe[:remaining].decode('utf-8','replace').encode()
            manifest.append(f'truncated: {name} (aggregate 1.5 MB cap)')
        members[name] = safe
        manifest.append('included: '+name)
    def file(name, path):
        try:
            text = read_text(path, ends=True)
            if len(text.encode()) >= 131072:
                manifest.append(f'truncated: {path} (read cap)')
            add(name, text)
            return text
        except FileNotFoundError:
            manifest.append('absent: '+str(path))
        except OSError as exc:
            manifest.append(f'unavailable: {path}: {exc}')
        return ''
    def collect(name, args):
        rc, out, duration = capture(args, timeout=8)
        add(name, f'command={shlex.join(args)}\nrc={rc} dur={duration:.3f}s\n'+out)
        if rc:
            manifest.append(f'unavailable: {name} (rc={rc})')
        return rc, out
    log_directory_unreadable = False
    try:
        logs = sorted((p for p in LOG_DIR.iterdir() if p.suffix == '.log' and not p.is_symlink()),
                      key=lambda p: p.stat().st_mtime, reverse=True)
    except FileNotFoundError:
        logs = []
    except OSError as exc:
        logs = []
        log_directory_unreadable = True
        manifest.append(f'unavailable: {LOG_DIR}: {exc}')
    if not logs and not log_directory_unreadable:
        manifest.append('absent: /var/log/brrdfeeder/*.log')
    if len(logs) > 10:
        manifest.append('truncated: install-logs (newest 10 files)')
    last_result = 'unknown (no install RESULT available)'
    uninstalled = False
    for path in logs[:10]:
        text = file('install-logs/'+path.name, path)
        if '\nmode=uninstall\n' in text and '\nresult=OK\n' in text:
            uninstalled = True
        if last_result.startswith('unknown') and '\nmode=install\n' in text:
            matches = re.findall(r'^result=(.*)$', text, re.M)
            steps = re.findall(r'^failed_step=(.*)$', text, re.M)
            if matches:
                last_result = matches[-1] + (' at '+steps[-1] if steps and steps[-1] else '')
                ids = re.findall(r'^run_id=([0-9a-fA-F]{8})$', text, re.M)
                last_result += f' (run {ids[0]})' if ids else ' (run unknown)'
    boots = [p for p in logs if p.name.startswith('bootstrap-')][:10]
    if not boots and not log_directory_unreadable:
        manifest.append('absent: /var/log/brrdfeeder/bootstrap-*.log')
    for path in boots:
        file(path.name, path)
    for path in sorted(Path('/tmp').glob('brrdfeeder-bootstrap-*.log'))[-10:]:
        file(path.name, path)
    add('environment.txt', environment())
    file('config.yaml', '/etc/brrdfeeder/config.yaml')
    file('bluetooth-prior.json', '/etc/brrdfeeder/.bluetooth-prior.json')
    file('gps-startup.json', '/var/lib/brrdfeeder-status/startup.json')
    for path in ['/etc/brrdfeeder/updater.json', '/var/lib/brrdfeeder/release_currency.json',
                 '/var/lib/brrdfeeder/update_outcome.json', '/var/lib/brrdfeeder-updater/state.json',
                 '/var/lib/brrdfeeder-updater/host-state.json', '/var/lib/brrdfeeder-updater/host-transaction.json']:
        file('updater/'+Path(path).name, path)
    sources = ['/etc/containers/systemd/brrdfeeder-engine.container', '/etc/brrdfeeder/brrdhouse.container',
               '/etc/systemd/system/brrdfeeder-updater.path', '/etc/systemd/system/brrdfeeder-updater.service',
               '/etc/systemd/system/brrdfeeder-release-poll.service', '/etc/systemd/system/brrdfeeder-release-poll.timer',
               '/etc/systemd/system/brrdfeeder-release-recover.service', '/etc/systemd/system/brrdfeeder-host-update.service',
               '/etc/systemd/system/brrdfeeder-host-update.timer',
               '/etc/udev/rules.d/99-cybrrd-brrdfeeder.rules']
    for path in sources:
        file('quadlets/'+Path(path).name, path)
    try:
        account = pwd.getpwnam('brrdhouse')
        user_prefix = ['runuser', '-u', 'brrdhouse', '--', 'env',
                       f'XDG_RUNTIME_DIR=/run/user/{account.pw_uid}',
                       f'DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{account.pw_uid}/bus']
        file('quadlets/console-user-link.txt', f'/etc/containers/systemd/users/{account.pw_uid}/brrdhouse.container')
    except KeyError:
        user_prefix = None
        manifest += ['absent: account brrdhouse', 'absent: /etc/containers/systemd/users/<console-uid>/brrdhouse.container']
    states = {}
    for scope, prefix, units in [('system', [], ['brrdfeeder-engine.service','brrdfeeder-updater.path','brrdfeeder-updater.service',
                                  'brrdfeeder-release-poll.service','brrdfeeder-release-poll.timer','brrdfeeder-release-recover.service',
                                  'brrdfeeder-host-update.service','brrdfeeder-host-update.timer']),
                                  ('user', user_prefix, ['brrdhouse.service'])]:
        if prefix is None:
            for item in ['systemd/user', 'podman/user', 'journal/user']:
                manifest.append('unavailable: '+item+' (console account absent)')
            states['brrdhouse.service'] = 'inactive (uninstalled)' if uninstalled else 'inactive (never installed)'
            continue
        flags = ['--user'] if scope == 'user' else []
        for unit in units:
            for verb in ['status','is-enabled','is-active']:
                rc, out = collect(f'systemd/{scope}-{unit}-{verb}.txt', prefix+['systemctl', *flags, verb, unit, '--no-pager'])
                if verb == 'is-active':
                    states[unit] = 'active' if rc == 0 and out.strip() == 'active' else 'inactive/unknown (see systemd record)'
            collect(f'journal/{scope}-{unit}.txt', prefix+['journalctl', *flags, '-u', unit, '-n', '500', '--no-pager'])
        for label, args in [('ps', ['ps','-a']), ('images', ['images','--digests'])]:
            collect(f'podman/{scope}-{label}.txt', prefix+['podman', *args])
    if not Path(sources[0]).exists() and states.get('brrdfeeder-engine.service') != 'active':
        states['brrdfeeder-engine.service'] = 'inactive (never installed or uninstalled)'
    collect('engine-log.txt', ['podman','logs','--tail','500','brrdfeeder-engine'])
    add('reach.txt', reachability())
    collect('clock.txt', ['timedatectl'])
    collect('clock-utc.txt', ['date', '-u'])
    summary = f'engine: {states.get("brrdfeeder-engine.service", "unknown")}; console: {states.get("brrdhouse.service", "unknown")}; last install: {last_result}'
    manifest.append('state='+summary)
    add('MANIFEST.txt', '\n'.join(manifest)+'\n')
    # Archive entirely in memory; only already-redacted, size-capped members enter.
    data = io.BytesIO()
    with tarfile.open(fileobj=data, mode='w:gz') as tar:
        for name, content in members.items():
            info = tarfile.TarInfo('bundle/'+name)
            info.size, info.mode = len(content), 0o640
            tar.addfile(info, io.BytesIO(content))
    if len(data.getvalue()) >= 2_000_000:
        raise RuntimeError('support bundle exceeds 2 MB; no archive written')
    fd, path = tempfile.mkstemp(prefix=f'brrdfeeder-support-{dt.datetime.now(dt.timezone.utc):%Y%m%dT%H%M%SZ}-{run_id}-', suffix='.tar.gz', dir='/tmp')
    with os.fdopen(fd, 'wb') as out:
        out.write(data.getvalue())
        # A sudo-created 0600 archive must be readable by the customer who must
        # email it. Change only this newly-created FD, never a caller-named path.
        if os.geteuid() == 0 and os.environ.get('SUDO_USER'):
            try:
                owner = pwd.getpwnam(os.environ['SUDO_USER'])
                if owner.pw_uid >= 100 and str(owner.pw_uid) == os.environ.get('SUDO_UID'):
                    os.fchown(out.fileno(), owner.pw_uid, owner.pw_gid)
            except (KeyError, OSError):
                pass  # direct-root or unmapped caller retains root ownership
    print(f'Support bundle: {path} ({len(data.getvalue())} bytes); {summary}; bundle run {run_id}', flush=True)
    return 0


SYSTEMS = ('GPS', 'Network (NATS)', 'Wi-Fi capture', 'Bluetooth')
STARTUP_ADVICE = {
    'GPS': 'Check the GPS connection and give its antenna a clear view of the sky.',
    'Network (NATS)': 'Check the network and clock; inspect the console for connection details.',
    'Wi-Fi capture': 'Check the USB capture adapter and inspect the console.',
    'Bluetooth': 'Check the Bluetooth adapter and inspect the console.',
}


def engine_snapshot(context, deadline):
    """Fresh engine status plus BLE lifecycle from ONLY the current invocation."""
    result = {'running': False, 'states': {k: 'still starting' for k in SYSTEMS}, 'failure': None}
    def query(argv):
        remaining = deadline-time.monotonic()
        if remaining <= 0:
            return 124, ''
        rc, out, _ = capture(argv, timeout=min(2, remaining))
        return rc, out
    args = ['systemctl', 'show', 'brrdfeeder-engine.service',
            '--property=ActiveState,SubState,InvocationID']
    rc, text = query(args)
    unit = dict(line.split('=', 1) for line in text.splitlines() if '=' in line) if rc == 0 else {}
    if unit.get('ActiveState') == 'failed':
        result['failure'] = 'Engine service failed. Inspect sudo systemctl status brrdfeeder-engine.service.'
        return result
    result['running'] = unit.get('ActiveState') == 'active'
    result['gps_waiting'] = unit.get('ActiveState') == 'activating' and unit.get('SubState') == 'start-pre'
    if result['gps_waiting']:
        try:
            sidecar = json.loads(read_text('/var/lib/brrdfeeder-status/startup.json'))
            stamp = dt.datetime.fromisoformat(sidecar['written_at'].replace('Z', '+00:00')).timestamp()
            result['gps_waiting'] = (sidecar.get('state') in ('gps-waiting', 'gps-missing', 'gps-busy', 'gps-fix')
                                     and stamp >= context['started_at'] and 0 <= time.time()-stamp <= 90)
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            result['gps_waiting'] = False
    invocation = unit.get('InvocationID', '')
    if not result['running'] or not re.fullmatch('[0-9a-f]{32}', invocation):
        return result
    rc, journal = query(['journalctl', '--no-pager', '-o', 'cat', '-n', '200', '_SYSTEMD_INVOCATION_ID='+invocation])
    try:
        status = json.loads(read_text('/var/lib/brrdfeeder-status/status.json'))
        written = dt.datetime.fromisoformat(status['written_at'].replace('Z', '+00:00'))
        interval = status['status_interval_secs']
        fresh = (status.get('schema_version') == 1 and type(interval) is int and 0 < interval <= 3600
                 and written.tzinfo is not None and written.timestamp() >= context['started_at']
                 and 0 <= time.time()-written.timestamp() <= 3*interval
                 and status['heartbeat']['node_id'] == context['node_id'])
        if fresh:
            heartbeat = status['heartbeat']
            if heartbeat.get('gps', {}).get('state') == 'healthy':
                result['states']['GPS'] = 'OK'
            if status.get('links', {}).get('nats_state', '').lower() == 'connected':
                result['states']['Network (NATS)'] = 'OK'
            if heartbeat.get('radio_status') == 'up' and any(
                    item.get('monitor_mode') is True for item in status.get('inventory', {}).get('capture', [])):
                result['states']['Wi-Fi capture'] = 'OK'
            # Main's status schema has only historical rfkill, not BLE health.
            # Never use that pre-unblock snapshot as a current health verdict.
            ble = None
            if rc == 0:
                for line in journal.splitlines():
                    match = re.search(r'\[rid_ble\] state=(Initializing|Healthy|Degraded|Failed)\b', line)
                    if match:
                        ble = match[1]
                    if line.strip() == '[rid_ble] disabled':
                        ble = 'Disabled'
                if ble == 'Healthy': result['states']['Bluetooth'] = 'OK'
                elif ble == 'Disabled': result['states']['Bluetooth'] = 'disabled in configuration'
                elif ble == 'Failed': result['states']['Bluetooth'] = 'needs attention'
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        pass
    rc, after = query(args)
    if rc or after != text:
        # A concurrent restart invalidates every result, not just BLE's journal.
        result = {'running': False, 'states': {k: 'still starting' for k in SYSTEMS}, 'failure': None}
    return result


def wait_engine(context, progress, timeout=60):
    deadline = time.monotonic()+timeout
    progress.start(f'Waiting for engine startup (up to {timeout:g} seconds)')
    result = {'running': False, 'states': {k: 'still starting' for k in SYSTEMS}, 'failure': None}
    while time.monotonic() < deadline:
        result = engine_snapshot(context, deadline)
        if result['failure'] or (result['running'] and all(v == 'OK' for v in result['states'].values())):
            return result
        time.sleep(min(1, max(0, deadline-time.monotonic())))
    if not result['running'] and not result.get('gps_waiting'):
        result['failure'] = 'Engine did not reach a running or verified GPS-waiting state within the startup deadline.'
    return result


# Canonical user-facing update status. Switch only when automatic updates ship.
SELF_UPDATE_STATUS = ('Self-Update is installed but not yet active — this version does not update itself. '
                      'To move to a newer release today: sudo brrdfeeder uninstall, then run the install command again '
                      '(you will link the sensor to your account again).')


def final_install_screen(context, readiness, log_path, run_id):
    title = ('BRRDfeeder is installed and running.' if readiness['running'] else
             'BRRDfeeder is installed. Engine startup is still waiting for GPS.')
    return '\n'.join([title, 'Node ID: '+context['node_id'], 'Console: '+context['console_url'],
                      *(key+': '+readiness['states'][key] for key in SYSTEMS),
                      SELF_UPDATE_STATUS,
                      'Status: sudo brrdfeeder status', 'Support: sudo brrdfeeder support-bundle',
                      'Uninstall: sudo brrdfeeder uninstall', f'Log: {log_path}  (run {run_id})'])


class Progress:
    """Terminal-only phase feedback, including while environment probes block.

    One worker owns the heartbeat; the lock serializes phase/notice output.
    Closing it before RESULT prevents a heartbeat after the final run-ID line.
    """
    def __init__(self):
        self.lock = threading.RLock()
        self.stop = threading.Event()
        self.label = None
        self.paused = False
        self.last = self.started = time.monotonic()
        self.worker = threading.Thread(target=self._heartbeat, daemon=True)
        self.worker.start()

    def emit(self, text):
        with self.lock:
            print(text, flush=True)
            self.last = time.monotonic()

    def start(self, label):
        with self.lock:
            self.finish()
            self.label = label
            self.paused = False
            self.started = time.monotonic()
            self.emit(label+' …')

    def finish(self, rc=0):
        with self.lock:
            if self.label:
                self.emit(self.label+(' — failed' if rc else ' — done'))
                self.label = None

    def notice(self, text):
        with self.lock:
            self.paused = text.startswith('Remove BRRDfeeder from this Pi?')
            self.emit(text)

    def _heartbeat(self):
        while not self.stop.wait(.25):
            with self.lock:
                if self.label and not self.paused and time.monotonic()-self.last >= 4:
                    self.emit(f'{self.label} … still working ({int(time.monotonic()-self.started)} s elapsed)')

    def close(self):
        self.stop.set()
        self.worker.join()


def supervise(script, args):
    # Establish visibility BEFORE setsid detaches the child. stdin can be a pipe
    # (curl | bash); Device Flow only needs a display, never keyboard input.
    with contextlib.ExitStack() as stack:
        display_fd = None
        tty_fd = None
        try:
            confirmation = os.open('/dev/tty', os.O_RDWR | os.O_NOCTTY)
            stack.callback(os.close, confirmation)
            if os.isatty(confirmation): tty_fd = confirmation
        except OSError:
            pass
        if sys.stdout.isatty():
            # fd 1 itself will become the child's capture pipe; preserve a dup.
            display_fd = os.dup(sys.stdout.fileno())
            stack.callback(os.close, display_fd)
        else:
            try:
                terminal = stack.enter_context(open('/dev/tty', 'w'))
                if terminal.isatty():
                    display_fd = terminal.fileno()
                    stack.enter_context(contextlib.redirect_stdout(terminal))
            except OSError:
                pass  # No TTY: link and code remain visible on stdout.
        progress = Progress()
        stack.callback(progress.close)
        return _supervise(script, args, display_fd, tty_fd, progress)


def _supervise(script, args, display_fd, tty_fd, progress):
    power_check.cache_clear()
    run_id = os.environ.get('BRRDFEEDER_RUN_ID', '')
    if not re.fullmatch('[0-9a-fA-F]{8}', run_id):
        run_id = secrets.token_hex(4)
    print(f'[brrdfeeder-install] run {run_id}', flush=True)
    redactor = Redactor()
    try:
        redactor.remember('nats-creds', read_text(CREDS, 65536).strip())
    except OSError:
        pass
    if args == ['--support-bundle']:
        return bundle(run_id, redactor)
    mode = 'dryrun' if '--dry-run' in args else 'uninstall' if '--uninstall' in args else 'verify' if '--verify' in args or '--status' in args else 'install'
    # Ordinary removal is compact even when invoked through the local command
    # without bootstrap flags. Dry-run still displays its full validated plan.
    quiet = mode == 'uninstall' or ('--no-verbose' in args and mode == 'install')
    progress.start('Installing BRRDfeeder (usually about 2–5 min; downloads can take longer)' if mode == 'install'
                   else 'Preparing '+('removal' if '--uninstall' in args else mode)+' and collecting diagnostics')
    override = next((a.split('=', 1)[1] for a in args if a.startswith('--audit-log=')), '')
    if override and any(Path(os.path.abspath(override)).is_relative_to(root) for root in
                        ['/etc/brrdfeeder', '/var/lib/brrdfeeder', '/var/lib/brrdhouse', '/var/lib/brrdfeeder-status', '/run/brrdfeeder-identity']):
        # Logging must never create/retain managed package state during teardown.
        print('[brrdfeeder-install] --audit-log is inside managed state; using default log directory.', flush=True)
        override = ''
    log = Log(mode, run_id, override)
    started = time.monotonic()
    bootstrap = os.environ.get('BRRDFEEDER_BOOTSTRAP_LOG', '')
    boot_hash = 'unknown (direct invocation or bootstrap hash unavailable)'
    if bootstrap:
        try:
            match = re.search(r'^bootstrap_sha256=(.*)$', read_text(bootstrap), re.M)
            if match:
                boot_hash = match[1]
        except OSError:
            pass
    header = f'==== BRRDFEEDER INSTALL LOG ====\nrun_id={run_id}\nmode={mode}\nstarted={now()}\ninstaller_sha256={hashlib.sha256(Path(script).read_bytes()).hexdigest()}\nbootstrap_sha256={boot_hash}\nbootstrap_log={bootstrap or "absent (direct invocation)"}\nargv={shlex.join(args)}\ninvoked_by_uid={os.geteuid()} sudo_user={os.environ.get("SUDO_USER", "")}\n'
    log.write(redactor.text(header))
    log.write('==== ENVIRONMENT ====\n'+redactor.text(environment(probe=mode == 'install'))+'\n')
    power_state, power_value, power_message = power_check()
    if mode != 'install' and power_state == 'unknown':
        progress.notice('WARNING: '+power_message)
    log.write('==== BOOTSTRAP ====\n'+redactor.text(os.environ.get('BRRDFEEDER_BOOTSTRAP_NOTES', 'absent (direct invocation)'))+'\n==== TRANSCRIPT ====\n')
    if log.note:
        log.write(redactor.text(log.note))
    nonce = secrets.token_hex(16)
    phase, step = ('uninstall', 'uninstall/validate') if mode == 'uninstall' else ('pre-flight', 'pre-flight/arguments')
    command = None
    last_output = deque(maxlen=40)
    command_output = None
    failure = None
    install_context = None
    env = {**os.environ, 'BRRDFEEDER_LOG_CHILD': nonce, 'BRRDFEEDER_RUN_ID': run_id,
           'BRRDFEEDER_LOG_TOKEN': nonce, 'PYTHONDONTWRITEBYTECODE': '1',
           'BRRDFEEDER_DISPLAY_FD': str(display_fd) if display_fd is not None else '',
           'BRRDFEEDER_TTY_FD': str(tty_fd) if tty_fd is not None else ''}
    env.update(BRRDFEEDER_POWER_STATE=power_state, BRRDFEEDER_POWER_VALUE=power_value,
               BRRDFEEDER_POWER_MESSAGE=power_message)
    p = subprocess.Popen(['bash', script, *args], stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                         env=env, start_new_session=True,
                         pass_fds=tuple(fd for fd in (display_fd,tty_fd) if fd is not None))
    def forward(signum, _frame):
        try:
            os.killpg(p.pid, signum)
        except ProcessLookupError:
            pass
    old_signals = {sig: signal.signal(sig, forward) for sig in (signal.SIGINT, signal.SIGTERM)}

    def finish_command(rc):
        nonlocal command, command_output, failure, step
        if command:
            duration = time.monotonic()-command[1]
            log.write(f'{now()} [CMD]   step={step} {command[0]} rc={rc} dur={duration:.3f}s\n')
            command_output.seek(0)
            for line in command_output:
                log.write('    '+line)
            command_output.close()
            if rc:
                failure = (rc, phase, step, list(last_output))
            else:
                failure = None
                step = phase+'/check'
            command = None
            command_output = None

    # Do not use readline(): a hostile/no-newline output must not grow memory
    # without bound. Overlong lines are omitted from the log, not cut mid-secret.
    pending = b''
    oversized = False
    def line(raw):
        nonlocal phase, step, command, command_output, failure, install_context
        text = raw.decode('utf-8', 'replace').rstrip('\n')
        prefix = '\x1e'+nonce+'\t'
        if prefix in text and not text.startswith(prefix):
            before, event = text.split(prefix, 1)
            line(before.encode())
            line((prefix+event).encode())
            return
        if text.startswith(prefix):
            kind, _, payload = text[len(prefix):].partition('\t')
            if kind == 'PHASE':
                failure = None
                phase, _, label = payload.partition('\t')
                step = phase+'/check'
                log.write(f'{now()} [PHASE] {phase} {redactor.text(label)}\n')
                progress.start(redactor.text(label or phase))
            elif kind == 'BEGIN':
                failure = None
                step, _, display = payload.partition('\t')
                phase = step.split('/')[0]
                last_output.clear()
                command = (redactor.text(display), time.monotonic())
                # Only REDACTED text may spill to the unnamed temporary file.
                command_output = tempfile.SpooledTemporaryFile(max_size=262144, mode='w+t', encoding='utf-8')
            elif kind == 'END':
                finish_command(int(payload))
            elif kind == 'SECRET':
                secret_kind, _, encoded = payload.partition('\t')
                redactor.remember(secret_kind, base64.b64decode(encoded).decode('utf-8', 'replace'))
            elif kind == 'NOTICE':
                safe = redactor.text(payload)
                progress.notice(safe)
                log.write(f'{now()} [INFO]  {safe}\n')
            elif kind == 'DETAIL':
                detail = base64.b64decode(payload).decode('utf-8', 'replace')
                log.write(f'{now()} [DETAIL] {redactor.text(detail)}\n')
            elif kind == 'UPDATE_STATUS':
                progress.notice(SELF_UPDATE_STATUS)
                log.write(f'{now()} [INFO]  {SELF_UPDATE_STATUS}\n')
            elif kind == 'INSTALL_CONTEXT' and mode == 'install':
                try:
                    value = json.loads(payload)
                    if (not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', value['node_id'])
                            or not re.fullmatch(r'http://[0-9.]+:[0-9]+/', value['console_url'])
                            or type(value['started_at']) is not int):
                        raise ValueError('invalid context')
                    install_context = value
                except (ValueError, TypeError, KeyError):
                    install_context = {'invalid': True}
            return
        safe = redactor.text(text)
        if not quiet or phase in ('enrollment', 'complete'):
            progress.emit(text)
        elif 'REFUSED:' in safe or 'FATAL' in safe or 'Unknown flag:' in safe or ' !! ' in safe or safe.startswith('[bluetooth] RID.BLE '):
            progress.emit(safe)
        if failure and 'FATAL' not in safe and 'REFUSED:' not in safe:
            # Recovery/continuation means that earlier nonzero status is not
            # the cause of a later, independent validation failure.
            failure = None
            step = phase+'/check'
        last_output.append(safe)
        if command:
            try:
                command_output.write(safe+'\n')
            except OSError:
                # A full/read-only temp filesystem must not kill the installer.
                # Continue in RAM, explicitly recording lost buffered history.
                try:
                    command_output.close()
                except OSError:
                    pass
                command_output = io.StringIO()
                command_output.write('[logging degraded: earlier command buffer unavailable]\n'+safe+'\n')
                print('[brrdfeeder-install] command-log buffer degraded; continuing in memory.', flush=True)
        else:
            token = '[WARN]' if ' !! ' in safe else '[OK]' if ' OK ' in safe else '[INFO]'
            # The single structured FAIL is emitted immediately before RESULT;
            # retain the original diagnostic message too, without losing output.
            log.write(f'{now()} {token:<7} {safe}\n')
    while True:
        chunk = os.read(p.stdout.fileno(), 4096)
        if not chunk:
            break
        pending += chunk
        while b'\n' in pending:
            raw, pending = pending.split(b'\n', 1)
            if oversized:
                if not quiet: print(raw.decode('utf-8', 'replace'), flush=True)
                line(b'[omitted: overlong output line]')
                oversized = False
            else:
                line(raw)
        if len(pending) > 131072:
            if not quiet: print(pending.decode('utf-8', 'replace'), end='', flush=True)
            pending = b''
            oversized = True
    if pending:
        line(b'[omitted: overlong output line]' if oversized else pending)
    rc = p.wait()
    if rc < 0:
        rc = 128-rc
    for sig, handler in old_signals.items():
        signal.signal(sig, handler)
    p.stdout.close()
    finish_command(rc)
    readiness = None
    if rc == 0 and mode == 'install' and install_context is not None:
        readiness = ({'failure': 'Cannot verify the final node identity/address.'}
                     if install_context.get('invalid') else wait_engine(install_context, progress))
        if readiness['failure']:
            rc = 1
            phase, step = 'verification', 'verification/engine-startup'
            failure = None
            progress.notice('ERROR: '+readiness['failure'])
            last_output.append(readiness['failure'])
        else:
            for name, state in readiness['states'].items():
                if state != 'OK':
                    notice = f'WARNING: {name}: {state} after the startup wait. '+STARTUP_ADVICE[name]
                    progress.notice(notice)
                    log.write(f'{now()} [WARN] {notice}\n')
    progress.finish(rc)
    progress.close()
    if rc and failure:
        result_rc, phase, step, last_output = failure
    else:
        result_rc = rc
    if rc:
        log.write(f'{now()} [FAIL]  step={step} rc={result_rc}\n')
    log.write('==== RESULT ====\n'+f'result={"FAILED" if rc else "OK"}\nfailed_phase={phase if rc else ""}\nfailed_step={step if rc else ""}\nexit_code={result_rc}\nduration_s={time.monotonic()-started:.3f}\nlast_output=\n')
    if rc:
        for safe in last_output:
            log.write('    '+safe+'\n')
    support_command = ('sudo brrdfeeder support-bundle' if Path('/usr/local/sbin/brrdfeeder').is_file()
                       else 'curl -fsSL https://get.cybrrd.com | bash -s support-bundle')
    log.write(f'log={log.path}\nsupport_bundle_cmd={support_command}\n')
    if rc == 0 and readiness is not None:
        log.write(final_install_screen(install_context, readiness, log.path, run_id)+'\n')
    if log.fd is not None:
        os.close(log.fd)
    if rc:
        if mode == 'install':
            print('BRRDfeeder installation failed.', flush=True)
        print(f'[brrdfeeder-install] FAILED at {step}  (run {run_id})\n[brrdfeeder-install] Log: {log.path}\n[brrdfeeder-install] For support, run:  {support_command}\n[brrdfeeder-install]   then email the file it names. (run {run_id})', flush=True)
    elif readiness is not None:
        print(final_install_screen(install_context, readiness, log.path, run_id), flush=True)
    else:
        print(f'Log: {log.path}  (run {run_id})', flush=True)
    return result_rc if result_rc >= 0 else 128-result_rc


if __name__ == '__main__':
    sys.exit(supervise(sys.argv[1], sys.argv[2:]))
INSTALL_LOG_PY
)" "$0" "$@"
fi

# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# shellcheck shell=bash
# Embedded before flag parsing; private records are consumed by install-log.py.
# These are action-level commands. Read-only predicates/captured queries remain
# ordinary shell code; their visible output is still captured by the supervisor.
LOG_PHASE=pre-flight
log_event() {
  local payload=$2
  # A diagnostic may contain newlines. Keep the entire record log-only.
  if [[ $1 == DETAIL ]]; then payload=$(printf '%s' "$payload" | base64 -w0); fi
  printf '\036%s\t%s\t%s\n' "$BRRDFEEDER_LOG_TOKEN" "$1" "$payload"
}
log_secret() {
  local encoded
  encoded=$(printf '%s' "$2" | base64 -w0)
  log_event SECRET "$1"$'\t'"$encoded"
}
run_step() {
  local step=$1 display rc
  shift
  printf -v display '%q ' "$@"
  log_event BEGIN "$step"$'\t'"$display"
  if "$@"; then rc=0; else rc=$?; fi
  # The supervisor recognizes this nonce even after output without a final LF.
  log_event END "$rc"
  return "$rc"
}
export -f log_event log_secret run_step
export BRRDFEEDER_LOG_TOKEN LOG_PHASE


# ----------------------------------------------------------------------
# Constants
# ----------------------------------------------------------------------
readonly TARGET_USER="brrdfeeder"
readonly SERVICE_HOME="/var/lib/brrdfeeder"  # useradd input; existing homes come from passwd

# Vendor-agnostic udev symlink names — downstream Quadlet AddDevice=
# directives reference these. Vendor swap = update this file's
# ATTRS{} only; symlinks (and Quadlet, and engine) stay stable.
readonly GPS_SYMLINK="cybrrd_gps"
readonly BLE_SYMLINK="cybrrd_ble"

# Default USB vendor:product IDs (test-node-2/test-node-1 baseline)
readonly UBLOX_VENDOR="1546"
readonly -a UBLOX_PRODUCTS=(01a5 01a6 01a7 01a8 01a9)
readonly NORDIC_VENDOR="1915"
readonly NORDIC_PRODUCT="c00a"

# Canonical host config tree (root-owned; product-grade — config does NOT
# live in a user homedir). Migrated automatically from the legacy
# service account's legacy brrdfeeder/ location if found there.
readonly ETC_DIR="/etc/brrdfeeder"
readonly SECRETS_DIR="${ETC_DIR}/secrets"
readonly CONFIG_PATH="${ETC_DIR}/config.yaml"
readonly CREDS_PATH="${SECRETS_DIR}/brrdfeeder.creds"

# Canonical system paths
readonly UDEV_RULES_FILE="/etc/udev/rules.d/99-cybrrd-brrdfeeder.rules"
# Journald drop-in: current name is 99-brrdfeeder.conf. Installs made before
# the 2026-09-25 naming change wrote 99-brrdfeeder-open.conf; that legacy file
# is still recognized (verified, migrated on re-run, removed on uninstall) by
# its own marker line, and a file carrying neither marker is never ours.
readonly JOURNALD_DROPIN="/etc/systemd/journald.conf.d/99-brrdfeeder.conf"
readonly JOURNALD_DROPIN_LEGACY="/etc/systemd/journald.conf.d/99-brrdfeeder-open.conf"
readonly JOURNALD_DROPIN_MARKER="# BRRDfeeder — protect MicroSD card from journald write wear."
readonly JOURNALD_DROPIN_LEGACY_MARKER="BRRDfeeder Open tier"
readonly CHRONY_DROPIN="/etc/chrony/conf.d/10-brrdfeeder.conf"
readonly LEGACY_CHRONY_DROPIN="/etc/chrony/conf.d/10-pack.conf"  # Recognition only; never generated.
readonly QUADLET_FILE="/etc/containers/systemd/brrdfeeder-engine.container"
readonly IMAGE_REPOSITORY="ghcr.io/cybrrd/brrdfeeder"
readonly IDENTITY_INSTALL="/usr/local/libexec/brrdfeeder-image-identity"
readonly CONSOLE_USER="brrdhouse"
readonly CONSOLE_HOME="/var/lib/brrdhouse"
readonly CONSOLE_REPOSITORY="ghcr.io/cybrrd/brrdhouse"
readonly CONSOLE_QUADLET_FILE="/etc/brrdfeeder/brrdhouse.container"
readonly STATUS_DIR="/var/lib/brrdfeeder-status"
readonly STATUS_PROVISIONER="/usr/local/libexec/brrdfeeder-provision-status"
readonly GPS_SEED="/usr/local/libexec/brrdfeeder-gps-seed"
readonly BLUETOOTH_HELPER="/usr/local/libexec/brrdfeeder-bluetooth"

# Engine-visible outcome/currency and legacy watermark surface.
# Signed pending releases live only in root-private /var/lib/brrdfeeder-updater.
readonly STATE_DIR="/var/lib/brrdfeeder"
# Standalone ARM64 host executable: independently reproduced on worldport with
# the pinned Go toolchain. Publication is a separate operator step, never tags.
readonly RELEASE_HELPER_SHA256="6b5bc93381cc15b5e58fa997be226339ffc7762d0b4109ea6e0b33dd0a34e3c5"
readonly RELEASE_HELPER_URL="https://get.cybrrd.com/releases/v1/updater/${RELEASE_HELPER_SHA256}/linux-arm64/brrdfeeder-release"

# Zitadel OAuth Device Flow (BRRDfeeder-tier enrollment — binds feeder to user account)
readonly OAUTH_ISSUER="https://hospitality.cybrrd.com"
readonly OAUTH_CLIENT_ID="376855240068104243"   # cyBRRD Platform / brrdfeeder-open-tier (Native, PKCE, Device Code grant)
readonly OAUTH_SCOPE="openid profile offline_access"
# flock v0 enrollment endpoint — exchanges the Zitadel access_token for
# scoped NATS .creds (Option A: control-plane exchange; data plane stays
# pure decentralized NATS Ed25519 — see esp32-reference-architecture.md
# Ruling 3/4 and the Oracle Bottleneck failure-domain argument).
readonly FLOCK_ENROLL_URL="https://ingest.cybrrd.com/v1/enroll/oauth"
readonly REFRESH_TOKEN_PATH="${SECRETS_DIR}/oauth_refresh.token"

# ----------------------------------------------------------------------
# Parse flags
# ----------------------------------------------------------------------
# Derivation requiring root runs only inside this verified, logged installer.
# Direct invocations retain their explicit-parameter contract.
BOOT_PREPARE=${BRRDFEEDER_BOOTSTRAP_PREPARE:-0}
for BOOT_ARG in "$@"; do
  case "$BOOT_ARG" in --uninstall|--status|--support-bundle|--verify|--dry-run) BOOT_PREPARE=0;; esac
done
if [[ $BOOT_PREPARE == 1 ]]; then
  eval "$(cat <<'BOOTSTRAP_PREPARE_EOF'
#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Embedded in the verified installer; executed only for bootstrap install mode.
# Root-only preparation belongs after checksum verification and inside logging.
bootstrap_say() { log_event NOTICE "$*"; }
bootstrap_die() { bootstrap_say "FATAL: $*"; exit 1; }
bootstrap_note() { bootstrap_say "$*"; export BRRDFEEDER_BOOTSTRAP_NOTES="${BRRDFEEDER_BOOTSTRAP_NOTES:-}; $*"; }
[[ $EUID == 0 ]] || bootstrap_die "Root preparation requires sudo; use curl -fsSL https://get.cybrrd.com | bash"
arch="$(uname -m)"
case "$arch" in
  aarch64|arm64) ;;
  *) bootstrap_die "this release is built for arm64 (Raspberry Pi 4/5). This machine is $arch.
       Installing the wrong architecture is a slower failure than refusing now." ;;
esac

for tool in curl sha256sum ip; do
  command -v "$tool" >/dev/null || bootstrap_die "$tool is required but not installed."
done

bootstrap_say "architecture : $arch"
bootstrap_say "engine/console pins: supplied by checksum-verified bootstrap"

# ── Console listen address ──────────────────────────────────────────────────
# The installer REQUIRES --console-listen as a literal IP:port and refuses a
# hostname, a wildcard or a shell expression. That is deliberate and correct:
# binding the status console to 0.0.0.0 would expose it beyond the owner's LAN,
# and this page is unauthenticated by design because it serves a home or
# small-business network.
#
# But a customer should not have to find their own LAN address to install
# software. So derive it — and derive it CONSERVATIVELY, refusing anything that
# would put the console somewhere surprising. An auto-detected listen address is
# only defensible if it fails rather than guesses.
#
# `ip route get 1.1.1.1` yields the source address of the interface carrying the
# default route. That is structurally the right answer: it cannot select the
# Wi-Fi capture adapter (a monitor-mode interface has no IP at all), and on our
# own sensors it correctly returns the LAN address rather than the tailnet one.
CONSOLE_PORT=8080

listen_supplied=0
for arg in "$@"; do
  case "$arg" in --console-listen|--console-listen=*) listen_supplied=1 ;; esac
done

if [ "$listen_supplied" -eq 1 ]; then
  bootstrap_note "console addr : supplied by operator"
else
  route_line="$(ip -4 route get 1.1.1.1 2>/dev/null | head -1)"
  # Field-walk rather than a regex. `sed 's/.*dev \(...\)/'` is greedy and matches
  # inside an interface name containing "dev" — caught in testing with a fixture
  # device named `testdev`, which parsed as "src". Real names are fine, but a latent
  # trap in an installer is still a trap.
  lan_ip="$(printf '%s\n' "$route_line" | awk '{for(i=1;i<NF;i++) if($i=="src"){print $(i+1); exit}}')"
  lan_dev="$(printf '%s\n' "$route_line" | awk '{for(i=1;i<NF;i++) if($i=="dev"){print $(i+1); exit}}')"

  [ -n "$lan_ip" ] || bootstrap_die "could not determine this machine's LAN address.
       The console needs a literal address to listen on. Re-run supplying it:
         curl -fsSL https://get.cybrrd.com | bash -s -- --console-listen <your-LAN-IP>:$CONSOLE_PORT"

  # Refuse addresses that would put the console somewhere the owner does not expect.
  o1="${lan_ip%%.*}"; rest="${lan_ip#*.}"; o2="${rest%%.*}"
  reason=""
  case "$lan_ip" in
    127.*)     reason="loopback — the console would be unreachable from any other device on your network" ;;
    169.254.*) reason="link-local — this machine did not get a DHCP lease; fix networking first" ;;
    0.0.0.0)   reason="wildcard — refusing to expose the console on every interface" ;;
  esac
  # 100.64.0.0/10 — CGNAT, which is also the Tailscale range. A console bound
  # there is reachable over the overlay and NOT from the owner's own LAN: the
  # opposite of what this page is for, and a surprise nobody would debug quickly.
  if [ -z "$reason" ] && [ "$o1" = "100" ] && [ "$o2" -ge 64 ] 2>/dev/null && [ "$o2" -le 127 ] 2>/dev/null; then
    reason="a CGNAT/Tailscale address — the console would be reachable over the overlay but NOT from your own LAN"
  fi
  [ -z "$reason" ] || bootstrap_die "declining to auto-select $lan_ip: $reason.
       Supply the address you want explicitly:
         curl -fsSL https://get.cybrrd.com | bash -s -- --console-listen <your-LAN-IP>:$CONSOLE_PORT"

  bootstrap_note "console addr : $lan_ip:$CONSOLE_PORT  (auto-detected on ${lan_dev:-?})"
  set -- --console-listen "$lan_ip:$CONSOLE_PORT" "$@"
fi

# ── Fresh install, abandoned template, or rerun? ────────────────────────────
# The installer has three states and treats first-install flags differently in
# each. Getting this wrong fatals on the installer's own line 340 ("Existing
# config is preserved; omit first-install --interface/--latitude/--longitude").
#
#   no config.yaml            fresh install  -> derive interface/listener; GPS seeds position
#   config.yaml with EDIT-ME- abandoned run  -> it is the installer's OWN unedited
#                                               template (the installer itself gates
#                                               on that token); remove it so the
#                                               installer regenerates it WITH the
#                                               flags, then proceed as fresh
#   config.yaml, no EDIT-ME-  real config    -> a rerun; pass NO first-install flags
#
# The second state is exactly what a customer hits after a first attempt stops
# early — as brrdg3s2 did on 2026-09-22 — and without this they would be told to
# hand-edit a YAML file, which is the thing the one-liner exists to prevent.
install_mode=fresh
if [ -e "$CONFIG_PATH" ]; then
  if grep -qE 'EDIT-ME-[A-Za-z]' "$CONFIG_PATH" 2>/dev/null; then
    bootstrap_note "config       : abandoned template from an earlier run — removing so this run completes it"
    [[ ! -L $CONFIG_PATH && ! -L /etc/brrdfeeder ]] || bootstrap_die "Refusing symlinked config template"
    run_step bootstrap/recover-template rm -f "$CONFIG_PATH"
    install_mode=fresh
  else
    bootstrap_note "config       : existing (rerun) — first-install parameters will not be passed"
    install_mode=rerun
  fi
fi

# ── Capture interface ───────────────────────────────────────────────────────
# The installer needs --interface on a fresh install. A customer cannot reasonably
# be expected to know their adapter is "wlan1". But it IS deterministic: Remote ID
# capture requires monitor mode, and on a Raspberry Pi exactly one adapter can do
# it — the external one. The Pi's built-in Wi-Fi (brcmfmac) cannot. So: find the
# phys that support monitor mode. Exactly one is the answer; zero means the
# external adapter is not plugged in, which is the single most common way a new
# install will fail and deserves a plain message rather than a cryptic one later.
iface_supplied=0; lat_supplied=0; lon_supplied=0
if [ "$install_mode" = "rerun" ]; then
  # A rerun must not carry first-install flags (installer line 340). If the
  # operator passed any, that is their explicit choice and the installer will
  # say so; the bootstrap adds none of its own.
  iface_supplied=1
fi
for arg in "$@"; do
  case "$arg" in
    --interface|--interface=*) iface_supplied=1 ;;
    --latitude|--latitude=*)   lat_supplied=1 ;;
    --longitude|--longitude=*) lon_supplied=1 ;;
  esac
done

if [ "$install_mode" = "rerun" ]; then
  bootstrap_note "capture iface: existing configuration preserved"
elif [ "$iface_supplied" -eq 1 ]; then
  bootstrap_note "capture iface: supplied by operator"
else
  if ! command -v iw >/dev/null; then
    bootstrap_say "installing iw (needed to identify the capture adapter)…"
    run_step bootstrap/install-iw apt-get install -y --no-install-recommends iw \
      || bootstrap_die "could not install iw. Supply the capture interface explicitly:
         curl -fsSL https://get.cybrrd.com | bash -s -- --interface <iface> ..."
  fi
  mon_ifaces=""
  for phy in /sys/class/ieee80211/*; do
    [ -e "$phy" ] || continue
    pname="$(basename "$phy")"
    if iw phy "$pname" info 2>/dev/null | awk '/Supported interface modes/,/Band /' | grep -q '\* monitor'; then
      dev="$(ls "$phy/device/net" 2>/dev/null | head -1)"
      [ -n "$dev" ] && mon_ifaces="$mon_ifaces $dev"
    fi
  done
  mon_ifaces="${mon_ifaces# }"
  case "$(printf '%s' "$mon_ifaces" | wc -w)" in
    0) bootstrap_die "no Wi-Fi adapter that supports monitor mode was found.
       Remote ID capture needs one, and the Raspberry Pi's built-in Wi-Fi cannot
       do it. Is the external adapter (ALFA AWUS036AXM/AXML) plugged in?
       If it is and this still fails, supply it explicitly:
         curl -fsSL https://get.cybrrd.com | bash -s -- --interface <iface> ..." ;;
    1) bootstrap_note "capture iface: $mon_ifaces  (the only monitor-capable adapter)"
       set -- --interface "$mon_ifaces" "$@" ;;
    *) bootstrap_die "more than one monitor-capable adapter found ($mon_ifaces).
       Not guessing which one is the capture radio. Supply it explicitly:
         curl -fsSL https://get.cybrrd.com | bash -s -- --interface <iface> ..." ;;
  esac
fi

# ── Sensor position ─────────────────────────────────────────────────────────
# The service waits for GPS, never the installer or the owner. A paired explicit
# expert override is still accepted; existing configuration is never reseeded.
if [ "$lat_supplied" -ne "$lon_supplied" ]; then
  bootstrap_die "supply --latitude and --longitude together, or neither."
elif [ "$install_mode" = "rerun" ]; then
  bootstrap_note "position     : existing configuration preserved; service seeds only if absent"
elif [ "$lat_supplied" -eq 1 ]; then
  bootstrap_note "position     : optional expert override supplied"
else
  bootstrap_note "position     : GPS service will wait for a measured fix; install will not wait or prompt"
fi
bootstrap_say ""
BOOTSTRAP_PREPARE_EOF
)"
fi
DRY_RUN=0
VERIFY_ONLY=0
UNINSTALL=0
ASSUME_YES=0
STATUS_ONLY=0
ADOPT_LEGACY_ACCOUNTS=0
AUDIT_LOG=""
REQUESTED_IMAGE=""
REQUESTED_CONSOLE_IMAGE=""
CONSOLE_LISTEN=""
INSTALL_INTERFACE=""
INSTALL_LATITUDE=""
INSTALL_LONGITUDE=""
INSTALL_RING=""
GPS_USB_ID_FLAG=""
ORIGINAL_ARGS="$*"
while [[ $# -gt 0 ]]; do
  arg=$1
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --verify)  VERIFY_ONLY=1 ;;
    --no-verbose) : ;; # Consumed by the logging supervisor; never changes actions.
    --uninstall) UNINSTALL=1 ;;
    --yes) ASSUME_YES=1 ;;
    --status) STATUS_ONLY=1 ;;
    --adopt-legacy-accounts) ADOPT_LEGACY_ACCOUNTS=1 ;;
    --audit-log=*) AUDIT_LOG="${arg#*=}" ;;
    --audit-log)
      echo "FATAL: --audit-log requires a path. Example: --audit-log=/var/log/brrdfeeder-install.log" >&2
      exit 2
      ;;
    --console-image|--console-listen|--interface|--latitude|--longitude|--ring|--gps-usb-id)
      [[ $# -ge 2 && -n "$2" && "$2" != --* ]] || {
        echo "FATAL: $arg requires a value" >&2; exit 2;
      }
      case "$arg" in
        --console-image) REQUESTED_CONSOLE_IMAGE=$2 ;;
        --console-listen) CONSOLE_LISTEN=$2 ;;
        --interface) INSTALL_INTERFACE=$2 ;;
        --latitude) INSTALL_LATITUDE=$2 ;;
        --longitude) INSTALL_LONGITUDE=$2 ;;
        --ring) INSTALL_RING=$2 ;;
        --gps-usb-id)
          [[ $2 =~ ^[0-9A-Fa-f]{4}:[0-9A-Fa-f]{4}$ ]] || {
            echo 'FATAL: --gps-usb-id requires VID:PID (e.g. 10c4:ea60)' >&2; exit 2;
          }
          GPS_USB_ID_FLAG=${2,,} ;;
      esac
      shift
      ;;
    --image)
      [[ $# -ge 2 && -n "$2" && "$2" != --* ]] || {
        echo 'FATAL: --image requires ghcr.io/cybrrd/brrdfeeder@sha256:<approved-release-digest>' >&2
        exit 2
      }
      REQUESTED_IMAGE=$2
      shift
      ;;
    *) echo "Unknown flag: $arg"; exit 2 ;;
  esac
  shift
done

if [[ $STATUS_ONLY -eq 1 ]]; then
  [[ $UNINSTALL$VERIFY_ONLY$DRY_RUN$ASSUME_YES$ADOPT_LEGACY_ACCOUNTS == 00000 && -z "$REQUESTED_IMAGE$REQUESTED_CONSOLE_IMAGE$CONSOLE_LISTEN$INSTALL_INTERFACE$INSTALL_LATITUDE$INSTALL_LONGITUDE" ]] || { echo 'FATAL: status does not accept install/uninstall options'; exit 2; }
  log_event PHASE $'status\tRead-only local service status'
  for unit in brrdfeeder-engine.service brrdfeeder-updater.path; do
    state=$(systemctl show "$unit" -p ActiveState --value 2>/dev/null) || state=unknown
    printf '%s: %s\n' "$unit" "${state:-inactive (never installed or uninstalled)}"
  done
  if record=$(getent passwd brrdhouse); then
    IFS=: read -r name _ uid gid _ home shell <<< "$record"
    if [[ $name == brrdhouse && $uid =~ ^[0-9]+$ && $uid -ge 100 && $home == /var/lib/brrdhouse && $shell == /usr/sbin/nologin && -S /run/user/$uid/bus ]]; then
      state=$(runuser -u brrdhouse -- env XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus" systemctl --user show brrdhouse.service -p ActiveState --value 2>/dev/null) || state=unknown
    else state='inactive or user manager unavailable'; fi
  else state='inactive (never installed or uninstalled)'; fi
  printf 'brrdhouse.service: %s\n' "${state:-unknown}"
  exit 0
fi

# The supervisor already owns logging. Dispatch before image validation,
# enrollment or any install mutation; uninstall itself remains offline.
if [[ $UNINSTALL -eq 1 ]]; then
  [[ $VERIFY_ONLY -eq 0 && -z "$REQUESTED_IMAGE$REQUESTED_CONSOLE_IMAGE$CONSOLE_LISTEN$INSTALL_INTERFACE$INSTALL_LATITUDE$INSTALL_LONGITUDE" ]] || {
    echo 'FATAL: --uninstall cannot be combined with install/verify options' >&2; exit 2;
  }
  log_event PHASE $'uninstall\tValidating local removal'
  uninstall_main() {
  bash -s -- "$@" <<'UNINSTALL_EOF'
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
UNINSTALL_EOF
  }
  # Capture the full validated WOULD-list before asking, without terminal noise.
  if [[ $DRY_RUN -eq 0 ]]; then uninstall_main 1 "$ADOPT_LEGACY_ACCOUNTS" "" plan "$ASSUME_YES"; fi
  uninstall_main "$DRY_RUN" "$ADOPT_LEGACY_ACCOUNTS" "" confirm "$ASSUME_YES"
  exit $?
fi
[[ $ASSUME_YES -eq 0 ]] || { echo 'FATAL: --yes is only for uninstall'; exit 2; }
[[ $ADOPT_LEGACY_ACCOUNTS -eq 0 ]] || { echo 'FATAL: --adopt-legacy-accounts requires --uninstall' >&2; exit 2; }

# ----------------------------------------------------------------------
# Output helpers
# ----------------------------------------------------------------------
say()   { echo "[brrdfeeder-install] $*"; }
gate()  { LOG_PHASE=$1; shift; log_event PHASE "$LOG_PHASE"$'\t'"$*"; echo "[brrdfeeder-install] >>> $*"; }
ok()    { echo "[brrdfeeder-install]  OK $*"; }
warn()  { echo "[brrdfeeder-install]  !! $*" >&2; }
fatal() { echo "[brrdfeeder-install] FATAL $*" >&2; exit 1; }

run() {
  if [[ $DRY_RUN -eq 1 ]]; then
    echo "[dry-run]  + $*"
  else
    run_step "$LOG_PHASE/${1##*/}" "$@"
  fi
}

# Explicit operands matter: uutils install applies umask to implicit parents.
# Callers list each possibly absent public parent, in parent-first order.
public_directories() {
  local directory
  for directory in "$@"; do
    run install -d -m 0755 -o root -g root "$directory"
    if [[ $DRY_RUN -ne 1 && $(stat -c '%a' "$directory") != 755 ]]; then
      fatal "Directory $directory must be 0755 so services can traverse it."
    fi
  done
}

# Parse node.storage_class from config.yaml (defaults to "persistent" if
# absent). Used by Step 4 to decide whether to apply BRRDfeeder-tier hardening.
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

# ----------------------------------------------------------------------
# Pre-flight substrate-truth checks
# ----------------------------------------------------------------------
atomic_install() {
  # Same-directory publication; stdin or a source file, with final metadata
  # durable before rename. No installer-owned final path is ever truncated.
  python3 /dev/fd/3 "$@" 3<<'ATOMIC_INSTALL_PY'
import grp, os, pathlib, pwd, subprocess, sys, tempfile
mode, owner, group, destination, *source = sys.argv[1:]
path = pathlib.Path(destination)
uid = int(owner) if owner.isdecimal() else pwd.getpwnam(owner).pw_uid
gid = int(group) if group.isdecimal() else grp.getgrnam(group).gr_gid
for parent in [path, *path.parents]:
    if parent.is_symlink(): raise SystemExit('Unsafe atomic install symlink: '+str(parent))
    if parent.exists():
        s = parent.stat()
        if s.st_uid != 0 or s.st_mode & 0o022:
            raise SystemExit('Unsafe atomic install owner/mode: '+str(parent))
if path.exists() and (not path.is_file() or path.stat().st_nlink != 1):
    raise SystemExit('Unsafe atomic install target: '+str(path))
if len(source) > 1:
    # Render edits completely before publication; a failed producer must not
    # replace a valid config with empty/partial pipeline output.
    data = subprocess.check_output([*source[1:], source[0]])
else:
    data = pathlib.Path(source[0]).read_bytes() if source else sys.stdin.buffer.read()
fd, temporary = tempfile.mkstemp(prefix='.brrdfeeder-atomic-', dir=path.parent)
try:
    with os.fdopen(fd, 'wb') as stream:
        os.fchown(stream.fileno(), uid, gid)
        os.fchmod(stream.fileno(), int(mode, 8))
        stream.write(data); stream.flush(); os.fsync(stream.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(directory)
    finally: os.close(directory)
finally:
    if os.path.exists(temporary): os.unlink(temporary)
ATOMIC_INSTALL_PY
}

validate_account_receipt() {
  # A zero-byte legacy receipt is evidence of an interrupted write, never
  # sufficient identity evidence alone. Nonempty mismatches always refuse.
  local user=$1 record name account_uid account_gid account_home account_shell receipt expected max groups privilege permissions
  receipt=/etc/brrdfeeder/.installer-created-$user
  [[ -e $receipt || -L $receipt ]] || return 0
  [[ ! -L /etc/brrdfeeder && $(realpath -m /etc/brrdfeeder) == /etc/brrdfeeder && $(stat -c %u /etc/brrdfeeder) == 0 ]] || fatal 'Unsafe account receipt directory.'
  permissions=$(stat -c %a /etc/brrdfeeder)
  (( (8#$permissions & 0022) == 0 )) || fatal 'Writable account receipt directory.'
  [[ -f $receipt && ! -L $receipt && $(stat -c %u "$receipt") == 0 && $(stat -c %h "$receipt") == 1 && $(stat -c %a "$receipt") == 600 ]] || fatal 'Unsafe account receipt.'
  record=$(getent passwd "$user") || fatal 'Orphan account receipt.'
  [[ $record != *$'\n'* ]] || fatal 'Ambiguous account identity.'
  IFS=: read -r name _ account_uid account_gid _ account_home account_shell <<< "$record"
  [[ $name == "$user" && $account_uid =~ ^[0-9]+$ && $account_gid =~ ^[0-9]+$ && $account_uid -ge 100 && $account_gid -ge 100 && $account_home == /var/lib/$user && $account_shell == /usr/sbin/nologin ]] || fatal 'Unsafe interrupted-install account profile.'
  if [[ $user == brrdfeeder ]]; then
    max=$(awk '$1=="SYS_UID_MAX" {print $2; exit}' /etc/login.defs); max=${max:-999}
    [[ $max =~ ^[0-9]+$ && $account_uid -le $max ]] || fatal 'Recovery requires a system account.'
  fi
  [[ $(getent passwd | awk -F: -v n="$account_uid" '$3==n {c++} END {print c+0}') == 1 ]] || fatal 'Shared account UID.'
  [[ $(getent group "$account_gid") == "$user:x:$account_gid:" ]] || fatal 'Recovery requires a dedicated group.'
  [[ $(passwd -S "$user" | awk '{print $2}') == L ]] || fatal 'Recovery requires a locked password.'
  groups=$(id -nG "$user") || fatal 'Cannot inspect account groups.'
  for privilege in sudo adm wheel root; do
    [[ " $groups " != *" $privilege "* ]] || fatal "Unsafe privileged account group: $privilege"
  done
  expected="v1:$user:$account_uid:$account_gid:$account_home:$account_shell"
  [[ ! -s $receipt || $(<"$receipt") == "$expected" ]] || fatal "account receipt mismatch: $user"
}

recover_account_receipt() {
  local user=$1 receipt=/etc/brrdfeeder/.installer-created-$1 record name account_uid account_gid account_home account_shell
  [[ -f $receipt && ! -s $receipt ]] || return 0
  validate_account_receipt "$user"
  record=$(getent passwd "$user")
  IFS=: read -r name _ account_uid account_gid _ account_home account_shell <<< "$record"
  printf 'v1:%s:%s:%s:%s:%s\n' "$name" "$account_uid" "$account_gid" "$account_home" "$account_shell" | atomic_install 0600 root root "$receipt"
  ok "Recovered interrupted account receipt: $user"
}

record_created_account() {
  # Provenance only: never claim an account the installer reused. No secret data.
  local user=$1 record name account_uid account_gid account_home account_shell receipt
  [[ ! -L /etc/brrdfeeder && $(realpath -m /etc/brrdfeeder) == /etc/brrdfeeder ]] || fatal 'Unsafe account receipt directory.'
  [[ ! -e /etc/brrdfeeder || ( -d /etc/brrdfeeder && $(stat -c %u /etc/brrdfeeder) == 0 ) ]] || fatal 'Unsafe account receipt owner.'
  run install -d -m 0755 -o root -g root /etc/brrdfeeder
  record=$(getent passwd "$user") || fatal 'Cannot record newly created identity.'
  IFS=: read -r name _ account_uid account_gid _ account_home account_shell <<< "$record"
  receipt="/etc/brrdfeeder/.installer-created-$user"
  [[ ! -e $receipt && ! -L $receipt ]] || fatal 'Existing account receipt: refusing to overwrite provenance.'
  printf 'v1:%s:%s:%s:%s:%s\n' "$name" "$account_uid" "$account_gid" "$account_home" "$account_shell" | atomic_install 0600 root root "$receipt"
}
gate pre-flight "Pre-flight"

[[ $EUID -eq 0 ]] || fatal "Must run as root. Use: curl -fsSL https://get.cybrrd.com | bash"

install_local_command() {
  local command=/usr/local/sbin/brrdfeeder parent permissions temporary
  for parent in /usr /usr/local /usr/local/sbin; do
    [[ ! -L $parent && -d $parent && $(stat -c %u "$parent") == 0 ]] || fatal "Unsafe local command directory: $parent"
    permissions=$(stat -c %a "$parent")
    (( (8#$permissions & 0022) == 0 )) || fatal "Local command directory is writable by other users: $parent"
  done
  [[ ! -L $command ]] || fatal 'Local brrdfeeder command is a symlink; refusing to overwrite it.'
  if [[ -e $command ]]; then
    [[ -f $command && $(stat -c %u "$command") == 0 && $(stat -c %h "$command") == 1 ]] || fatal 'Unsafe existing brrdfeeder command.'
    if ! grep -qF '# BRRDfeeder local product command — self-contained recovery, no download.' "$command"; then
      [[ ! -s $command && -f /etc/brrdfeeder/.installer-created-brrdfeeder ]] || fatal 'An unrelated brrdfeeder command already exists; refusing to overwrite it.'
      validate_account_receipt brrdfeeder
    fi
    if cmp -s "$0" "$command"; then return; fi
  fi
  atomic_install 0755 root root "$command" "$0" || fatal 'Could not install the local recovery command.'
  ok 'Local commands ready: sudo brrdfeeder status | uninstall | support-bundle'
}
if [[ $DRY_RUN -eq 0 && $VERIFY_ONLY -eq 0 ]]; then
  validate_account_receipt brrdfeeder
  validate_account_receipt brrdhouse
  install_local_command
  recover_account_receipt brrdfeeder
  recover_account_receipt brrdhouse
fi

# Never resolve a registry tag here, and never use re-install as an update path.
retained_engine_version() {
  command -v python3 >/dev/null || return 0
  python3 - "$1" <<'RETAINED_ENGINE_VERSION_PY'
import json, os, re, stat, sys
try:
    fd = os.open('/var/lib/brrdfeeder-status/status.json', os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'r') as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > 1048576:
            raise ValueError('invalid status file')
        record = json.load(stream)
    heartbeat = record.get('heartbeat', {})
    version = heartbeat.get('product_version', '')
    # Last-reported version is useful only for the exact image being retained.
    # No registry query, guessed version, or workload invocation.
    if (record.get('schema_version') == 1 and heartbeat.get('image_digest') == sys.argv[1].split('@')[-1]
            and isinstance(version, str) and len(version) <= 64
            and re.fullmatch(r'\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?', version)):
        print(version)
except (OSError, ValueError, TypeError, AttributeError):
    pass
RETAINED_ENGINE_VERSION_PY
}

valid_image_pin() {
  local repository=${2:-$IMAGE_REPOSITORY}
  [[ $1 == "$repository"@sha256:* && ${1#"$repository"@sha256:} =~ ^[a-f0-9]{64}$ ]]
}
CONTAINER_IMAGE=$REQUESTED_IMAGE
if [[ -f "$QUADLET_FILE" ]]; then
  INSTALLED_IMAGE=$(sed -n 's/^Image=//p' "$QUADLET_FILE")
  valid_image_pin "$INSTALLED_IMAGE" || fatal "Existing Quadlet has no unique approved digest pin; explicit operator migration required (no tag resolution)."
  # A newer one-liner repairs host setup with the installed workload pins.
  # Explicit direct-installer digest changes still belong to the signed updater.
  [[ -z "$REQUESTED_IMAGE" || "$REQUESTED_IMAGE" == "$INSTALLED_IMAGE" || $BOOT_PREPARE -eq 1 ]] || fatal "Refusing to replace the installed digest: updates belong to the signed release poller. To install this release: sudo brrdfeeder uninstall, then run the one-liner again."
  if [[ ${VERIFY_ONLY:-0} -eq 0 ]]; then
    RETAINED_ENGINE_VERSION=$(retained_engine_version "$INSTALLED_IMAGE")
    if [[ -n "$RETAINED_ENGINE_VERSION" ]]; then
      say "Engine was NOT updated; version $RETAINED_ENGINE_VERSION remains installed (last reported by this image). This run repairs host setup only."
    else
      say 'Engine was NOT updated; the installed image is unchanged. Its version is unavailable from matching local status; check the console when it is reporting.'
    fi
  fi
  CONTAINER_IMAGE=$INSTALLED_IMAGE
fi
valid_image_pin "$CONTAINER_IMAGE" || fatal "Fresh install requires --image ${IMAGE_REPOSITORY}@sha256:<approved-release-digest>; tags are not accepted."
readonly CONTAINER_IMAGE

CONSOLE_IMAGE=$REQUESTED_CONSOLE_IMAGE
if [[ -f "$CONSOLE_QUADLET_FILE" ]]; then
  INSTALLED_CONSOLE_IMAGE=$(sed -n 's/^Image=//p' "$CONSOLE_QUADLET_FILE")
  valid_image_pin "$INSTALLED_CONSOLE_IMAGE" "$CONSOLE_REPOSITORY" || fatal "Existing console Quadlet has no unique approved digest pin; no tag resolution."
  [[ -z "$REQUESTED_CONSOLE_IMAGE" || "$REQUESTED_CONSOLE_IMAGE" == "$INSTALLED_CONSOLE_IMAGE" || $BOOT_PREPARE -eq 1 ]] \
    || fatal "Refusing to replace the installed console digest; reinstall is not an update channel. To install this release: sudo brrdfeeder uninstall, then run the one-liner again."
  CONSOLE_IMAGE=$INSTALLED_CONSOLE_IMAGE
  if [[ -z "$CONSOLE_LISTEN" ]]; then
    CONSOLE_LISTEN=$(sed -n 's/^Exec=--listen=\([^ ]*\) .*/\1/p' "$CONSOLE_QUADLET_FILE")
  fi
fi
valid_image_pin "$CONSOLE_IMAGE" "$CONSOLE_REPOSITORY" \
  || fatal "Fresh install requires --console-image ${CONSOLE_REPOSITORY}@sha256:<approved-release-digest>; tags are not accepted."
readonly CONSOLE_IMAGE
[[ -n "$CONSOLE_LISTEN" ]] || fatal "Fresh install requires --console-listen <literal-LAN-IP>:8080 (IPv6: [address]:8080)."
LISTEN_PATTERN='^[][0-9a-fA-F:.]+$'
[[ $CONSOLE_LISTEN =~ $LISTEN_PATTERN ]] || fatal "Console listener must be a literal IP:port, not a hostname, wildcard or shell expression."
if [[ -n "$INSTALL_INTERFACE$INSTALL_LATITUDE$INSTALL_LONGITUDE" ]]; then
  [[ -z "$INSTALL_INTERFACE" || ( $INSTALL_INTERFACE =~ ^[a-zA-Z0-9_.:-]{1,15}$ && $INSTALL_INTERFACE != -* ) ]] \
    || fatal "Supply a literal capture interface."
  if [[ -n "$INSTALL_LATITUDE$INSTALL_LONGITUDE" ]]; then
    [[ $INSTALL_LATITUDE =~ ^-?[0-9]+([.][0-9]+)?$ && $INSTALL_LONGITUDE =~ ^-?[0-9]+([.][0-9]+)?$ ]] \
      || fatal "Optional expert --latitude and --longitude must be supplied together as decimal coordinates."
    awk -v lat="$INSTALL_LATITUDE" -v lon="$INSTALL_LONGITUDE" 'BEGIN {exit !(lat>=-90 && lat<=90 && lon>=-180 && lon<=180 && (lat!=0 || lon!=0))}' \
      || fatal "Installation coordinates must be in range and not unknown (0,0)."
  fi
  [[ ! -e "$CONFIG_PATH" ]] || fatal "Existing config is preserved; omit first-install --interface/--latitude/--longitude parameters."
fi

# Service identity is independent of SUDO_USER and customer login accounts.
# A dry-run does not allocate IDs; symbolic values below are PLAN output only.
if ! PASSWD_RECORD=$(getent passwd "$TARGET_USER"); then
  if [[ $DRY_RUN -eq 1 ]]; then
    say "[dry-run] would create system account $TARGET_USER (UID/GID allocated on apply)"
    TARGET_UID=ALLOCATED_ON_APPLY
    TARGET_GID=ALLOCATED_ON_APPLY
    TARGET_HOME=$SERVICE_HOME
  elif [[ $VERIFY_ONLY -eq 1 ]]; then
    fatal "Service account $TARGET_USER is absent; run the installer without --verify to create it."
  else
    run useradd --system --user-group --no-create-home --home-dir "$SERVICE_HOME" --shell /usr/sbin/nologin "$TARGET_USER" \
      || fatal "Cannot create system account $TARGET_USER; check account tools/name collisions and retry."
    record_created_account "$TARGET_USER"
    PASSWD_RECORD=$(getent passwd "$TARGET_USER") \
      || fatal "Created service account $TARGET_USER cannot be resolved through getent passwd."
  fi
fi
if [[ -n "$PASSWD_RECORD" ]]; then
  [[ "$PASSWD_RECORD" != *$'\n'* ]] || fatal "Ambiguous service account record for $TARGET_USER."
  IFS=: read -r ACCOUNT_NAME _ TARGET_UID TARGET_GID _ TARGET_HOME TARGET_SHELL <<< "$PASSWD_RECORD"
  SYSTEM_UID_MAX=$(awk '$1 == "SYS_UID_MAX" { print $2; exit }' /etc/login.defs)
  SYSTEM_UID_MAX=${SYSTEM_UID_MAX:-999}
  [[ "$ACCOUNT_NAME" == "$TARGET_USER" && "$TARGET_UID" =~ ^[0-9]+$ && "$TARGET_GID" =~ ^[0-9]+$ \
     && "$SYSTEM_UID_MAX" =~ ^[0-9]+$ ]] || fatal "Invalid service account record for $TARGET_USER."
  [[ "$TARGET_UID" -gt 0 && "$TARGET_UID" -le "$SYSTEM_UID_MAX" && "$TARGET_GID" -gt 0 \
     && "$TARGET_HOME" == /* && "$TARGET_HOME" != / \
     && ( "$TARGET_SHELL" == /usr/sbin/nologin || "$TARGET_SHELL" == /sbin/nologin ) ]] \
    || fatal "Unsafe existing $TARGET_USER account: require non-root system UID/GID, absolute non-root home and nologin shell; refusing to repurpose a login account."
  [[ $(id -u "$TARGET_USER") == "$TARGET_UID" && $(id -g "$TARGET_USER") == "$TARGET_GID" ]] \
    || fatal "Service account $TARGET_USER identity is inconsistent."
  SERVICE_GROUP_RECORD=$(getent group "$TARGET_GID") || fatal "Cannot resolve service group for $TARGET_USER."
  IFS=: read -r SERVICE_GROUP _ _ SERVICE_MEMBERS <<< "$SERVICE_GROUP_RECORD"
  [[ "$SERVICE_GROUP" == "$TARGET_USER" && ( -z "$SERVICE_MEMBERS" || "$SERVICE_MEMBERS" == "$TARGET_USER" ) ]] \
    || fatal "Service group must be dedicated to $TARGET_USER; refusing shared credential access."
elif [[ $DRY_RUN -ne 1 || ${TARGET_UID:-} != ALLOCATED_ON_APPLY ]]; then
  fatal "Service account $TARGET_USER cannot be resolved through getent passwd."
fi
DIALOUT_RECORD=$(getent group dialout) || fatal "Required dialout group is absent; provision the Debian system group before retrying."
IFS=: read -r _ _ DIALOUT_GID _ <<< "$DIALOUT_RECORD"
[[ "$DIALOUT_GID" =~ ^[0-9]+$ && "$DIALOUT_GID" -gt 0 ]] || fatal "Invalid dialout group ID."
readonly TARGET_UID TARGET_GID TARGET_HOME DIALOUT_GID
readonly LEGACY_CONFIG="${TARGET_HOME}/brrdfeeder/config.yaml"
readonly LEGACY_CREDS="${TARGET_HOME}/brrdfeeder/secrets/brrdfeeder.creds"
ok "service user $TARGET_USER (uid=$TARGET_UID gid=$TARGET_GID home=$TARGET_HOME; dialout=$DIALOUT_GID)"

# --- Bootstrap deps for enrollment ------------------------------------
# The Zitadel Device Flow (still in pre-flight, below) parses JSON with jq
# and talks HTTPS with curl. A factory-fresh Pi OS Lite image has neither,
# and the full package install is Step 1 — AFTER pre-flight. Install the
# minimal enrollment toolchain up front so the Device Flow can run.
if [[ $DRY_RUN -eq 0 ]] && { ! command -v jq >/dev/null 2>&1 || ! command -v curl >/dev/null 2>&1; }; then
  say "installing enrollment bootstrap deps (jq, curl)…"
  apt-get update -qq && apt-get install -y -qq jq curl ca-certificates
  ok "enrollment bootstrap deps present"
fi

# --- Canonical config tree -------------------------------------------
# Create /etc/brrdfeeder (0755) + secrets/ (0750 root:service) up front.
# NATS creds are 0640 root:service; the refresh token remains 0600 root.
run install -d -m 0755 -o root -g root "$ETC_DIR"
run install -d -m 0750 -o root -g "$TARGET_GID" "$SECRETS_DIR"
ok "config tree present: $ETC_DIR (secrets/ 0750 root:service-group)"

# Persistent anti-replay state and signed-update requests.
run install -d -m 0750 -o "$TARGET_UID" -g "$TARGET_GID" "$STATE_DIR"
ok "state dir present: $STATE_DIR (service-owned)"

# --- Power-supply sanity (Pi only) ------------------------------------
# A high-draw monitor-mode radio (Alfa/MT7921U) + GPS can brown out the USB
# bus on an inadequate PSU, intermittently dropping the radio/GPS — brutal on
# a remote feeder. get_throttled bit 0 = undervoltage now, bit 16 = latched
# since boot. Either means: replace the USB-C supply before deploying.
# The supervisor already made one bounded, detached firmware query. Reuse it:
# another shell pipeline here would reintroduce the uninterruptible-device hang.
if [[ ${BRRDFEEDER_POWER_STATE:-unknown} == observed ]]; then
  THROTTLED=${BRRDFEEDER_POWER_VALUE:-}
  if [[ ! $THROTTLED =~ ^0x[0-9a-fA-F]{1,8}$ ]]; then
    warn "power check: unknown (invalid firmware response). Reboot the Pi, then retry."
  elif (( THROTTLED & 0x10001 )); then
    warn "UNDERVOLTAGE DETECTED (get_throttled=$THROTTLED) — the USB-C power"
    warn "supply is inadequate for this Pi + radio + GPS load. Replace it with"
    warn "a quality 5.1V/3A+ (Pi 4) supply before field deployment. See"
    warn "docs/brrdfeeder-skus.md (Power Supply). Continuing install, but this"
    warn "node is NOT deploy-ready until get_throttled reads 0x0."
  elif (( THROTTLED != 0 )); then
    warn "Power/temperature limits reported (get_throttled=$THROTTLED). Check cooling and the power supply."
  else
    ok "power supply OK (get_throttled=$THROTTLED)"
  fi
elif [[ ${BRRDFEEDER_POWER_STATE:-unknown} == unknown ]]; then
  warn "${BRRDFEEDER_POWER_MESSAGE:-power check: unknown (firmware query unavailable). Reboot the Pi, then retry.}"
fi

# --- Legacy migration (pre-2026-06-10 layout) -------------------------
if [[ ! -f "$CONFIG_PATH" && -f "$LEGACY_CONFIG" ]]; then
  say "migrating legacy config: $LEGACY_CONFIG → $CONFIG_PATH"
  run atomic_install 0644 root root "$CONFIG_PATH" "$LEGACY_CONFIG"
fi
if [[ ! -f "$CREDS_PATH" && -f "$LEGACY_CREDS" ]]; then
  say "migrating legacy creds: $LEGACY_CREDS → $CREDS_PATH"
  run atomic_install 0600 root root "$CREDS_PATH" "$LEGACY_CREDS"
fi

# --- config.yaml: generate baseline template if absent ----------------
if [[ ! -f "$CONFIG_PATH" ]]; then
  say "no config.yaml found — generating baseline template"
  if [[ $DRY_RUN -eq 0 ]]; then
    # Schema mirrors the known-good running config (test-node-1/test-node-2). The
    # The pinned engine still requires node.location. Its ExecStartPre helper
    # supplies a measured GPS fix before the container runs; never a placeholder.
    atomic_install 0644 root root "$CONFIG_PATH" <<'CFGEOF'
# /etc/brrdfeeder/config.yaml — BRRDfeeder node configuration
# Generated as a TEMPLATE by brrdfeeder-install.sh.
# Replace the placeholder field(s) below, then re-run: sudo bash brrdfeeder-install.sh

node:
  id: "unenrolled"                 # AUTO-ASSIGNED by flock at Device Flow enrollment
  channel: general                # Self-Update channel: dev | staging | general; signed release must match
  status_file: /var/lib/brrdfeeder-status/status.json
  storage_class: "ephemeral"       # ephemeral = SD-card (BRRDfeeder tier, journald-to-RAM)
  # location is seeded by the service from GPS, not guessed by the installer.

capture:
  interface: "EDIT-ME-wlanX"       # monitor-mode-capable Wi-Fi iface (ip link show)
  hunter:
    enabled: true

backhaul:
  broker_urls:
    - "tls://ingest.cybrrd.com:4222"
  credentials_path: "/etc/brrdfeeder/secrets/brrdfeeder.creds"
  target_subject: "cybrrd.telemetry.frame.rid"

tuning:
  edge_processing:
    deduplication_window_ms: 1000
  heartbeat:
    interval_secs: 5

sensors:
  gps:
    device: "/dev/cybrrd_gps"      # stable udev symlink — leave as-is
    required: true
CFGEOF
    if [[ -n "$INSTALL_INTERFACE" ]]; then
      atomic_install 0644 root root "$CONFIG_PATH" "$CONFIG_PATH" sed -e "s/EDIT-ME-wlanX/$INSTALL_INTERFACE/"
    fi
    if [[ -n "$INSTALL_LATITUDE" ]]; then
      atomic_install 0644 root root "$CONFIG_PATH" "$CONFIG_PATH" sed "/^  # location is seeded/c\\  location: {latitude: $INSTALL_LATITUDE, longitude: $INSTALL_LONGITUDE, elevation_meters: 0}"
      say "Position: optional expert override supplied."
    else
      say "Position: GPS service will seed a measured fix; installer will not wait."
    fi
    run chmod 0644 "$CONFIG_PATH"
  fi
  if [[ -n "$INSTALL_INTERFACE" && $DRY_RUN -eq 0 ]]; then
    ok "config created from capture interface; position is expert-supplied or deferred to GPS"
  elif [[ $DRY_RUN -eq 1 ]]; then
    warn "[dry-run] would write TEMPLATE to $CONFIG_PATH (no file written)"
  else
    warn "TEMPLATE written to $CONFIG_PATH"
  fi
  if [[ -z "$INSTALL_INTERFACE" || $DRY_RUN -eq 1 ]]; then
    warn "ACTION REQUIRED: supply --interface on first install, or edit the capture-interface template and rerun. No position flags are required."
    exit 2
  fi
fi
ok "config.yaml present at $CONFIG_PATH"

# Gate on placeholder VALUES (EDIT-ME-<token>), NOT the bare string — the
# template's own instructional text must never trip this check.
if grep -qE 'EDIT-ME-[A-Za-z]' "$CONFIG_PATH" 2>/dev/null; then
  fatal "config.yaml still contains EDIT-ME- placeholders. Edit $CONFIG_PATH, then re-run."
fi

# --- creds: Zitadel OAuth Device Flow enrollment ----------------------
# ZITADEL_DEVICE_FLOW_HOOK — implemented 2026-06-10 (Option A exchange).
# Flow: device_authorization → user approves in browser → poll token
# endpoint → exchange access_token at flock for scoped NATS .creds.
# Manual scp remains a fallback. Approval needs a browser, not terminal input.

manual_creds_instructions() {
  warn "Manual provisioning fallback:"
  warn "  scp brrdfeeder.creds <host>:/tmp/ && sudo install -m 0600 -o root -g root /tmp/brrdfeeder.creds $CREDS_PATH"
}

zitadel_device_flow() {
  # CRITICAL: suppress xtrace for the ENTIRE enrollment. Two reasons:
  #   1. SECURITY — set -x would write the device_code, access_token,
  #      refresh_token, and id_token into the audit log in plaintext.
  #      Tokens must NEVER hit disk. (Leak found 2026-06-13, field-node.)
  #   2. UX — the prompt + poll loop must be clean and readable, not buried
  #      in curl/jq/sleep trace. The user has to CATCH the code.
  # Restored at every return path below (and the single success path).
  local _xtrace=0; case $- in *x*) _xtrace=1; set +x ;; esac
  _restore_xtrace() { [[ $_xtrace -eq 1 ]] && set -x || true; }

  # ANSI bold/bright/green only when stdout is a terminal
  local B="" Y="" G="" R=""
  if [[ -t 1 ]]; then B=$'\033[1m'; Y=$'\033[1;33m'; G=$'\033[1;32m'; R=$'\033[0m'; fi

  gate enrollment "Linking to your cyBRRD account"
  log_event DETAIL 'Zitadel OAuth Device Flow enrollment'
  local attempt last_outcome="" access_token="" refresh_token=""
  poll_outcome() {
    if [[ $last_outcome != "$1" ]]; then
      log_event DETAIL "device-flow outcome=$1 attempt=$attempt"
      last_outcome=$1
    fi
  }
  for attempt in 1 2 3; do
  access_token=""; refresh_token=""; last_outcome=""

  # ---- Step 1: request device authorization --------------------------
  local auth_resp
  auth_resp=$(curl -sf --connect-timeout 15 --max-time 30 -X POST "${OAUTH_ISSUER}/oauth/v2/device_authorization" \
    -H "Content-Type: application/x-www-form-urlencoded" \
    --data-urlencode "client_id=${OAUTH_CLIENT_ID}" \
    --data-urlencode "scope=${OAUTH_SCOPE}") \
    || { warn "device_authorization request failed (network or ${OAUTH_ISSUER} unreachable)"; _restore_xtrace; return 1; }

  local device_code user_code verification_uri_complete expires_in interval
  device_code=$(jq -re '.device_code' <<<"$auth_resp")                         || { warn "bad device_authorization response"; _restore_xtrace; return 1; }
  user_code=$(jq -re '.user_code' <<<"$auth_resp") || { warn 'Account linking returned an incomplete code.'; _restore_xtrace; return 1; }
  log_secret device-code "$device_code"
  log_secret device-code "$user_code"
  verification_uri_complete=$(jq -re '.verification_uri_complete // .verification_uri' <<<"$auth_resp") || { warn 'Account linking returned no approval link.'; _restore_xtrace; return 1; }
  expires_in=$(jq -re '.expires_in // 300' <<<"$auth_resp")
  interval=$(jq -re '.interval // 5' <<<"$auth_resp")
  if [[ ! $expires_in =~ ^[1-9][0-9]{0,4}$ || ! $interval =~ ^(0|[1-9][0-9]{0,3})$ ]]; then
    warn 'Account linking returned an invalid time limit.'; _restore_xtrace; return 1
  fi

  # ---- Step 2: prompt the operator (BIG, unmissable, install is PAUSED) -
  printf '\n\n'
  echo "${B}  ╔═══════════════════════════════════════════════════════════════╗${R}"
  echo "${B}  ║   ⏸  INSTALL PAUSED — ONE STEP NEEDS YOU                       ║${R}"
  echo "${B}  ╚═══════════════════════════════════════════════════════════════╝${R}"
  echo
  echo "     ${B}1.${R} On your phone or computer, open this link:"
  echo
  echo "        ${Y}${verification_uri_complete}${R}"
  echo
  echo "     ${B}2.${R} Confirm the code shown there matches:   ${Y}${B}${user_code}${R}"
  echo
  echo "     ${B}3.${R} Sign in with your cyBRRD account and click Approve."
  echo "        (This binds the feeder to you + your leaderboard score.)"
  echo
  echo "     The install will continue ${G}automatically${R} the moment you approve."
  echo "     Code $attempt of 3: $expires_in seconds remaining. Expired codes refresh here automatically."
  echo

  # ---- Step 3: poll the token endpoint (quiet spinner, no trace) ------
  local deadline=$(( $(date +%s) + expires_in ))
  local token_resp http_code body err remaining wait_seconds
  while true; do
    remaining=$((deadline - $(date +%s)))
    (( remaining > 0 )) || break
    printf '     Waiting for your approval — %s seconds remaining.\n' "$remaining"
    wait_seconds=$interval
    (( wait_seconds <= remaining )) || wait_seconds=$remaining
    sleep "$wait_seconds"
    (( $(date +%s) < deadline )) || break

    token_resp=$(curl -s --connect-timeout 15 --max-time 30 -w '\n%{http_code}' -X POST "${OAUTH_ISSUER}/oauth/v2/token" \
      -H "Content-Type: application/x-www-form-urlencoded" \
      --data-urlencode "grant_type=urn:ietf:params:oauth:grant-type:device_code" \
      --data-urlencode "device_code=${device_code}" \
      --data-urlencode "client_id=${OAUTH_CLIENT_ID}")
    http_code=$(tail -n1 <<<"$token_resp")
    body=$(sed '$d' <<<"$token_resp")

    if [[ "$http_code" == "200" ]]; then
      access_token=$(jq -re '.access_token' <<<"$body") || { poll_outcome exchange-failed; warn "Account linking returned no access token."; _restore_xtrace; return 1; }
      refresh_token=$(jq -r '.refresh_token // empty' <<<"$body")
      log_secret access-token "$access_token"
      [[ -z $refresh_token ]] || log_secret refresh-token "$refresh_token"
      poll_outcome approved
      printf '\n\n'
      echo "  ${G}${B}✓ Approved — thank you. Continuing the install…${R}"
      echo
      break
    fi

    err=$(jq -r '.error // "unknown_error"' <<<"$body" 2>/dev/null || echo "unparseable")
    case "$err" in
      authorization_pending) poll_outcome pending ;;
      slow_down)             poll_outcome slow_down; interval=$(( interval + 5 )) ;; # RFC 8628 §3.5
      expired_token)         break ;;
      access_denied)         poll_outcome denied; warn "Account linking was declined in the browser."; _restore_xtrace; return 1 ;;
      *)                     poll_outcome poll-failed; warn "Could not check account approval. Check your connection and retry."; _restore_xtrace; return 1 ;;
    esac
  done
  [[ -z $access_token ]] || break
  poll_outcome expired
  if (( attempt < 3 )); then
    say 'That code expired. Getting a fresh code here — no need to restart the install.'
  else
    warn 'All three codes expired. Re-run from a terminal when you are ready to approve in your browser.'
    _restore_xtrace; return 1
  fi
  done

  # ---- Step 4/5: Option A exchange — access_token → NATS .creds -------
  gate enrollment "Finishing account linking"
  log_event DETAIL 'Exchanging access token at flock for scoped NATS credentials'
  local creds_resp creds_code creds_body hdrs
  hdrs=$(mktemp)
  creds_resp=$(curl -s --connect-timeout 15 --max-time 30 -D "$hdrs" -w '\n%{http_code}' -X POST "$FLOCK_ENROLL_URL" \
    -H "Authorization: Bearer ${access_token}" \
    -H "Content-Type: application/json" \
    -d "{\"node_hostname\":\"$(hostname)\"}") || true
  creds_code=$(tail -n1 <<<"$creds_resp")
  creds_body=$(sed '$d' <<<"$creds_resp")

  if [[ "$creds_code" != "200" ]]; then
    poll_outcome exchange-failed
    warn "Account approval succeeded, but the feeder could not finish linking. Please retry."
    run rm -f "$hdrs"; _restore_xtrace; return 1
  fi

  # Expect the raw .creds file body (-----BEGIN NATS USER JWT----- ...)
  if ! grep -q "BEGIN NATS USER JWT" <<<"$creds_body"; then
    poll_outcome exchange-failed
    warn "Account approval succeeded, but the returned feeder credentials were invalid. Please retry."
    run rm -f "$hdrs"; _restore_xtrace; return 1
  fi

  # flock assigns the canonical node_id; the engine reads it from config.yaml
  # and its creds are SCOPED to it — write it back or every publish is denied.
  local assigned_id
  assigned_id=$(awk -F': ' 'tolower($1)=="x-cybrrd-node-id" {gsub(/\r/,"",$2); print $2}' "$hdrs")
  run rm -f "$hdrs"
  if [[ -n "$assigned_id" ]]; then
    atomic_install 0644 root root "$CONFIG_PATH" "$CONFIG_PATH" sed "s/^\(\s*id:\s*\).*/\1\"${assigned_id}\"/"
    ok "node identity assigned by flock: ${assigned_id} (written to config.yaml)"
  else
    warn "flock did not return X-Cybrrd-Node-Id — config node.id left as-is"
  fi

  umask 077
  printf '%s\n' "$creds_body" | atomic_install 0640 root "$TARGET_GID" "$CREDS_PATH"
  if [[ -n "$refresh_token" ]]; then
    printf '%s\n' "$refresh_token" | atomic_install 0600 root root "$REFRESH_TOKEN_PATH"
    ok "refresh token stored (enables non-interactive creds renewal)"
  fi
  ok "NATS creds written to $CREDS_PATH (0640 root:service-group)"
  _restore_xtrace
  return 0
}

if [[ ! -f "$CREDS_PATH" ]]; then
  if [[ $DRY_RUN -eq 1 ]]; then
    say "[dry-run] would link this feeder to your cyBRRD account"
  else
    if ! zitadel_device_flow; then
      warn "Account linking did not complete."
      manual_creds_instructions
      fatal "Credentials required — re-run from a terminal to link your account, or provision manually and re-run."
    fi
  fi
fi
run chmod 0640 "$CREDS_PATH"
run chown root:"$TARGET_GID" "$CREDS_PATH"
ok "NATS creds present at $CREDS_PATH (0640 root:service-group)"

# Verify USB hardware visible (best-effort; lsusb may not exist until
# Step 1 installs usbutils on minimal images — guard accordingly)
HAVE_UBLOX=0; HAVE_NORDIC=0
gps_usb_preflight() {
  local id vendor product supported device parent supported_product supported_ids=""
  local -A seen=()
  # USB serial candidates come from sysfs ancestry, without opening/probing them.
  # lsusb also finds u-blox devices whose tty driver has not attached yet.
  while IFS= read -r id; do
    [[ $id =~ ^[[:xdigit:]]{4}:[[:xdigit:]]{4}$ ]] || continue
    id=${id,,}
    [[ -z ${seen[$id]:-} ]] || continue
    seen[$id]=1
    vendor=${id%:*}; product=${id#*:}; supported=0
    if [[ $vendor == "$UBLOX_VENDOR" ]]; then
      for supported_product in "${UBLOX_PRODUCTS[@]}"; do
        [[ $product != "$supported_product" ]] || supported=1
      done
    fi
    if [[ $id == "$gps_declared_usb_id" && $GPS_USB_ID_PRESENT -eq 1 ]]; then
      ok "Operator-declared GPS USB ID $id (NMEA-confirmed); tty rule will map /dev/$GPS_SYMLINK"
    elif [[ $supported -eq 1 ]]; then
      HAVE_UBLOX=1
      ok "Supported GPS USB ID $id detected; tty rule will map /dev/$GPS_SYMLINK"
    elif [[ $id == 10c4:ea60 && $HAVE_UBLOX -eq 0 && -z $gps_declared_usb_id ]]; then
      GPS_PROMPT_NEEDED=1
      warn "Silicon Labs CP2102N bridge $id present — the Adafruit Ultimate GPS (#746) uses this bridge, but so do many other devices. Not mapping without opt-in."
    else
      warn "USB serial adapter $id present — not a supported GPS; no automatic /dev/$GPS_SYMLINK mapping. See docs/gps-runtime.md for the explicit opt-in boundary; this may be non-GPS hardware."
    fi
  done < <(
    for device in /sys/class/tty/*/device; do
      parent=$(readlink -f "$device" 2>/dev/null) || continue
      while [[ $parent == /sys/devices/* ]]; do
        if [[ -r "$parent/idVendor" && -r "$parent/idProduct" ]]; then
          read -r vendor < "$parent/idVendor" || break
          read -r product < "$parent/idProduct" || break
          printf '%s:%s\n' "$vendor" "$product"
          break
        fi
        parent=${parent%/*}
      done
    done
    if command -v lsusb >/dev/null 2>&1; then
      lsusb -d "${UBLOX_VENDOR}:" 2>/dev/null | awk '$5 == "ID" {print $6}' || true
    fi
  )
  for product in "${UBLOX_PRODUCTS[@]}"; do supported_ids+=" $UBLOX_VENDOR:$product"; done
  say "Supported automatic GPS IDs:$supported_ids; connect one GPS receiver."
  [[ $HAVE_UBLOX -eq 1 ]] || warn "No supported GPS detected; the service will wait. Check the candidate USB IDs above before assuming the receiver is absent."
  if [[ $GPS_PROMPT_NEEDED -eq 1 && $GPS_USB_ID_PRESENT -eq 0 ]]; then
    local answer=""
    local terminal=${BRRDFEEDER_TTY_FD:-}
    if [[ ! $terminal =~ ^[0-9]+$ ]] || ! [[ -t $terminal ]]; then
      if ! { exec {terminal}<>/dev/tty; } 2>/dev/null; then
        terminal=""
      fi
    fi
    if [[ -n $terminal ]] && IFS= read -r -t 60 answer <&"$terminal"; then
      case $answer in
        y|Y|yes|YES)
          gps_declared_usb_id=10c4:ea60
          local candidate_tty; candidate_tty=$(gps_candidate_tty 10c4:ea60)
          if [[ -n $candidate_tty ]]; then
            say "GPS opt-in accepted: passive NMEA confirm on $candidate_tty (5 s, read-only)…"
            if [[ $(gps_nmea_confirm "$candidate_tty") == ok ]]; then
              GPS_USB_ID_PRESENT=1
              ok "NMEA confirmed on $candidate_tty; udev rule will map /dev/$GPS_SYMLINK for 10c4:ea60"
            else
              warn "Opted in, no NMEA seen on $candidate_tty. NOT mapping; the service will run the degraded path. Check wiring/power; re-run to retry."
            fi
          fi
          ;;
        *) say "GPS opt-in declined; 10c4:ea60 stays unmapped. Re-run with --gps-usb-id 10c4:ea60 to opt in later." ;;
      esac
    else
      say "No terminal for the GPS opt-in prompt; 10c4:ea60 stays unmapped (declined by default). Headless opt-in: --gps-usb-id 10c4:ea60"
    fi
  fi
}
gps_declared_id() {
  # sensors.gps.usb_id from the live config, if any (idempotent re-runs).
  python3 - "$CONFIG_PATH" <<'GPS_ID_EOF'
import sys, yaml
try:
    cfg = yaml.safe_load(open(sys.argv[1]))
    usb = cfg.get('sensors', {}).get('gps', {}).get('usb_id', '')
    import re
    if isinstance(usb, str) and re.fullmatch(r'[0-9a-fA-F]{4}:[0-9a-fA-F]{4}', usb):
        print(usb.lower())
except Exception:
    pass
GPS_ID_EOF
}

gps_nmea_confirm() {
  # Passive NMEA confirm for an opted-in candidate device (Synth amendment 2):
  # ~5s read-only at 9600 baud; requires >=2 checksum-valid $GP/$GN sentences;
  # NEVER writes to the port. Prints "ok" or "no-nmea".
  local dev=$1
  python3 - "$dev" <<'GPS_PROBE_EOF'
import sys, os, termios, select, time
dev = sys.argv[1]
try:
    fd = os.open(dev, os.O_RDONLY | os.O_NOCTTY | os.O_NONBLOCK)
except OSError:
    print('no-nmea'); raise SystemExit(0)
try:
    attrs = termios.tcgetattr(fd)
    attrs[0] = attrs[1] = attrs[3] = 0  # no input/output/line processing
    attrs[3] |= termios.CS8
    attrs[4] = termios.B9600
    attrs[5] = termios.B9600
    attrs[6][termios.VMIN], attrs[6][termios.VTIME] = 0, 0
    termios.tcsetattr(fd, termios.TCSANOW, attrs)
except termios.error:
    os.close(fd); print('no-nmea'); raise SystemExit(0)
def valid(s):
    if len(s) < 6 or not (s.startswith('$GP') or s.startswith('$GN')):
        return False
    if not s.endswith('*') and '*' not in s:
        return False
    body, _, ck = s.rpartition('*')
    if len(ck) < 2:
        return False
    v = 0
    for ch in body[1:]:
        v ^= ord(ch)
    return ('%02X' % v) == ck[:2].upper()
buf, seen, deadline = '', 0, time.monotonic() + 5.0
while time.monotonic() < deadline and seen < 2:
    r, _, _ = select.select([fd], [], [], max(0.0, deadline - time.monotonic()))
    if not r:
        continue
    try:
        chunk = os.read(fd, 256)
    except (OSError, BlockingIOError):
        continue
    if not chunk:
        break
    try:
        text = chunk.decode('ascii', 'replace')
    except Exception:
        continue
    for ch in text:
        if ch == '$':
            if valid(buf):
                seen += 1
            buf = '$'
        elif buf:
            buf += ch
            if len(buf) > 128:
                buf = ''
if valid(buf):
    seen += 1
os.close(fd)
print('ok' if seen >= 2 else 'no-nmea')
GPS_PROBE_EOF
}

gps_candidate_tty() {
  # First tty whose USB ancestry carries the declared VID:PID (sysfs only).
  local want=$1 device parent vendor product
  for device in /sys/class/tty/*/device; do
    parent=$(readlink -f "$device" 2>/dev/null) || continue
    while [[ $parent == /sys/devices/* ]]; do
      if [[ -r "$parent/idVendor" && -r "$parent/idProduct" ]]; then
        read -r vendor < "$parent/idVendor" || break
        read -r product < "$parent/idProduct" || break
        if [[ ${vendor,,}:${product,,} == "$want" ]]; then
          basename "$(dirname "$(readlink -f "$device")")" | sed 's|^|/dev/|'
          return 0
        fi
        break
      fi
      parent=${parent%/*}
    done
  done
  return 0
}

gps_declared_usb_id=""
if [[ -n $GPS_USB_ID_FLAG ]]; then
  gps_declared_usb_id=$GPS_USB_ID_FLAG
elif configured=$(gps_declared_id); then
  gps_declared_usb_id=$configured   # idempotent re-run: honor without prompting
fi
GPS_USB_ID_PRESENT=0
GPS_PROMPT_NEEDED=0
if [[ -n $gps_declared_usb_id && $HAVE_UBLOX -eq 0 ]]; then
  # A declared non-u-blox id is a deliberate claim: confirm NMEA before mapping.
  candidate_tty=$(gps_candidate_tty "$gps_declared_usb_id")
  if [[ -n $candidate_tty ]]; then
    say "GPS opt-in: $gps_declared_usb_id on $candidate_tty — passive NMEA confirm (5 s, read-only)…"
    probe=$(gps_nmea_confirm "$candidate_tty")
    if [[ $probe == ok ]]; then
      GPS_USB_ID_PRESENT=1
      ok "NMEA confirmed on $candidate_tty; udev rule will map /dev/$GPS_SYMLINK for $gps_declared_usb_id"
    else
      warn "Opted in, no NMEA seen on $candidate_tty ($gps_declared_usb_id). NOT mapping; the service will run the degraded path (preserved position + GPS fault). Check wiring/power; re-run to retry."
    fi
  else
    warn "Opted in GPS id $gps_declared_usb_id not currently attached; not mapping this run (config keeps the declaration; re-run after plugging it in)."
  fi
fi
gps_usb_preflight
# Passive USB inventory only; never send HCI commands while the engine may own it.
HAVE_REALTEK=0
for ble_device in /sys/bus/usb/devices/*; do
  if [[ -r "$ble_device/idVendor" && -r "$ble_device/idProduct" ]]; then
    read -r ble_vendor < "$ble_device/idVendor" || continue
    read -r ble_product < "$ble_device/idProduct" || continue
    if [[ ${ble_vendor,,}:${ble_product,,} == 0bda:876e || ${ble_vendor,,}:${ble_product,,} == 0bda:a728 ]]; then
      HAVE_REALTEK=1
      ok "Supported RID.BLE adapter found: Realtek USB ${ble_vendor,,}:${ble_product,,}; missing config key will be enabled before service startup."
    fi
  fi
done
if command -v lsusb >/dev/null 2>&1; then
  lsusb -d "${NORDIC_VENDOR}:${NORDIC_PRODUCT}" >/dev/null 2>&1 && HAVE_NORDIC=1
  [[ $HAVE_NORDIC -eq 1 ]] && ok "Nordic nRF52 detected on USB (${NORDIC_VENDOR}:${NORDIC_PRODUCT})" \
                           || { [[ $HAVE_REALTEK -eq 1 ]] || say "Legacy Nordic serial BLE adapter not detected; this does not test the Realtek USB Bluetooth receiver used by rid_ble."; }
else
  say "lsusb unavailable — USB inventory deferred until usbutils installed (Step 1)"
fi

# Capture interface declared in config actually exists? (warn-only — USB
# Wi-Fi may not be plugged yet)
CFG_IFACE="$(awk '/^capture:/{f=1;next} f&&/^[a-z]/{f=0} f&&/interface:/{gsub(/["'"'"' ]/,"",$2);print $2;exit}' "$CONFIG_PATH")"
if [[ -n "$CFG_IFACE" ]]; then
  if ip link show "$CFG_IFACE" >/dev/null 2>&1; then
    ok "capture interface $CFG_IFACE present"
  else
    warn "capture interface $CFG_IFACE NOT present — plug the adapter before the engine can capture"
  fi
fi

if [[ $VERIFY_ONLY -eq 1 ]]; then
  gate "Verify-only mode — checking current substrate state"
  if [[ -x /usr/local/libexec/brrdfeeder-gps-runtime ]]; then
    /usr/local/libexec/brrdfeeder-gps-runtime status
  fi
  [[ -f "$UDEV_RULES_FILE" ]]   && ok "udev rules present: $UDEV_RULES_FILE"   || warn "udev rules ABSENT"
  [[ -L "/dev/$GPS_SYMLINK" ]]  && ok "/dev/$GPS_SYMLINK -> $(readlink -f /dev/$GPS_SYMLINK)" || warn "/dev/$GPS_SYMLINK MISSING"
  [[ -L "/dev/$BLE_SYMLINK" ]]  && ok "/dev/$BLE_SYMLINK -> $(readlink -f /dev/$BLE_SYMLINK)" || warn "/dev/$BLE_SYMLINK MISSING (optional)"
  [[ -f "$QUADLET_FILE" ]]      && ok "Quadlet present: $QUADLET_FILE"        || warn "Quadlet ABSENT"
  if [[ -f "$CHRONY_DROPIN" || -f "$LEGACY_CHRONY_DROPIN" ]]; then
    ok "Clock correction settings present"
  else
    warn "Clock correction settings absent; re-run the installer."
  fi
  command -v podman &>/dev/null && ok "podman installed: $(podman --version)" || warn "podman NOT installed"
  command -v jq &>/dev/null     && ok "jq installed"                          || warn "jq NOT installed"
  systemctl is-active chrony >/dev/null 2>&1 && ok "chrony ACTIVE" || warn "chrony NOT active"
  VSC="$(get_storage_class "$CONFIG_PATH")"
  [[ -z "$VSC" ]] && VSC="(unset, defaults persistent)"
  log_event DETAIL "config node.storage_class = $VSC"
  if [[ -f "$JOURNALD_DROPIN" ]]; then
    ok "journald drop-in present (BRRDfeeder hardening installed)"
  elif [[ -f "$JOURNALD_DROPIN_LEGACY" ]]; then
    warn "legacy journald drop-in present — re-run the installer to migrate it to $JOURNALD_DROPIN"
  else
    say "System logs are kept on disk."
  fi
  systemctl is-active brrdfeeder-engine.service >/dev/null 2>&1 \
    && ok "brrdfeeder-engine.service is ACTIVE" \
    || warn "brrdfeeder-engine.service is NOT active"
  # The periodic signed-release poll is the convergence mechanism.
  [[ -d "$STATE_DIR" ]] && ok "state dir present ($STATE_DIR)" || warn "state dir MISSING ($STATE_DIR)"
  if systemctl is-active brrdfeeder-release-poll.timer >/dev/null 2>&1; then
    ok "host-updater ARMED (brrdfeeder-release-poll.timer active)"
  else
    warn "host-updater NOT armed (brrdfeeder-release-poll.timer inactive)"
  fi
  exit 0
fi

# ----------------------------------------------------------------------
# Step 1 — Install host packages (podman, jq, curl, chrony, usbutils)
# ----------------------------------------------------------------------
gate host-packages "Step 1 — host packages (podman, jq, curl, chrony, usbutils)"

if command -v podman &>/dev/null && command -v jq &>/dev/null \
   && command -v curl &>/dev/null && command -v chronyd &>/dev/null \
   && command -v lsusb &>/dev/null && command -v newuidmap &>/dev/null \
   && python3 -c 'import yaml' 2>/dev/null && dpkg-query -W -f='${Status}' dbus-user-session 2>/dev/null | grep -q 'install ok installed'; then
  ok "prerequisites already installed"
else
  run apt-get update
  # chrony's postinst auto-disables systemd-timesyncd — desired.
  run apt-get install -y --no-install-recommends podman jq curl ca-certificates chrony usbutils \
    uidmap dbus-user-session fuse-overlayfs python3 python3-yaml
  ok "host packages installed"
fi

# Remove only the exact old template pair; never serialize/reformat YAML.
# Run after python3-yaml is installed and after verify-only has returned.
migrate_template_lock_on() {
  local migration result mode owner group
  migration=$(cat <<'LOCK_ON_MIGRATION_PY'
import pathlib, re, sys, yaml
raw = pathlib.Path(sys.argv[2]).read_bytes()
changed = raw
try:
    text = raw.decode('utf-8')
    # Aliases/anchors and duplicate keys make ownership ambiguous: preserve.
    if any(isinstance(t, (yaml.tokens.AnchorToken, yaml.tokens.AliasToken))
           for t in yaml.scan(text)):
        raise ValueError('references')
    def mapping(node):
        if not isinstance(node, yaml.MappingNode) or node.flow_style:
            raise ValueError('not a block mapping')
        result = {}
        for key, value in node.value:
            if not isinstance(key, yaml.ScalarNode) or key.value in result:
                raise ValueError('ambiguous key')
            result[key.value] = (key, value)
        return result
    root = mapping(yaml.compose(text))
    capture = mapping(root['capture'][1])
    hunter = mapping(capture['hunter'][1])
    lines = text.splitlines(keepends=True)
    remove = set()
    for name, expected in [('lock_on_duration_ms', '2000'), ('lock_on_min_ms', '500')]:
        key, value = hunter[name]
        line = lines[key.start_mark.line]
        if not re.fullmatch(r' +' + name + ': ' + expected + '\n', line):
            raise ValueError('not byte-equal to the template')
        if value.tag != 'tag:yaml.org,2002:int' or value.value != expected:
            raise ValueError('not the template scalar')
        remove.add(key.start_mark.line)
    changed = ''.join(line for n, line in enumerate(lines) if n not in remove).encode('utf-8')
except (ValueError, KeyError, UnicodeError, yaml.YAMLError):
    pass
if sys.argv[1] == 'check':
    sys.exit(0 if changed != raw else 3)
sys.stdout.buffer.write(changed)
LOCK_ON_MIGRATION_PY
)
  if python3 -c "$migration" check "$CONFIG_PATH"; then
    read -r mode owner group < <(stat -c '%a %u %g' "$CONFIG_PATH")
    atomic_install "$mode" "$owner" "$group" "$CONFIG_PATH" "$CONFIG_PATH" python3 -c "$migration" render
    ok 'Removed old installer lock-on settings; channel preset defaults now apply.'
  else
    result=$?
    [[ $result -eq 3 ]] || fatal 'Could not check old installer lock-on settings; config unchanged.'
  fi
}
if [[ $DRY_RUN -eq 0 ]]; then
  migrate_template_lock_on
fi

# ----------------------------------------------------------------------
# Step 1b — immutable image pull, BEFORE writing/activating the deployment
# ----------------------------------------------------------------------
gate images "Checking the downloaded images match the approved versions"
log_event DETAIL 'Immutable image preflight: inspect-and-assert; no signature claim'
if [[ $DRY_RUN -eq 1 ]]; then
  say "[dry-run] would pull if absent and assert RepoDigests contains $CONTAINER_IMAGE"
else
  if ! podman image exists "$CONTAINER_IMAGE"; then
    run_step images/pull-engine podman pull "$CONTAINER_IMAGE" || fatal "Digest-pinned image pull failed; deployment not changed."
  fi
  # .Digest can be the architecture child of an index. Require the EXACT full
  # requested reference in RepoDigests, not a substring, tag, or child guess.
  IMAGE_REPO_DIGESTS=$(podman image inspect "$CONTAINER_IMAGE" --format '{{range .RepoDigests}}{{println .}}{{end}}') \
    || fatal "Cannot inspect the pinned image; deployment not changed."
  printf '%s\n' "$IMAGE_REPO_DIGESTS" | grep -qxF "$CONTAINER_IMAGE" \
    || fatal "Pinned image RepoDigests mismatch; deployment not changed."
  ok "Downloaded engine image matches the approved version"
  log_event DETAIL 'RepoDigests matches installer/operator-approved digest (NOT signature verified)'
fi

# ----------------------------------------------------------------------
# Step 1c — rootless console identity, immutable image and public status storage
# ----------------------------------------------------------------------
gate console "Step 1c — intrinsic BRRDhouse console (rootless, digest-pinned)"

console_run() {
  runuser -u "$CONSOLE_USER" -- sh -c 'cd "$1" && shift && exec "$@"' sh "$CONSOLE_HOME" \
    env XDG_RUNTIME_DIR="/run/user/$CONSOLE_UID" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$CONSOLE_UID/bus" "$@"
}

console_memory_policy() {
  CONSOLE_MEMORY_ARGS=""
  if [[ $DRY_RUN -eq 1 ]]; then
    log_event DETAIL "console_memory_limit=deferred reason=dry-run; would inspect the running console user's delegated cgroup"
    return
  fi
  local group controllers reason=delegation-unavailable
  group=$(systemctl show "user@$CONSOLE_UID.service" -p ControlGroup --value 2>/dev/null) || group=""
  # Resolve the actual service cgroup, never infer delegation from the root list.
  if [[ $group == "/user.slice/user-$CONSOLE_UID.slice/user@$CONSOLE_UID.service" ]]; then
    if controllers=$(console_run cat "/sys/fs/cgroup$group/cgroup.controllers" 2>/dev/null); then
      reason=memory-not-delegated
      if [[ " $controllers " == *" memory "* ]]; then
        reason=delegation-not-writable
        if console_run test -w "/sys/fs/cgroup$group/cgroup.subtree_control"; then
          CONSOLE_MEMORY_ARGS=" --memory=96m --memory-swap=96m"
          log_event DETAIL "console_memory_limit=96m reason=memory-delegated cgroup=$group"
          return
        fi
      fi
    fi
  fi
  log_event DETAIL "console_memory_limit=omitted reason=$reason cgroup=${group:-unknown}; console has no memory cap; other hardening is retained"
}

if [[ $DRY_RUN -eq 1 ]]; then
  say "[dry-run] would create locked console user, enable linger, verify $CONSOLE_IMAGE, run shipped status provisioner and configure node.status_file"
else
  # Strict literal listener validation, before a unit file or service is changed.
  run python3 - "$CONSOLE_LISTEN" <<'LISTEN_PY'
import ipaddress, sys
value = sys.argv[1]
try:
    host, port = value.rsplit(':', 1)
    address = ipaddress.ip_address(host.strip('[]'))
    assert str(int(port)) == port and 1024 <= int(port) <= 65535
    assert address.is_private and not address.is_unspecified and not address.is_multicast and not address.is_link_local
    assert value == (f'[{address}]:{port}' if address.version == 6 else f'{address}:{port}')
except (ValueError, AssertionError):
    sys.exit('Console listener must be a specific private LAN (or loopback) IP and unprivileged port; no WAN, tailnet, wildcard or link-local binding.')
LISTEN_PY
  if ! getent passwd "$CONSOLE_USER" >/dev/null; then
    # A locked non-login account, with useradd-allocated subordinate IDs for Podman.
    # Do not share the engine UID, group, private state or credentials.
    run useradd --create-home --user-group --home-dir "$CONSOLE_HOME" --shell /usr/sbin/nologin "$CONSOLE_USER"
    record_created_account "$CONSOLE_USER"
  fi
  CONSOLE_RECORD=$(getent passwd "$CONSOLE_USER")
  [[ $CONSOLE_RECORD != *$'\n'* ]] || fatal "Ambiguous console account."
  IFS=: read -r CONSOLE_NAME _ CONSOLE_UID CONSOLE_GID _ CONSOLE_ACCOUNT_HOME CONSOLE_SHELL <<< "$CONSOLE_RECORD"
  [[ $CONSOLE_NAME == "$CONSOLE_USER" && $CONSOLE_UID =~ ^[0-9]+$ && $CONSOLE_GID =~ ^[0-9]+$ \
     && $CONSOLE_UID -gt 0 && $CONSOLE_GID -gt 0 && $CONSOLE_UID != "$TARGET_UID" && $CONSOLE_GID != "$TARGET_GID" \
     && $CONSOLE_ACCOUNT_HOME == "$CONSOLE_HOME" && $CONSOLE_SHELL == /usr/sbin/nologin ]] \
    || fatal "Unsafe existing console account; require a dedicated non-root, non-login brrdhouse identity."
  [[ $(id -u "$CONSOLE_USER") == "$CONSOLE_UID" && $(id -g "$CONSOLE_USER") == "$CONSOLE_GID" \
     && $(id -G "$CONSOLE_USER") == "$CONSOLE_GID" ]] || fatal "Console account must have no supplementary groups."
  [[ $(getent group "$CONSOLE_GID") == "$CONSOLE_USER:x:$CONSOLE_GID:" \
     && $(passwd -S "$CONSOLE_USER" | awk '{print $2}') == L ]] || fatal "Console group must be dedicated and account password locked."
  for subid_file in /etc/subuid /etc/subgid; do
    awk -F: -v user="$CONSOLE_USER" '$1==user && $2>=65536 && $3>=65536 {found=1} END {exit !found}' "$subid_file" \
      || fatal "No subordinate ID range for $CONSOLE_USER in $subid_file; fix host useradd allocation before retrying."
  done
  run loginctl enable-linger "$CONSOLE_USER"
  run systemctl start "user@$CONSOLE_UID.service"
  if ! console_run podman image exists "$CONSOLE_IMAGE"; then
    run_step images/pull-console console_run podman pull "$CONSOLE_IMAGE" || fatal "Console digest-pinned pull failed; deployment not changed."
  fi
  CONSOLE_REPO_DIGESTS=$(console_run podman image inspect "$CONSOLE_IMAGE" --format '{{range .RepoDigests}}{{println .}}{{end}}') \
    || fatal "Cannot inspect pinned console image."
  printf '%s\n' "$CONSOLE_REPO_DIGESTS" | grep -qxF "$CONSOLE_IMAGE" || fatal "Console RepoDigests mismatch; deployment not changed."
  public_directories /usr/local /usr/local/libexec
  # Exact shipped deploy/provision-status.sh; equality-checked by package tests.
  atomic_install 0755 root root "$STATUS_PROVISIONER" <<'STATUS_PROVISIONER_EOF'
#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
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
if [[ ${BRRDFEEDER_INSTALLER:-0} != 1 ]]; then
  printf 'Next: follow README unit setup to mount it in the engine and enable node.status_file.\n'
fi
STATUS_PROVISIONER_EOF
  run chmod 0755 "$STATUS_PROVISIONER"
  run chown root:root "$STATUS_PROVISIONER"
  BRRDFEEDER_INSTALLER=1 "$STATUS_PROVISIONER"
  say "Setting up the local console automatically."
  # Preserve all user settings except this package-owned status destination.
# --- persist a confirmed non-u-blox GPS declaration (idempotent) -------
# sensors.gps.usb_id records the operator's NMEA-confirmed claim; the udev
# render above reads it on re-runs (auto-update never touches this file).
if [[ -n $gps_declared_usb_id && $GPS_USB_ID_PRESENT -eq 1 && $DRY_RUN -eq 0 ]]; then
  run python3 - "$CONFIG_PATH" "$gps_declared_usb_id" <<'GPS_USBID_PY'
import sys, re, yaml
path, usb = sys.argv[1], sys.argv[2]
cfg = yaml.safe_load(open(path)) or {}
sensors = cfg.setdefault('sensors', {})
gps = sensors.setdefault('gps', {})
if gps.get('usb_id', '').lower() != usb:
    gps['usb_id'] = usb
    yaml.safe_dump(cfg, open(path, 'w'), sort_keys=False, default_flow_style=False)
    print('sensors.gps.usb_id recorded: '+usb)
else:
    print('sensors.gps.usb_id already '+usb)
GPS_USBID_PY
fi

  run python3 - "$CONFIG_PATH" "$STATUS_DIR/status.json" <<'STATUS_CONFIG_PY'
import os, pathlib, sys, tempfile, yaml
path = pathlib.Path(sys.argv[1])
class UniqueLoader(yaml.SafeLoader):
    pass
def mapping(loader, node, deep=False):
    result = {}
    for k, v in node.value:
        key = loader.construct_object(k, deep=deep)
        if key in result:
            raise ValueError(f'duplicate config key: {key}')
        result[key] = loader.construct_object(v, deep=deep)
    return result
UniqueLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, mapping)
if path.is_symlink():
    sys.exit('Refusing symlink config')
config = yaml.load(path.read_text(), Loader=UniqueLoader)
if not isinstance(config, dict) or not isinstance(config.get('node'), dict):
    sys.exit('Config requires a node mapping; no duplicate node section will be added')
config['node']['status_file'] = sys.argv[2]
fd, temp = tempfile.mkstemp(prefix='.config-status-', dir=path.parent)
try:
    with os.fdopen(fd, 'w') as output:
        yaml.safe_dump(config, output, sort_keys=False)
        output.flush()
        os.fchmod(output.fileno(), 0o644)
        os.fsync(output.fileno())
    os.replace(temp, path)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(directory)
    finally: os.close(directory)
finally:
    if os.path.exists(temp): os.unlink(temp)
STATUS_CONFIG_PY
fi

# ----------------------------------------------------------------------
# Step 2 — Clock hardening (chrony aggressive makestep)
# ----------------------------------------------------------------------
# Pi/CM4 have no battery RTC. Without this, a reboot can leave the clock
# in the past, preventing a secure connection. makestep 1.0 -1 = STEP
# (not slew) any offset >= 1.0s, unlimited times.
# ----------------------------------------------------------------------
gate clock "Step 2 — clock hardening (chrony makestep)"

public_directories /etc/chrony "$(dirname "$CHRONY_DROPIN")"
if [[ -e "$CHRONY_DROPIN" || -L "$CHRONY_DROPIN" ]]; then
  [[ -f $CHRONY_DROPIN && ! -L $CHRONY_DROPIN && $(stat -c %u "$CHRONY_DROPIN") == 0 ]] \
    && grep -qF '# BRRDfeeder clock correction' "$CHRONY_DROPIN" \
    || fatal "Unrecognised clock settings; ask the administrator to review them before retrying."
fi

NEW_CHRONY=$(cat <<'CEOF'
# BRRDfeeder clock correction — installed by brrdfeeder-install.sh.
# Aggressive makestep: STEP (not slew) clock if offset >= 1.0s, UNLIMITED
# times. Default "makestep 1 3" cannot recover from large RTC-less boot
# drift, which makes certificates appear not yet valid and prevents
# the engine from connecting securely.
makestep 1.0 -1
CEOF
)

if [[ -e "$LEGACY_CHRONY_DROPIN" || -L "$LEGACY_CHRONY_DROPIN" ]]; then
  [[ -f $LEGACY_CHRONY_DROPIN && ! -L $LEGACY_CHRONY_DROPIN && $(stat -c %u "$LEGACY_CHRONY_DROPIN") == 0 ]] \
    && grep -qF 'Pack-canonical chrony override' "$LEGACY_CHRONY_DROPIN" \
    || fatal "Unrecognised legacy clock settings; ask the administrator to review them before retrying."
fi
if [[ -f "$CHRONY_DROPIN" && ! -e "$LEGACY_CHRONY_DROPIN" ]] && diff -q <(echo "$NEW_CHRONY") "$CHRONY_DROPIN" >/dev/null 2>&1; then
  ok "chrony hardening already current"
else
  if [[ $DRY_RUN -eq 1 ]]; then
    say "[dry-run] would write $CHRONY_DROPIN + restart chrony"
  else
    printf '%s\n' "$NEW_CHRONY" | atomic_install 0644 root root "$CHRONY_DROPIN"
    run chmod 0644 "$CHRONY_DROPIN"
    if [[ -f $LEGACY_CHRONY_DROPIN ]]; then
      # Ownership marker checked above; remove only this obsolete package file.
      rm -- "$LEGACY_CHRONY_DROPIN"
      say "Migrated the package's legacy clock settings to $CHRONY_DROPIN."
    fi
    # chrony does not support `systemctl reload` — restart is required
    run systemctl restart chrony
    ok "chrony makestep hardening applied"
  fi
fi

# ----------------------------------------------------------------------
# Step 3 — udev rules (substrate-stable device naming)
# ----------------------------------------------------------------------
gate udev "Step 3 — udev rules"

# Verbatim aviary hardened block. Its historical UID 1001:20 comment is not
# allocation policy: this installer uses dynamic IDs plus numeric GroupAdd.
GPS_UDEV_RULES=$(for product in "${UBLOX_PRODUCTS[@]}"; do
  printf 'SUBSYSTEM=="tty", ATTRS{idVendor}=="%s", ATTRS{idProduct}=="%s", SYMLINK+="%s", GROUP="dialout", MODE="0660"\n' "$UBLOX_VENDOR" "$product" "$GPS_SYMLINK"
done)
# Operator-declared non-u-blox GPS (e.g. the Adafruit Ultimate GPS #746 via its
# CP2102N bridge). Rendered ONLY when the declaration passed the passive NMEA
# confirm this run — never for an unconfirmed or stale declaration.
GPS_DECLARED_RULE=""
if [[ -n $gps_declared_usb_id && $GPS_USB_ID_PRESENT -eq 1 ]]; then
  GPS_DECLARED_RULE=$(printf 'SUBSYSTEM=="tty", ATTRS{idVendor}=="%s", ATTRS{idProduct}=="%s", SYMLINK+="%s", GROUP="dialout", MODE="0660", ENV{CYBRRD_GPS_DECLARED}=="1"' "${gps_declared_usb_id%:*}" "${gps_declared_usb_id#*:}" "$GPS_SYMLINK")
  # Report — never delete or overwrite — a pre-existing third-party rule that
  # already claims this VID:PID (Synth amendment 3).
  local_style=""
  for rulefile in /etc/udev/rules.d/*.rules; do
    [[ -r $rulefile ]] || continue
    [[ $rulefile == "$UDEV_RULES_FILE" ]] && continue
    if grep -qE "ATTRS\{idVendor\}==\"${gps_declared_usb_id%:*}\".*ATTRS\{idProduct\}==\"${gps_declared_usb_id#*:}\"" "$rulefile" 2>/dev/null; then
      warn "Pre-existing udev rule $rulefile also matches GPS $gps_declared_usb_id. Left untouched (we never delete files we did not create); the operator may remove it to avoid duplicate /dev/$GPS_SYMLINK links. Known case: brrdg3s3's interim /etc/udev/rules.d/97-cybrrd-gps-local-adafruit.rules — remove by hand during the live test."
    fi
  done
fi
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

# Supported u-blox USB family; one connected GPS, no wildcard clone probing.
${GPS_UDEV_RULES}
# Operator-declared, NMEA-confirmed non-u-blox GPS (sensors.gps.usb_id).
${GPS_DECLARED_RULE}

# Nordic Semiconductor nRF52 Connectivity (BLE)
SUBSYSTEM=="tty", ATTRS{idVendor}=="${NORDIC_VENDOR}", ATTRS{idProduct}=="${NORDIC_PRODUCT}", SYMLINK+="${BLE_SYMLINK}", GROUP="dialout", MODE="0660"
KERNEL=="rfkill", SUBSYSTEM=="misc", GROUP="dialout", MODE="0660"
EOF
)

if [[ -f "$UDEV_RULES_FILE" ]] && diff -q <(echo "$NEW_UDEV_CONTENT") "$UDEV_RULES_FILE" >/dev/null 2>&1; then
  ok "udev rules already current"
else
  if [[ $DRY_RUN -eq 1 ]]; then
    say "[dry-run] would write:"
    echo "$NEW_UDEV_CONTENT" | sed 's/^/  /'
  else
    printf '%s\n' "$NEW_UDEV_CONTENT" | atomic_install 0644 root root "$UDEV_RULES_FILE"
    run chmod 0644 "$UDEV_RULES_FILE"
    ok "wrote $UDEV_RULES_FILE"
  fi
fi

run udevadm control --reload-rules
run udevadm trigger --subsystem-match=tty --action=change
run udevadm trigger --subsystem-match=misc --sysname-match=rfkill --action=change
run udevadm settle --timeout=5
ok "udev rules applied"

if [[ $DRY_RUN -eq 0 && $HAVE_UBLOX -eq 1 ]]; then
  if [[ -L "/dev/$GPS_SYMLINK" ]]; then
    ok "/dev/$GPS_SYMLINK -> $(readlink -f /dev/$GPS_SYMLINK)"
  else
    warn "/dev/$GPS_SYMLINK NOT created — re-plug u-blox or check rule syntax"
  fi
fi

# ----------------------------------------------------------------------
# Step 4 — BRRDfeeder MicroSD hardening (storage_class=ephemeral)
# ----------------------------------------------------------------------
gate storage "Step 4 — protecting the storage device"

STORAGE_CLASS="$(get_storage_class "$CONFIG_PATH")"
[[ -z "$STORAGE_CLASS" ]] && STORAGE_CLASS="persistent"
log_event DETAIL "config-declared node.storage_class=$STORAGE_CLASS"

if [[ "$STORAGE_CLASS" == "ephemeral" ]]; then
  JOURNALD_DROPIN_DIR="$(dirname "$JOURNALD_DROPIN")"
  public_directories /etc/systemd "$JOURNALD_DROPIN_DIR"

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
  # Marker-gated recognition: a file at either name that carries neither the
  # current nor the legacy marker is not ours and must stop the run, not be
  # overwritten or removed.
  if [[ -f "$JOURNALD_DROPIN_LEGACY" ]] && ! grep -qF -- "$JOURNALD_DROPIN_LEGACY_MARKER" "$JOURNALD_DROPIN_LEGACY"; then
    fatal "unrecognised legacy journald drop-in: $JOURNALD_DROPIN_LEGACY carries no BRRDfeeder marker; review it and remove it yourself"
  fi
  if [[ -f "$JOURNALD_DROPIN" ]] && ! grep -qF -- "$JOURNALD_DROPIN_MARKER" "$JOURNALD_DROPIN" && ! diff -q <(echo "$NEW_JOURNALD") "$JOURNALD_DROPIN" >/dev/null 2>&1; then
    fatal "unrecognised journald drop-in: $JOURNALD_DROPIN carries no BRRDfeeder marker; review it and remove it yourself"
  fi
  JOURNALD_CHANGED=0
  if [[ -f "$JOURNALD_DROPIN" ]] && diff -q <(echo "$NEW_JOURNALD") "$JOURNALD_DROPIN" >/dev/null 2>&1; then
    ok "journald drop-in already current"
  else
    if [[ $DRY_RUN -eq 1 ]]; then
      say "[dry-run] would write $JOURNALD_DROPIN"
    else
      printf '%s\n' "$NEW_JOURNALD" | atomic_install 0644 root root "$JOURNALD_DROPIN"
      run chmod 0644 "$JOURNALD_DROPIN"
      JOURNALD_CHANGED=1
      ok "journald Storage=volatile applied (logs in /run/log/journal tmpfs)"
    fi
  fi
  # Migration: a pre-rename install left 99-brrdfeeder-open.conf behind. The
  # marker check above proved it is ours; remove it only then, after the new
  # drop-in is in place.
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
  if [[ $JOURNALD_CHANGED -eq 1 ]]; then run systemctl restart systemd-journald; fi
  say "BRRDfeeder journald hardening active (engine logs land in RAM, NOT on SD)"
  say "Note: PCAP capture tmpfs is INSIDE the container (Tmpfs= directive in Quadlet)"
else
  ok "Persistent storage: keeping system logs on disk"
fi

# ----------------------------------------------------------------------
# Step 5 — Install rootful Quadlet at /etc/containers/systemd/
# ----------------------------------------------------------------------
# Install the exact D33 host helper; regression guard compares this body with
# the canonical source. The customer installer remains a self-contained file.
if [[ $DRY_RUN -eq 1 ]]; then
  say "[dry-run] would install $IDENTITY_INSTALL"
else
  public_directories /usr/local /usr/local/libexec
  atomic_install 0755 root root "$IDENTITY_INSTALL" <<'IDENTITY_SH_EOF'
#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Host-side Quadlet lifecycle helper. No tag inspection, runtime socket mount,
# credentials, last-known fallback, or authority granted to the engine.
set -euo pipefail
export LC_ALL=C
mode=${1:-}
state_dir=${2:-}
cidfile=${3:-}
invocation=${INVOCATION_ID:-}
[[ $invocation =~ ^[a-f0-9]{32}$ ]] || { echo '[identity] invalid invocation; unknown' >&2; exit 1; }
[[ $state_dir == /* && -d $state_dir && ! -L $state_dir ]] || exit 1
[[ $(stat -c %u -- "$state_dir") == "$EUID" ]] || exit 1
permissions=$(stat -c %a -- "$state_dir")
(( (8#$permissions & 0022) == 0 )) || exit 1

write_record() {
    local state=$1 cid=${2:-} digest=${3:-} tmp
    tmp=$(mktemp "$state_dir/.identity.XXXXXXXX")
    chmod 0644 "$tmp"
    printf '{"schema_version":1,"invocation_id":"%s","state":"%s"' "$invocation" "$state" > "$tmp"
    if [[ $state == known ]]; then
        printf ',"container_id":"%s","image_digest":"%s"' "$cid" "$digest" >> "$tmp"
    fi
    printf '}\n' >> "$tmp"
    mv -fT -- "$tmp" "$state_dir/identity.json"
}

case "$mode" in
    prepare) write_record pending ;;
    resolve)
        # Invalidate FIRST, but do not finish the engine's wait before inspect.
        # On any failure publish a terminal unknown; no known record survives.
        write_record pending
        trap 'rc=$?; if (( rc != 0 )); then write_record unverified || true; fi' EXIT
        [[ -f $cidfile && ! -L $cidfile ]] || exit 1
        cid=$(<"$cidfile")
        [[ $cid =~ ^[a-f0-9]{64}$ ]] || exit 1
        # Literal format, validated CID, timeout; never inspect a moving tag.
        inspected=$(timeout 5 podman container inspect --format '{{.Id}} {{.ImageDigest}} {{.State.Running}}' "$cid") || {
            echo '[identity] WARNING reason=running_identity_unverified; container inspection failed' >&2
            exit 1
        }
        read -r actual digest running extra <<< "$inspected"
        [[ $actual == "$cid" && $digest =~ ^sha256:[a-f0-9]{64}$ && $running == true && -z $extra ]] || {
            echo '[identity] WARNING reason=running_identity_unverified; no running-container manifest digest' >&2
            exit 1
        }
        write_record known "$actual" "$digest"
        printf '[identity] container_id=%s image_digest=%s source=podman_container_inspect\n' "$actual" "$digest"
        ;;
    *) echo 'usage: brrdfeeder-image-identity prepare|resolve RUNTIME_DIR [CIDFILE]' >&2; exit 2 ;;
esac
IDENTITY_SH_EOF
  run chmod 0755 "$IDENTITY_INSTALL"
  run chown root:root "$IDENTITY_INSTALL"
fi

# Host memory observations are separate from engine-writable status/state.
if [[ $DRY_RUN -eq 1 ]]; then
  say '[dry-run] would install the BRRDfeeder host memory observer and timer'
else
  atomic_install 0755 root root /usr/local/libexec/brrdfeeder-host-memory <<'MEMORY_HELPER_EOF'
#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Read-only host observations; only stop mode writes a durable OOM receipt.

No network, Podman socket, subprocesses, journal content, or engine-writable
input. Metrics are observations, not health or release authority.
"""
import fcntl
import json
import os
from pathlib import Path
import pwd
import re
import stat
import sys
import tempfile
import time

STATE = Path('/var/lib/brrdfeeder-memory')
RUNTIME = Path('/run/brrdfeeder-memory')
PROC = Path('/proc')
JOURNAL = Path('/run/log/journal')
MAX_BYTES = 16384


def read(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError('not regular')
        raw = stream.read(MAX_BYTES + 1)
        if len(raw) > MAX_BYTES:
            raise ValueError('oversize')
        return raw.decode('ascii')


def protected_dir(path):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o022:
        raise ValueError('unprotected directory')


def atomic(path, doc, durable=False):
    protected_dir(path.parent)
    fd, name = tempfile.mkstemp(prefix='.memory-', dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as stream:
            os.fchmod(stream.fileno(), 0o644)
            json.dump(doc, stream, sort_keys=True)
            stream.write('\n')
            stream.flush()
            if durable:
                os.fsync(stream.fileno())
        os.replace(name, path)
        if durable:
            directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def counter(state):
    try:
        doc = json.loads(read(state / 'events.json'))
    except FileNotFoundError:
        return {'schema_version': 1, 'memory_cap_events': 0, 'last_invocation': None}
    if (not isinstance(doc, dict) or type(doc.get('schema_version')) is not int
            or doc.get('schema_version') != 1 or type(doc.get('memory_cap_events')) is not int
            or not 0 <= doc['memory_cap_events'] < 2**64
            or not isinstance(doc.get('last_invocation'), str)
            or not re.fullmatch('[a-f0-9]{32}', doc['last_invocation'])):
        raise ValueError('invalid counter; refusing reset')
    return doc


def record_stop(state, result, invocation):
    protected_dir(state)
    if result != 'oom-kill':
        return False
    if not re.fullmatch('[a-f0-9]{32}', invocation):
        raise ValueError('invalid invocation')
    fd = os.open(state / '.lock', os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        doc = counter(state)
        if doc['last_invocation'] == invocation:
            return False
        if doc['memory_cap_events'] == 2**64 - 1:
            raise ValueError('counter exhausted')
        doc.update(memory_cap_events=doc['memory_cap_events'] + 1, last_invocation=invocation)
        atomic(state / 'events.json', doc, durable=True)
        print('BRRDfeeder memory event: service_result=oom-kill memory_cap_events=' +
              str(doc['memory_cap_events']) + ' invocation=' + invocation, flush=True)
        return True


def kib(text, key):
    matches = [line.split() for line in text.splitlines() if line.startswith(key + ':')]
    if len(matches) != 1 or len(matches[0]) != 3 or matches[0][2] != 'kB':
        return None
    value = matches[0][1]
    return int(value) * 1024 if value.isascii() and value.isdecimal() and int(value) < 2**54 else None


def console_rss(proc, uid):
    # Exact dedicated UID + systemd unit membership; never attribute another
    # user's similarly named process. Sum only the console executable, not conmon.
    total, found = 0, False
    for index, process in enumerate(proc.iterdir()):
        if index > 65536:
            return None
        if not process.name.isdecimal():
            continue
        try:
            if process.stat().st_uid != uid:
                continue
            group = read(process / 'cgroup')
            if not any(line.startswith('0::') and 'brrdhouse.service' in line[3:].split('/') for line in group.splitlines()):
                continue
            if read(process / 'comm').strip() != 'brrdhouse':
                continue
            value = kib(read(process / 'status'), 'VmRSS')
            if value is None:
                return None
            total += value
            found = True
        except (OSError, ValueError):
            continue
    return total if found else None


def journal_bytes(root):
    # Allocated blocks, not file contents or persistent /var/log/journal usage.
    if not root.is_dir() or root.is_symlink():
        return None
    total, entries = 0, 0
    def fail(error):
        raise error
    for directory, dirs, files in os.walk(root, followlinks=False, onerror=fail):
        dirs[:] = [name for name in dirs if not (Path(directory) / name).is_symlink()]
        for name in files:
            entries += 1
            if entries > 8192:
                return None
            info = (Path(directory) / name).lstat()
            if stat.S_ISREG(info.st_mode) and (name.endswith('.journal') or name.endswith('.journal~')):
                total += info.st_blocks * 512
    return total


def sample(proc, journal, state, console_uid):
    doc = {'schema_version': 1, 'boot_id': read(proc / 'sys/kernel/random/boot_id').strip(),
           'sampled_boottime_secs': int(time.clock_gettime(time.CLOCK_BOOTTIME))}
    try:
        meminfo = read(proc / 'meminfo')
        doc['host_mem_total_bytes'] = kib(meminfo, 'MemTotal')
        doc['host_mem_available_bytes'] = kib(meminfo, 'MemAvailable')
    except (OSError, ValueError):
        pass
    for key, observe in (
        ('console_rss_bytes', lambda: console_rss(proc, console_uid) if console_uid is not None else None),
        ('volatile_journal_bytes', lambda: journal_bytes(journal)),
        ('memory_cap_events', lambda: counter(state)['memory_cap_events']),
    ):
        try:
            doc[key] = observe()
        except (OSError, ValueError):
            print('BRRDfeeder memory observation unavailable: ' + key, file=sys.stderr)
    return {key: value for key, value in doc.items() if value is not None}


def main():
    if os.geteuid() != 0 or sys.argv[1:] not in (['sample'], ['stop']):
        raise ValueError('root-only sample|stop')
    protected_dir(STATE)
    protected_dir(RUNTIME)
    if sys.argv[1] == 'stop':
        record_stop(STATE, os.environ.get('SERVICE_RESULT', ''), os.environ.get('INVOCATION_ID', ''))
    try:
        uid = pwd.getpwnam('brrdhouse').pw_uid
    except KeyError:
        uid = None
    atomic(RUNTIME / 'host.json', sample(PROC, JOURNAL, STATE, uid))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        print('BRRDfeeder memory helper failed: ' + type(error).__name__, file=sys.stderr)
        raise SystemExit(1)
MEMORY_HELPER_EOF
  atomic_install 0644 root root /etc/systemd/system/brrdfeeder-memory.service <<'MEMORY_SERVICE_EOF'
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
[Unit]
Description=BRRDfeeder read-only host memory observations

[Service]
Type=oneshot
ExecStart=/usr/local/libexec/brrdfeeder-host-memory sample
StateDirectory=brrdfeeder-memory
StateDirectoryMode=0755
RuntimeDirectory=brrdfeeder-memory
RuntimeDirectoryMode=0755
RuntimeDirectoryPreserve=yes
TimeoutStartSec=10
MemoryMax=64M
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
RestrictAddressFamilies=AF_UNIX
MEMORY_SERVICE_EOF
  atomic_install 0644 root root /etc/systemd/system/brrdfeeder-memory.timer <<'MEMORY_TIMER_EOF'
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
[Unit]
Description=BRRDfeeder periodic host memory observations

[Timer]
OnBootSec=10
OnUnitInactiveSec=30
AccuracySec=1
Unit=brrdfeeder-memory.service

[Install]
WantedBy=timers.target
MEMORY_TIMER_EOF
fi

gate quadlets "Step 5 — system-mode Quadlet (rootful container, non-root engine)"

if [[ $DRY_RUN -eq 1 ]]; then
  say "[dry-run] would install GPS position seed helper; service waits for GPS, installer does not"
else
  atomic_install 0755 root root "$GPS_SEED" <<'GPS_SEED_EOF'
#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""BRRDfeeder GPS location seed; only the engine's systemd ExecStartPre may run it.

No network, receiver commands, fabricated position, or engine-status writes.
"""
import datetime
import fcntl
import json
import math
import os
from pathlib import Path
import re
import select
import signal
import socket
import stat
import subprocess
import tempfile
import termios
import time
import yaml

CONFIG = Path('/etc/brrdfeeder/config.yaml')
STARTUP = Path('/var/lib/brrdfeeder-status/startup.json')
UNIT = 'brrdfeeder-engine.service'
SYSFS_TTY = Path('/sys/class/tty')
LAST_LOG = None


class UniqueLoader(yaml.SafeLoader):
    pass


def mapping(loader, node, deep=False):
    result = {}
    for key, value in node.value:
        key = loader.construct_object(key, deep=deep)
        if key in result:
            raise ValueError('duplicate config key')
        result[key] = loader.construct_object(value, deep=deep)
    return result


UniqueLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, mapping)


def location_valid(value):
    if not isinstance(value, dict):
        return False
    coords = [value.get(k) for k in ('latitude', 'longitude', 'elevation_meters')]
    if not all(type(x) in (int, float) and math.isfinite(x) for x in coords):
        return False
    lat, lon, _ = coords
    return -90 <= lat <= 90 and -180 <= lon <= 180 and (lat != 0 or lon != 0)


def coordinate(value, hemisphere, latitude):
    digits, bound, hemispheres = (2, 90, 'NS') if latitude else (3, 180, 'EW')
    if hemisphere not in tuple(hemispheres) or not re.fullmatch(r'\d{' + str(digits+2) + r'}(?:\.\d{1,9})?', value):
        raise ValueError('invalid NMEA coordinate')
    degrees, minutes = int(value[:digits]), float(value[digits:])
    if minutes >= 60 or degrees > bound or (degrees == bound and minutes != 0):
        raise ValueError('out-of-range coordinate')
    return (degrees + minutes/60) * (-1 if hemisphere in 'SW' else 1)


def parse_fix(line):
    """Each sentence stands alone; never borrow coordinates from an old fix."""
    try:
        if len(line) > 128:
            return None
        text = line.decode('ascii').rstrip('\r\n')
        if not re.fullmatch(r'\$[A-Z]{2}(?:GGA|RMC),[ -~]+\*[0-9A-Fa-f]{2}', text):
            return None
        body, checksum = text[1:].rsplit('*', 1)
        value = 0
        for char in body:
            value ^= ord(char)
        if value != int(checksum, 16):
            return None
        fields = body.split(',')
        if not re.fullmatch(r'\d{6}(?:\.\d{1,9})?', fields[1]) or int(fields[1][:2]) > 23 or int(fields[1][2:4]) > 59 or int(fields[1][4:6]) > 60:
            return None
        if fields[0].endswith('GGA'):
            # 6/7/8 are estimated/manual/simulator, not a measured fix.
            if len(fields) != 15 or fields[6] not in ('1', '2', '3', '4', '5'):
                return None
            lat, lon = coordinate(fields[2], fields[3], True), coordinate(fields[4], fields[5], False)
            if fields[10] != 'M' or not re.fullmatch(r'-?\d{1,6}(?:\.\d{1,6})?', fields[9]):
                return None
            elevation = float(fields[9])
        else:
            # RMC contains no altitude: wait for GGA instead of inventing one.
            # The current engine requires finite elevation_meters as well.
            return None
        result = dict(latitude=lat, longitude=lon, elevation_meters=elevation)
        return result if location_valid(result) else None
    except (ValueError, IndexError, UnicodeError):
        return None


class NMEADiagnostics:
    """Bounded public observations; never raw sentences, identities or positions.

    Complete GSV cycles only. Latest signal cycle per talker (not a sum across
    signals); combined GN takes precedence over constellation-specific talkers.
    All observations expire after 15 seconds, independently of receiver chatter.
    """
    TALKERS = ('GP', 'GL', 'GA', 'GB', 'BD', 'GQ', 'GI', 'GN')

    def __init__(self):
        self.last = None
        self.gga = self.gsa = None
        self.pending, self.completed = {}, {}

    @staticmethod
    def integer(text, low, high):
        if not re.fullmatch(r'\d{1,3}', text) or not low <= int(text) <= high:
            raise ValueError('invalid diagnostic number')
        return int(text)

    def observe(self, line, now):
        try:
            if len(line) > 128:
                return
            text = line.decode('ascii').rstrip('\r\n')
            if not re.fullmatch(r'\$[A-Z]{5},[ -~]+\*[0-9A-Fa-f]{2}', text):
                return
            body, checksum = text[1:].rsplit('*', 1)
            check = 0
            for char in body:
                check ^= ord(char)
            if check != int(checksum, 16):
                return
            self.last = now  # checksum-valid framed NMEA, even without a fix
            f = body.split(',')
            talker, kind = f[0][:2], f[0][2:]
            if talker not in self.TALKERS:
                return
            if kind == 'GGA' and len(f) == 15:
                hdop = float(f[8]) if re.fullmatch(r'\d{1,3}(?:\.\d{1,6})?', f[8]) else None
                if hdop is not None and not 0 < hdop <= 999:
                    hdop = None
                self.gga = (now, self.integer(f[6], 0, 8), self.integer(f[7], 0, 99), hdop)
            elif kind == 'GSA' and len(f) in (18, 19) and f[1] in ('A', 'M'):
                self.gsa = (now, self.integer(f[2], 1, 3))
            elif kind == 'GSV':
                self.gsv(talker, f, now)
        except (ValueError, IndexError, UnicodeError):
            return  # Bad diagnostic data must not prevent measured-fix seeding.

    def gsv(self, talker, f, now):
        total, part, visible = self.integer(f[1], 1, 9), self.integer(f[2], 1, 9), self.integer(f[3], 0, 36)
        if part > total or total != max(1, (visible+3)//4):
            return
        count = min(4, max(0, visible-4*(part-1)))
        if len(f) not in (4+4*count, 5+4*count):
            return
        signal_id = f[-1] if len(f) == 5+4*count else ''
        if signal_id:
            self.integer(signal_id, 0, 15)
        snrs = []
        for start in range(4, 4+4*count, 4):
            self.integer(f[start], 1, 999)
            if f[start+1]: self.integer(f[start+1], 0, 90)
            if f[start+2]: self.integer(f[start+2], 0, 359)
            if f[start+3]: snrs.append(self.integer(f[start+3], 0, 99))
        if part == 1:
            self.pending[talker] = (now, total, visible, signal_id, 0, [])
        previous = self.pending.get(talker)
        if not previous or now-previous[0] > 15 or previous[1:4] != (total, visible, signal_id) or part != previous[4]+1:
            self.pending.pop(talker, None)
            return
        values = previous[5]+snrs  # <=36 values per one of eight fixed talkers
        if part == total:
            self.completed[talker] = (now, visible, values)
            self.pending.pop(talker, None)
        else:
            self.pending[talker] = (*previous[:4], part, values)

    def snapshot(self, now):
        data = dict(satellites_used=None, satellites_in_view=None, fix_quality=None,
                    fix_mode=None, hdop=None, snr_max_dbhz=None, snr_avg_dbhz=None,
                    nmea_age_secs=None if self.last is None else round(min(1e9, max(0, now-self.last)), 1))
        if self.gga and now-self.gga[0] <= 15:
            data['fix_quality'], data['satellites_used'], data['hdop'] = self.gga[1:]
        if self.gsa and now-self.gsa[0] <= 15:
            data['fix_mode'] = self.gsa[1]
        fresh = {k: v for k, v in self.completed.items() if now-v[0] <= 15}
        # GB and BD are aliases for BeiDou; never count both.
        if 'GB' in fresh: fresh.pop('BD', None)
        cycles = [fresh['GN']] if 'GN' in fresh else list(fresh.values())
        if cycles:
            data['satellites_in_view'] = sum(c[1] for c in cycles)
            snrs = [value for c in cycles for value in c[2]]
            if snrs:
                data['snr_max_dbhz'] = max(snrs)
                data['snr_avg_dbhz'] = round(sum(snrs)/len(snrs), 1)
        return data


def load_config():
    if CONFIG.is_symlink() or CONFIG.parent.is_symlink():
        raise ValueError('unsafe config path')
    info = CONFIG.stat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != 0 or info.st_mode & 0o022:
        raise ValueError('unsafe config ownership/mode')
    raw = CONFIG.read_bytes()
    if len(raw) > 1048576:
        raise ValueError('config too large')
    config = yaml.load(raw, Loader=UniqueLoader)
    if not isinstance(config, dict) or not isinstance(config.get('node'), dict):
        raise ValueError('node mapping required')
    existing = config['node'].get('location')
    if existing is not None and not location_valid(existing):
        raise ValueError('invalid existing location; refusing to replace operator data')
    return config, raw, info


def atomic_write(path, data, mode, uid, gid):
    fd, temporary = tempfile.mkstemp(prefix='.'+path.name+'-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as output:
            output.write(data)
            os.fchmod(output.fileno(), mode)
            os.fchown(output.fileno(), uid, gid)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def adapter_ids():
    """USB IDs only, no USB serial strings or receiver probing/auto-claiming."""
    result = set()
    for tty in SYSFS_TTY.glob('*/device'):
        for parent in [tty.resolve(), *tty.resolve().parents]:
            try:
                value = (parent/'idVendor').read_text().strip()+':'+(parent/'idProduct').read_text().strip()
            except OSError:
                continue
            if re.fullmatch(r'[0-9a-fA-F]{4}:[0-9a-fA-F]{4}', value):
                result.add(value.lower())
            break
        if len(result) >= 8:
            break
    return sorted(result)


def report(state, message, diagnostics=None):
    global LAST_LOG
    diagnostics = diagnostics if diagnostics is not None else NMEADiagnostics().snapshot(time.monotonic())
    message += '; '+ ' '.join(k+'='+('unknown' if v is None else str(v)) for k, v in diagnostics.items())
    adapters = adapter_ids()
    if state == 'gps-missing':
        message += '; serial adapter IDs seen: '+(', '.join(adapters) or 'none')
    now = time.monotonic()
    if LAST_LOG is None or now-LAST_LOG >= 60 or state == 'gps-fix':
        print(message, flush=True)
        LAST_LOG = now
    # Fixed public vocabulary only, never NMEA/device contents or node identity.
    data = dict(schema_version=1, state=state, written_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                status_interval_secs=5, gps=diagnostics, usb_adapter_ids=adapters)
    parent = STARTUP.parent.stat()
    if STARTUP.parent.is_symlink() or parent.st_mode & 0o022 or not stat.S_ISDIR(parent.st_mode):
        raise ValueError('unsafe startup status directory')
    encoded = (json.dumps(data, allow_nan=False)+'\n').encode()
    if len(encoded) >= 4096:
        raise ValueError('startup record exceeds size limit')
    atomic_write(STARTUP, encoded, 0o644, parent.st_uid, parent.st_gid)
    address = os.environ.get('NOTIFY_SOCKET')
    if address:
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as sock:
                sock.sendto(('STATUS='+message).encode(), '\0'+address[1:] if address.startswith('@') else address)
        except OSError:
            pass  # Journal and public sidecar still report the state.


def prestart_only():
    output = subprocess.check_output(['systemctl', 'show', UNIT, '-p', 'ControlPID', '--value'], text=True, timeout=5)
    if output.strip() != str(os.getpid()):
        raise ValueError('GPS seed may run only as the engine ExecStartPre')
    result = subprocess.run(['podman', 'container', 'exists', 'brrdfeeder-engine'], timeout=10)
    if result.returncode == 0:
        running = subprocess.check_output(['podman', 'inspect', '--format', '{{.State.Running}}', 'brrdfeeder-engine'], text=True, timeout=10)
        if running.strip() != 'false':
            raise ValueError('engine container still holds devices; refusing GPS access')
    elif result.returncode != 1:
        raise ValueError('cannot establish engine container is stopped')


def held_elsewhere(device):
    target = os.stat(device)
    for process in Path('/proc').iterdir():
        if not process.name.isdigit() or int(process.name) == os.getpid():
            continue
        try:
            for fd in (process/'fd').iterdir():
                try:
                    other = fd.stat()
                    if stat.S_ISCHR(other.st_mode) and other.st_rdev == target.st_rdev:
                        return True
                except FileNotFoundError:
                    pass
        except (FileNotFoundError, ProcessLookupError):
            pass
    return False  # Permission errors propagate: inability to inspect is not absence.


def open_gps(device, baud):
    if not stat.S_ISCHR(os.stat(device).st_mode):
        raise ValueError('GPS path must resolve to a character device')
    if held_elsewhere(device):
        raise BlockingIOError('GPS held by another process')
    fd = os.open(device, os.O_RDONLY | os.O_NONBLOCK | os.O_NOCTTY)
    try:
        fcntl.ioctl(fd, termios.TIOCEXCL)
        if held_elsewhere(device):
            raise BlockingIOError('GPS acquired concurrently')
        speed = getattr(termios, 'B'+str(baud), None)
        if speed is None:
            raise ValueError('unsupported GPS baud')
        options = termios.tcgetattr(fd)
        options[0:4] = [0, 0, termios.CS8 | termios.CREAD | termios.CLOCAL, 0]
        options[4:6] = [speed, speed]
        options[6][termios.VMIN], options[6][termios.VTIME] = 0, 0
        termios.tcsetattr(fd, termios.TCSANOW, options)
        termios.tcflush(fd, termios.TCIFLUSH)
        return fd
    except BaseException:
        close_gps(fd)
        raise


def close_gps(fd):
    try:
        fcntl.ioctl(fd, termios.TIOCNXCL)
    except OSError:
        pass  # Disconnected tty: still close our descriptor.
    finally:
        os.close(fd)


def device_matches(fd, device):
    """A stable symlink may now name another tty, even with a reused minor ID."""
    try:
        opened, current = os.fstat(fd), os.stat(device)
        return (stat.S_ISCHR(current.st_mode) and
                (opened.st_dev, opened.st_ino, opened.st_rdev) ==
                (current.st_dev, current.st_ino, current.st_rdev))
    except OSError:
        return False


def seed():
    prestart_only()
    config, raw, info = load_config()
    if location_valid(config['node'].get('location')):
        STARTUP.unlink(missing_ok=True)
        print('Position: existing real location preserved; GPS not opened.', flush=True)
        return
    gps = config.get('sensors', {}).get('gps', {})
    device, baud = gps.get('device', '/dev/cybrrd_gps'), gps.get('baud', 9600)
    if gps.get('required', True) is not True:
        raise ValueError('GPS optional but no configured position; expert override required')
    if not isinstance(device, str) or not device.startswith('/dev/'):
        raise ValueError('GPS device must be under /dev')
    while True:
        try:
            fd = open_gps(device, baud)
        except FileNotFoundError:
            report('gps-missing', 'GPS not detected: plug in the GPS')
            time.sleep(5)
            continue
        except (OSError, ValueError):
            report('gps-busy', 'GPS unavailable or held by another process; waiting without probing it')
            time.sleep(5)
            continue
        try:
            buffer = b''
            updated = 0
            diagnostics = NMEADiagnostics()
            last_data = time.monotonic()
            while True:
                if not device_matches(fd, device):
                    report('gps-busy', 'GPS disconnected or replaced; closing the old device and retrying')
                    break
                if time.monotonic()-last_data >= 15:
                    report('gps-busy', 'GPS receiver silent for 15 seconds; reopening it. Check receiver power, cable and baud setting')
                    break
                if time.monotonic()-updated >= 5:
                    report('gps-waiting', 'Waiting for GPS fix; place the antenna with a clear view of the sky',
                           diagnostics.snapshot(time.monotonic()))
                    updated = time.monotonic()
                try:
                    if not select.select([fd], [], [], 1)[0]:
                        continue
                    data = os.read(fd, 1024)
                except OSError:
                    report('gps-busy', 'GPS disconnected or serial I/O failed; closing the device and retrying')
                    break
                if not data:
                    report('gps-busy', 'GPS disconnected (end of data); closing the device and retrying')
                    break
                last_data = time.monotonic()
                for byte in data:
                    if byte == ord('$'):
                        buffer = b'$'
                    elif buffer:
                        buffer += bytes([byte])
                        if len(buffer) > 128:
                            buffer = b''
                        elif byte == 10:
                            diagnostics.observe(buffer, time.monotonic())
                            fix = parse_fix(buffer)
                            buffer = b''
                            if fix:
                                if not device_matches(fd, device):
                                    break  # Recheck/reopen in outer loop; never seed a replaced receiver's fix.
                                current, current_raw, current_info = load_config()
                                if current_raw != raw or current_info.st_ino != info.st_ino:
                                    raise ValueError('config changed while waiting; restart service to reload it')
                                current['node']['location'] = fix
                                atomic_write(CONFIG, yaml.safe_dump(current, sort_keys=False).encode(),
                                             stat.S_IMODE(info.st_mode), info.st_uid, info.st_gid)
                                report('gps-fix', 'Measured GPS fix acquired; preparing engine startup',
                                       diagnostics.snapshot(time.monotonic()))
                                STARTUP.unlink(missing_ok=True)
                                print('Position seeded from measured GPS fix; releasing GPS before engine startup.', flush=True)
                                return
        finally:
            close_gps(fd)
        time.sleep(1)


if __name__ == '__main__':
    def stop(_signal, _frame):
        raise SystemExit(0)  # Run serial/tempfile finally blocks on service stop.
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        seed()
    except Exception as error:
        # Do not echo parser/config contents into a diagnostic stream.
        print('GPS seed refused ('+type(error).__name__+'); check configuration and service/device ownership.', flush=True)
        raise SystemExit(1)
GPS_SEED_EOF
  run chmod 0755 "$GPS_SEED"
  run chown root:root "$GPS_SEED"
fi

# GPS_RUNTIME_INSTALL_BEGIN
if [[ $DRY_RUN -eq 0 ]]; then
  atomic_install 0755 root root /usr/local/libexec/brrdfeeder-gps-runtime <<'GPS_RUNTIME_EOF'
#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""BRRDfeeder GPS runtime transport: exact device grant, bounded hotplug recovery.

Never opens a serial port or discovers/claims adapters. A stable character inode
survives USB removal between ExecStartPre and Podman's device stat. /dev/null is
the absent transport: termios rejects it, so it cannot masquerade as live GPS.
"""
import fcntl
import datetime
import grp
import json
import os
import re
from pathlib import Path
import stat
import subprocess
import sys
import time
import yaml

ROOT = Path('/run/brrdfeeder-gps')
DEVICE = Path('/dev/cybrrd_gps')
CONFIG = Path('/etc/brrdfeeder/config.yaml')
UNIT = 'brrdfeeder-engine.service'
STATUS = Path('/var/lib/brrdfeeder-status')


def status():
    try:
        record = json.loads((STATUS/'startup.json').read_text())
        stamp = datetime.datetime.fromisoformat(record['written_at'])
        age = (datetime.datetime.now(datetime.timezone.utc)-stamp).total_seconds()
        if not 0 <= age <= 15:
            print('GPS startup status is stale; current GPS state unknown.')
            return
        state = record.get('state')
        if state == 'gps-missing':
            ids = record.get('usb_adapter_ids', [])
            ids = [s for s in ids[:8] if isinstance(s, str) and re.fullmatch(r'[0-9a-f]{4}:[0-9a-f]{4}', s)]
            print('No GPS device found; serial adapter IDs seen: '+(', '.join(ids) or 'none')+'.')
        elif state == 'gps-waiting':
            gps = record.get('gps', {})
            def number(key):
                value = gps.get(key)
                return str(value) if type(value) in (int, float) and 0 <= value <= 999 else 'unknown'
            print('GPS device present, no fix yet; satellites='+number('satellites_used')+' HDOP='+number('hdop')+'.')
        else:
            print('GPS first-fix startup is waiting; inspect the local console for details.')
    except FileNotFoundError:
        try:
            record = json.loads((STATUS/'status.json').read_text())
            stamp = datetime.datetime.fromisoformat(record['written_at'].replace('Z', '+00:00'))
            age = (datetime.datetime.now(datetime.timezone.utc)-stamp).total_seconds()
            if record.get('heartbeat', {}).get('os_clock_trusted') is False:
                print('Last engine report: saved position preserved; clock untrusted, report freshness unverified.'+
                      (' GPS not live.' if record.get('heartbeat', {}).get('gps', {}).get('state') != 'healthy' else ''))
            elif not 0 <= age <= 3*record['status_interval_secs']:
                print('Engine status is stale; current GPS state unknown.')
            elif record.get('heartbeat', {}).get('gps', {}).get('state') != 'healthy':
                print('Engine running on saved position: position preserved, GPS not live.')
            else:
                print('GPS reports a live fix; see the console for position and time trust.')
        except (OSError, ValueError, KeyError, TypeError):
            print('GPS status not available yet.')
    except (OSError, ValueError, KeyError, TypeError):
        print('GPS status is invalid; current GPS state unknown.')


def identity():
    try:
        value = DEVICE.stat()
        if stat.S_ISCHR(value.st_mode):
            return [value.st_dev, value.st_ino, value.st_rdev, value.st_ctime_ns]
    except OSError:
        pass
    return None


def prepare(state, current):
    temporary = ROOT/'device.new'
    temporary.unlink(missing_ok=True)
    if current is None:
        # Unlike a USB symlink, /dev/null cannot disappear on receiver removal.
        os.symlink('/dev/null', temporary)
    else:
        os.mknod(temporary, stat.S_IFCHR | 0o660, current[2])
        os.chown(temporary, 0, grp.getgrnam('dialout').gr_gid)
        os.chmod(temporary, 0o660)
    os.replace(temporary, ROOT/'device')
    state['mapped'] = current
    state['attempted'] = current


def should_restart(state, current, now):
    # Removal never restarts. Each new present identity gets at most one attempt;
    # flapping is bounded to three attempts per ten minutes and one per minute.
    attempts = [t for t in state.get('restarts', []) if 0 <= now-t < 600]
    state['restarts'] = attempts
    if current is None or current == state.get('mapped') or current == state.get('attempted'):
        return False
    if len(attempts) >= 3 or (attempts and now-attempts[-1] < 60):
        return False
    state['attempted'] = current
    attempts.append(now)
    return True


def save(state):
    temporary = ROOT/'state.new'
    with open(temporary, 'w', encoding='utf-8') as output:
        json.dump(state, output)
    os.replace(temporary, ROOT/'state.json')


def main():
    global DEVICE
    if sys.argv[1:] == ['status']:
        status()
        return
    os.umask(0o077)
    config = yaml.safe_load(CONFIG.read_text())
    device = config.get('sensors', {}).get('gps', {}).get('device', '/dev/cybrrd_gps')
    if not isinstance(device, str) or not device.startswith('/dev/'):
        raise ValueError('GPS device must be under /dev')
    DEVICE = Path(device)
    ROOT.mkdir(mode=0o700, exist_ok=True)
    info = ROOT.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o077:
        raise ValueError('unsafe GPS runtime directory')
    with open(ROOT/'lock', 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        path = ROOT/'state.json'
        state = json.loads(path.read_text()) if path.exists() else {}
        current = identity()
        if sys.argv[1:] == ['prepare']:
            prepare(state, current)
            save(state)
            print('GPS transport: '+('device present' if current else 'absent; position preserved, GPS not live'), flush=True)
        elif sys.argv[1:] == ['check']:
            # Never interrupt a first-fix ExecStartPre, intentional stop, update,
            # or failed service. --no-block avoids waiting on prepare's lock.
            active = subprocess.run(['systemctl', 'is-active', '--quiet', UNIT], timeout=5)
            if active.returncode == 0 and should_restart(state, current, time.monotonic()):
                save(state)  # Persist attempt before calling the restart effector.
                print('GPS appeared/replaced: requesting one bounded engine restart to attach the exact device', flush=True)
                subprocess.run(['systemctl', 'try-restart', '--no-block', UNIT], check=True, timeout=10)
        else:
            raise ValueError('expected prepare or check')


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print('GPS runtime refused ('+type(error).__name__+'); inspect configuration/runtime ownership.', flush=True)
        raise SystemExit(1)
GPS_RUNTIME_EOF
  atomic_install 0644 root root /etc/systemd/system/brrdfeeder-gps-runtime.service <<'GPS_RUNTIME_SERVICE_EOF'
# BRRDfeeder GPS runtime transport
[Unit]
Description=Attach a returned GPS with a bounded engine restart
[Service]
Type=oneshot
ExecStart=/usr/local/libexec/brrdfeeder-gps-runtime check
TimeoutStartSec=20
GPS_RUNTIME_SERVICE_EOF
  atomic_install 0644 root root /etc/systemd/system/brrdfeeder-gps-runtime.timer <<'GPS_RUNTIME_TIMER_EOF'
# BRRDfeeder GPS runtime transport
[Unit]
Description=Check for a returned configured GPS
[Timer]
OnBootSec=15s
OnUnitInactiveSec=5s
AccuracySec=1s
[Install]
WantedBy=timers.target
GPS_RUNTIME_TIMER_EOF
fi
# GPS_RUNTIME_INSTALL_END

QUADLET_DIR="$(dirname "$QUADLET_FILE")"
public_directories /etc/containers "$QUADLET_DIR"

NEW_QUADLET=$(cat <<EOF
# /etc/containers/systemd/brrdfeeder-engine.container
#
# cyBRRD BRRDfeeder engine — system-mode container.
# Installed by brrdfeeder-install.sh.
#
# Rootful is REQUIRED for the engine: Wi-Fi monitor mode + libpcap raw
# socket capture need CAP_NET_ADMIN/CAP_NET_RAW on the host network
# namespace, which rootless containers cannot grant even with
# --privileged.
#
# No :Z on Volume= mounts — SELinux relabel is a no-op on Debian-family
# hosts and unnecessary here; the canonical BRRDfeeder platform is Pi OS / Ubuntu.

[Unit]
Description=cyBRRD BRRDfeeder engine (containerized)
Documentation=https://github.com/cybrrd/brrdfeeder
After=network-online.target chrony.service
Wants=network-online.target
Wants=brrdfeeder-memory.service brrdfeeder-memory.timer
After=brrdfeeder-memory.service
StartLimitIntervalSec=300
StartLimitBurst=3

[Container]
Image=${CONTAINER_IMAGE}
ContainerName=brrdfeeder-engine
PodmanArgs=--cgroups=split
Volume=/run/brrdfeeder-memory:/run/brrdfeeder-memory:ro

# Rootful container, non-root process. IDs come from the dedicated host account.
User=${TARGET_UID}:${TARGET_GID}
GroupAdd=${DIALOUT_GID}

# Required for Wi-Fi monitor mode + libpcap raw sockets
Network=host
LogDriver=journald
DropCapability=ALL
AddCapability=CAP_NET_ADMIN CAP_NET_RAW CAP_SYS_TIME
# Do not set NoNewPrivileges: the engine acquires these exact file capabilities.
ReadOnly=true
Pull=never

# USB device passthrough — explicit src:dest preserves the udev
# symlink name inside the container (otherwise podman resolves the
# symlink to the underlying ttyACM* target).
AddDevice=/run/brrdfeeder-gps/device:/dev/${GPS_SYMLINK}:rw
Environment=BRRDFEEDER_GPS_TRANSPORT=/dev/${GPS_SYMLINK}
AddDevice=/dev/rfkill:/dev/rfkill:rw

# Bind-mount config + creds (read-only) at CANONICAL paths.
# /etc/brrdfeeder/secrets/ inside the image is 0755 since Containerfile
# commit a9e77aa — the pre-2026-06-08 bypass path is no longer needed.
Volume=${CONFIG_PATH}:/etc/brrdfeeder/config.yaml:ro
Volume=${SECRETS_DIR}:/etc/brrdfeeder/secrets:ro

# Persistent state (anti-replay watermark +
# the pending_update.json hand-off the host-updater consumes). Survives
# container restarts; no :Z (Debian-family, see header note).
Volume=${STATE_DIR}:/var/lib/brrdfeeder
Volume=${STATUS_DIR}:${STATUS_DIR}:rw

# Ephemeral PCAP capture surface (tmpfs in RAM, 50MB cap).
# mode=01777 = sticky-world-writable (like /tmp); container's
# runtime user can write. uid=/gid= are NOT valid podman tmpfs
# options (only size= and mode=).
Tmpfs=/etc/brrdfeeder/capture:size=50M,mode=01777

# Publish the observed airspace state once per second.
Environment=BRRDFEEDER_ENABLE_AIRSPACE_PUBLISHER=true
PodmanArgs=--cidfile=%t/%N.cid
Environment=BRRDFEEDER_IDENTITY_INVOCATION=\${INVOCATION_ID}
Volume=%t/brrdfeeder-identity:/run/brrdfeeder-identity:ro
Notify=false

[Service]
RuntimeDirectory=brrdfeeder-identity
MemoryAccounting=yes
MemoryMax=256M
MemorySwapMax=0
OOMPolicy=kill
ExecStopPost=/usr/local/libexec/brrdfeeder-host-memory stop
RuntimeDirectoryMode=0755
RuntimeDirectoryPreserve=no
ExecStartPre=${GPS_SEED}
ExecStartPre=/usr/local/libexec/brrdfeeder-gps-runtime prepare
ExecStartPre=-/usr/local/libexec/brrdfeeder-image-identity prepare %t/brrdfeeder-identity
ExecStartPost=-/usr/local/libexec/brrdfeeder-image-identity resolve %t/brrdfeeder-identity %t/%N.cid
Restart=always
RestartSec=5
TimeoutStartSec=infinity
NotifyAccess=all
TimeoutStopSec=10
KillMode=mixed
KillSignal=SIGTERM
LimitCORE=0

[Install]
WantedBy=multi-user.target
EOF
)

if [[ -f "$QUADLET_FILE" ]] && diff -q <(echo "$NEW_QUADLET") "$QUADLET_FILE" >/dev/null 2>&1; then
  ok "Quadlet already current — no change"
else
  if [[ -f "$QUADLET_FILE" ]]; then
    backup="${QUADLET_FILE}.bak.$(date +%Y%m%d-%H%M%S)"
    run atomic_install 0644 root root "$backup" "$QUADLET_FILE"
    say "backed up existing Quadlet to $backup"
  fi
  if [[ $DRY_RUN -eq 1 ]]; then
    say "[dry-run] would write Quadlet to $QUADLET_FILE"
  else
    printf '%s\n' "$NEW_QUADLET" | atomic_install 0644 root root "$QUADLET_FILE"
    run chmod 0644 "$QUADLET_FILE"
    ok "wrote $QUADLET_FILE"
  fi
fi

# The console reads only credential-free status and host memory observations.
# Its authoritative pin is root-owned; the user generator follows this root-owned link.
console_memory_policy
if [[ $DRY_RUN -eq 1 ]]; then
  say "[dry-run] would write rootless console Quadlet at $CONSOLE_QUADLET_FILE"
else
  CONSOLE_PORT=${CONSOLE_LISTEN##*:}
  UNIT_HOSTNAME=$(hostname -s)
  [[ $UNIT_HOSTNAME =~ ^[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?$ ]] || fatal "Hostname is not a safe DNS label for console Host validation."
  atomic_install 0644 root root "$CONSOLE_QUADLET_FILE" <<CONSOLE_QUADLET_EOF
[Unit]
Description=BRRDhouse intrinsic read-only LAN status console
After=network-online.target
Wants=network-online.target

[Container]
Image=${CONSOLE_IMAGE}
ContainerName=brrdhouse
Network=host
UserNS=keep-id:uid=65532,gid=65532
User=65532:65532
DropCapability=all
NoNewPrivileges=true
ReadOnly=true
ReadOnlyTmpfs=false
Pull=never
Volume=${STATUS_DIR}:${STATUS_DIR}:ro
Volume=/run/brrdfeeder-memory:/run/brrdfeeder-memory:ro
Exec=--listen=${CONSOLE_LISTEN} --allowed-hosts=${CONSOLE_LISTEN},${UNIT_HOSTNAME}:${CONSOLE_PORT},${UNIT_HOSTNAME}.local:${CONSOLE_PORT} --status-file=${STATUS_DIR}/status.json
PodmanArgs=--pids-limit=64${CONSOLE_MEMORY_ARGS}

[Service]
Restart=on-failure
RestartSec=5
TimeoutStopSec=10

[Install]
WantedBy=default.target
CONSOLE_QUADLET_EOF
  run chmod 0644 "$CONSOLE_QUADLET_FILE"
  run chown root:root "$CONSOLE_QUADLET_FILE"
  public_directories /etc/containers /etc/containers/systemd /etc/containers/systemd/users \
    "/etc/containers/systemd/users/$CONSOLE_UID"
  run ln -sfn "$CONSOLE_QUADLET_FILE" "/etc/containers/systemd/users/$CONSOLE_UID/brrdhouse.container"
fi

# ----------------------------------------------------------------------
# Step 5.5 — Install independent signed-release convergence and recovery (D44)
# ----------------------------------------------------------------------
# Node-driven pull ONLY. Blue cannot schedule updates. The timer fetches a
# signed package manifest and runs the health-gated host effector under one lock.
# D44's updater is an independent, hash-pinned host artifact. Neither workload
# image contains it; a broken image or unavailable registry cannot disable recovery.
gate updater "Step 5.5 — Self-Update setup"
if [[ $DRY_RUN -eq 1 ]]; then
  say "[dry-run] would verify/install standalone host updater sha256:$RELEASE_HELPER_SHA256 and local recovery + separate poll timers"
else
  UPDATE_STAGE=$(mktemp -d /tmp/brrdfeeder-update.XXXXXXXX)
  UPDATE_HELPER=/usr/local/libexec/brrdfeeder-release
  if [[ -e $UPDATE_HELPER || -L $UPDATE_HELPER ]]; then
    [[ -f $UPDATE_HELPER && ! -L $UPDATE_HELPER && $(stat -c %u "$UPDATE_HELPER") == 0 ]] || fatal 'Unsafe existing host updater'
    [[ -f /etc/brrdfeeder/.updater-helper.sha256 && ! -L /etc/brrdfeeder/.updater-helper.sha256 ]] || fatal 'Missing updater ownership receipt; explicit package reinstall required'
    [[ $(< /etc/brrdfeeder/.updater-helper.sha256) =~ ^[a-f0-9]{64}'  /usr/local/libexec/brrdfeeder-release'$ ]] || fatal 'Invalid updater ownership receipt'
    sha256sum --check --status /etc/brrdfeeder/.updater-helper.sha256 || fatal 'Existing updater differs from its recorded identity'
    "$UPDATE_HELPER" self-test || fatal 'Existing updater cannot start; boot recovery or package reinstall required'
    say 'Preserving independently updated host executable on re-run'
  else
    run curl --fail --silent --show-error --proto '=https' --tlsv1.2 --connect-timeout 10 --max-time 120 --max-filesize 33554432 --output "$UPDATE_STAGE/brrdfeeder-release" "$RELEASE_HELPER_URL"
    [[ $(sha256sum "$UPDATE_STAGE/brrdfeeder-release" | cut -d' ' -f1) == "$RELEASE_HELPER_SHA256" ]] || fatal 'Standalone updater SHA256 mismatch; refusing execution'
    public_directories /usr/local /usr/local/libexec
    # Receipt first: a crash can leave the verified hash without the executable,
    # but never an executable lacking provenance. Retry requires the same hash.
    if [[ -e /etc/brrdfeeder/.updater-helper.sha256 ]]; then
      [[ $(< /etc/brrdfeeder/.updater-helper.sha256) == "$RELEASE_HELPER_SHA256  $UPDATE_HELPER" ]] || fatal 'Mismatched pending updater ownership receipt'
    fi
    printf '%s  %s\n' "$RELEASE_HELPER_SHA256" "$UPDATE_HELPER" | atomic_install 0600 root root /etc/brrdfeeder/.updater-helper.sha256
    atomic_install 0755 root root "$UPDATE_HELPER" "$UPDATE_STAGE/brrdfeeder-release"
    "$UPDATE_HELPER" self-test || fatal 'Pinned host updater cannot start on this host'
  fi
  # GPS may not yet have seeded location. Project only updater fields rather
  # than requiring the capture engine's full position-bearing startup config.
  CONSOLE_BUILD=$(console_run podman image inspect --format '{{index .Labels "com.macawi.brrdhouse.build_seq"}}' "$CONSOLE_IMAGE")
  [[ $CONSOLE_BUILD =~ ^[1-9][0-9]*$ ]] || fatal "Self-Update requires a console image with a positive build sequence label."
  run python3 - "$CONFIG_PATH" "$UPDATE_STAGE/config.json" "$INSTALL_RING" "$CONSOLE_UID" "$CONSOLE_LISTEN" "$CONSOLE_BUILD" <<'RELEASE_CONFIG_PY'
import json, os, sys, tempfile, yaml
from pathlib import Path
path,out,requested,uid,listen,build=sys.argv[1:]
config=yaml.safe_load(Path(path).read_text())
node=config['node']
ring=requested or node.get('channel','general')
if ring not in ('dev','staging','general'): raise SystemExit('Invalid release ring; rc/stable/pilot are retired, use dev/staging/general')
if node.get('status_file') != '/var/lib/brrdfeeder-status/status.json': raise SystemExit('Updater requires the provisioned status source')
node['channel']=ring
if config.get('upward') is None: config['upward']={'spool_dir':'/var/lib/brrdfeeder/upward'}
if config['upward'].get('spool_dir')!='/var/lib/brrdfeeder/upward': raise SystemExit('Updater requires canonical upward spool for safe uninstall')
fd,tmp=tempfile.mkstemp(prefix='.release-config-',dir=str(Path(path).parent))
try:
    with os.fdopen(fd,'w') as f:
        prior=os.stat(path);os.fchmod(f.fileno(),prior.st_mode & 0o777);os.fchown(f.fileno(),prior.st_uid,prior.st_gid)
        f.write(yaml.safe_dump(config,sort_keys=False));f.flush();os.fsync(f.fileno())
    os.replace(tmp,path)
    directory=os.open(str(Path(path).parent),os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(directory)
    finally: os.close(directory)
finally:
    if os.path.exists(tmp): os.unlink(tmp)
Path(out).write_text(json.dumps(dict(node_id=node['id'],gps_required=config.get('sensors',{}).get('gps',{}).get('required',False),status_file=node['status_file'],upward_enabled=True,ring=ring,console_uid=int(uid),console_url='http://'+listen,console_build_seq=int(build))))
RELEASE_CONFIG_PY
  "$UPDATE_HELPER" install --config "$UPDATE_STAGE/config.json"
  rm -f "$UPDATE_STAGE/brrdfeeder-release" "$UPDATE_STAGE/config.json"
  rmdir "$UPDATE_STAGE"
  log_event UPDATE_STATUS ''
  log_event DETAIL "Update notifications require server-side permissions and streams; this installer does not provision them."
fi

# ----------------------------------------------------------------------
# Step 6 — image already verified before deployment writes (Step 1b)
# ----------------------------------------------------------------------
gate images-retained "Step 6 — retaining the immutable image checked in Step 1b (no tag refresh)"

# ----------------------------------------------------------------------
# Step 7 — Stop the legacy user-mode service if present
# ----------------------------------------------------------------------
gate migration "Step 7 — stop any existing legacy user-mode service"

OLD_USER_UNIT="${TARGET_HOME}/.config/systemd/user/brrdfeeder-engine.service"
OLD_USER_QUADLET="${TARGET_HOME}/.config/containers/systemd/brrdfeeder-engine.container"

if [[ -f "$OLD_USER_UNIT" ]] || [[ -f "$OLD_USER_QUADLET" ]]; then
  say "found legacy user-mode unit — migrating"
  if [[ $DRY_RUN -eq 0 ]]; then
    sudo -u "$TARGET_USER" \
      XDG_RUNTIME_DIR=/run/user/$TARGET_UID \
      DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$TARGET_UID/bus \
      systemctl --user stop brrdfeeder-engine.service 2>/dev/null || true
    sudo -u "$TARGET_USER" \
      XDG_RUNTIME_DIR=/run/user/$TARGET_UID \
      DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$TARGET_UID/bus \
      systemctl --user disable brrdfeeder-engine.service 2>/dev/null || true
    [[ -f "$OLD_USER_UNIT" ]] && rm -f "$OLD_USER_UNIT"
    [[ -f "$OLD_USER_QUADLET" ]] && rm -f "$OLD_USER_QUADLET"
    sudo -u "$TARGET_USER" \
      XDG_RUNTIME_DIR=/run/user/$TARGET_UID \
      DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$TARGET_UID/bus \
      systemctl --user daemon-reload 2>/dev/null || true
    ok "legacy user-mode unit removed"
  fi
else
  ok "no legacy user-mode unit found"
fi

# ----------------------------------------------------------------------
# Step 8 — Activate the system-mode service
# ----------------------------------------------------------------------
gate services "Step 8 — daemon-reload + start brrdfeeder-engine.service"

run systemctl daemon-reload
run systemctl enable --now brrdfeeder-gps-runtime.timer
# BLE ownership is established before queuing the engine. The helper saves the
# prior bluetoothd state for uninstall, and preserves explicit operator config.
if [[ $DRY_RUN -eq 1 ]]; then
  say "[dry-run] would install $BLUETOOTH_HELPER; inspect USB/config, add rid_ble only if absent and supported; record prior Bluetooth state then stop/disable/mask bluetoothd only if enabled"
else
  [[ ! -L "$BLUETOOTH_HELPER" && ! -L /usr/local/libexec ]] || fatal "Unsafe Bluetooth helper path."
  for helper_parent in /usr /usr/local /usr/local/libexec; do
    [[ ! -L "$helper_parent" && -d "$helper_parent" && $(stat -c %u "$helper_parent") == 0 ]] || fatal "Unsafe Bluetooth helper parent: $helper_parent"
    helper_mode=$(stat -c %a "$helper_parent")
    (( (8#$helper_mode & 0022) == 0 )) || fatal "Writable Bluetooth helper parent: $helper_parent"
  done
  if [[ -e "$BLUETOOTH_HELPER" ]]; then
    [[ -f "$BLUETOOTH_HELPER" && $(stat -c %u "$BLUETOOTH_HELPER") == 0 && $(stat -c %h "$BLUETOOTH_HELPER") == 1 ]] || fatal "Unsafe Bluetooth helper owner/type."
    helper_mode=$(stat -c %a "$BLUETOOTH_HELPER")
    (( (8#$helper_mode & 0022) == 0 )) || fatal "Writable Bluetooth helper."
  fi
  atomic_install 0755 root root "$BLUETOOTH_HELPER" <<'BLUETOOTH_EOF'
#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""BRRDfeeder Bluetooth ownership: offline, fixed paths, reversible service state."""
import json
import errno
import fcntl
import os
from pathlib import Path
import re
import stat
import socket
import struct
import subprocess
import sys
import tempfile
import time
import datetime as dt
from contextlib import contextmanager
import shutil

CONFIG = Path('/etc/brrdfeeder/config.yaml')
RECEIPT = Path('/etc/brrdfeeder/.bluetooth-prior.json')
USB = Path('/sys/bus/usb/devices')
HCI = Path('/sys/class/bluetooth')
DEVICES = Path('/sys/devices')
SUPPORTED = ('0bda:876e', '0bda:a728')  # finite allowlist; no HCI discovery
UNIT = 'bluetooth.service'
ENGINE = 'brrdfeeder-engine.service'
STATUS = Path('/var/lib/brrdfeeder-status/status.json')
STATES = ('enabled', 'enabled-runtime', 'disabled', 'static', 'indirect',
          'masked', 'masked-runtime', 'absent')


def say(message):
    print('[bluetooth] '+message, flush=True)


def safe(path, mode=None):
    for parent in [path, *path.parents]:
        if parent.is_symlink():
            raise ValueError('symlink in Bluetooth state path')
        if parent.exists():
            s = parent.stat()
            if s.st_uid != 0 or s.st_mode & 0o022:
                raise ValueError('Bluetooth state path must be root-owned and not group/world writable')
    if path.exists():
        s = path.stat()
        if not stat.S_ISREG(s.st_mode) or s.st_nlink != 1 or s.st_size > 1024*1024:
            raise ValueError('unsafe Bluetooth state file')
        if mode is not None and stat.S_IMODE(s.st_mode) != mode:
            raise ValueError('unsafe Bluetooth receipt mode')


def atomic(path, data, mode):
    safe(path)
    fd, tmp = tempfile.mkstemp(prefix='.bluetooth-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as out:
            os.fchmod(out.fileno(), mode)
            out.write(data)
            out.flush()
            os.fsync(out.fileno())
        os.replace(tmp, path)
        sync_parent(path)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def sync_parent(path):
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)


def ctl(*args):
    started = time.monotonic()
    result = subprocess.run(['systemctl', *args], text=True, capture_output=True, timeout=30)
    # Do not dump arbitrary service output/config into the installer log.
    say('[CMD] systemctl '+' '.join(args)+' rc='+str(result.returncode)+f' dur={time.monotonic()-started:.3f}s')
    if result.returncode:
        raise ValueError('systemctl failed; previous-state receipt retained for retry')
    return result.stdout


def service_state(unit=UNIT):
    pairs = dict(line.split('=', 1) for line in ctl('show', unit,
        '--property=LoadState,UnitFileState,ActiveState').splitlines() if '=' in line)
    load, enabled, active = (pairs.get(k) for k in ('LoadState', 'UnitFileState', 'ActiveState'))
    if load == 'not-found' and active == 'inactive': enabled = 'absent'
    if load not in ('loaded', 'masked', 'not-found') or enabled not in STATES or active not in ('active', 'inactive'):
        raise ValueError('unrecognised/transitional Bluetooth service state; inspect systemctl before retrying')
    return dict(version=1, unit=UNIT, enabled=enabled, active=(active == 'active'))


def receipt():
    safe(RECEIPT, 0o600)
    if not RECEIPT.exists(): return None
    value = json.loads(RECEIPT.read_bytes())
    if (not isinstance(value, dict) or set(value) != {'version','unit','enabled','active'} or
        type(value['version']) is not int or value['version'] != 1 or value['unit'] != UNIT or
        value['enabled'] not in STATES or type(value['active']) is not bool or
        (value['enabled'] in ('absent','masked','masked-runtime') and value['active'])):
        raise ValueError('invalid Bluetooth ownership receipt')
    return value


def container_gone():
    # Read-only, current privileges only; never sudo or initialize a new store.
    if not shutil.which('podman'): return
    try:
        result = subprocess.run(['podman', 'container', 'exists', 'brrdfeeder-engine'],
                                capture_output=True, timeout=5)
    except (OSError, subprocess.TimeoutExpired):
        say('container absence unavailable; using systemd state and MainPID=0')
        return
    if result.returncode == 1: return
    if result.returncode == 125:
        say('container store unavailable at current privileges; using systemd state and MainPID=0')
        return
    raise ValueError('engine container remains or absence is ambiguous; refusing Bluetooth ownership change')


def set_service_state(prior):
    if service_state()['enabled'] != 'absent': ctl('stop', UNIT)
    ctl('unmask', UNIT)
    ctl('--runtime', 'unmask', UNIT)
    state = prior['enabled']
    if state in ('enabled','enabled-runtime'):
        ctl(*(['--runtime'] if state.endswith('-runtime') else []), 'enable', UNIT)
    elif state in ('masked','masked-runtime'):
        ctl(*(['--runtime'] if state.endswith('-runtime') else []), 'mask', UNIT)
    elif state == 'disabled': ctl('disable', UNIT)
    if prior['active']: ctl('start', UNIT)
    if service_state() != prior: raise ValueError('Bluetooth restore did not reproduce prior state; receipt retained')


@contextmanager
def stop_engine():
    """Stop for a handover; a failed handover restores a previously running node."""
    output = ctl('show', ENGINE, '--property=LoadState', '--value').strip()
    if output not in ('loaded', 'not-found'):
        raise ValueError('cannot establish engine service identity for BLE takeover')
    state = ctl('show', ENGINE, '--property=ActiveState', '--value').strip()
    if state not in ('active', 'activating', 'inactive', 'failed'):
        raise ValueError('engine already transitional; refusing Bluetooth ownership change')
    restart = output == 'loaded' and state in ('active', 'activating')
    attempted = False
    snapshot = None
    try:
        if output == 'loaded':
            attempted = True  # even a failed/timeout stop may have stopped the unit
            ctl('stop', ENGINE)
        stopped = ctl('show', ENGINE, '--property=ActiveState', '--value').strip()
        pid = ctl('show', ENGINE, '--property=MainPID', '--value').strip()
        if stopped not in ('inactive', 'failed') or pid != '0':
            raise ValueError('engine did not stop; refusing Bluetooth ownership change')
        container_gone()
        if stopped == 'failed':
            ctl('reset-failed', ENGINE)
            if ctl('show', ENGINE, '--property=ActiveState', '--value').strip() != 'inactive':
                raise ValueError('engine failed state did not clear; refusing Bluetooth ownership change')
        # Capture after stopping the GPS waiter so its last position seed survives.
        safe(CONFIG)
        config = (CONFIG.read_bytes(), stat.S_IMODE(CONFIG.stat().st_mode)) if CONFIG.exists() else None
        snapshot = (service_state(), config, RECEIPT.read_bytes() if RECEIPT.exists() else None)
        yield
    except Exception as error:
        rollback_failed = False
        if snapshot is not None:
            prior, config, saved_receipt = snapshot
            try:
                if config is not None and CONFIG.read_bytes() != config[0]: atomic(CONFIG, *config)
            except Exception:
                rollback_failed = True
            try:
                if service_state() != prior: set_service_state(prior)
                if saved_receipt is not None and not RECEIPT.exists(): atomic(RECEIPT, saved_receipt, 0o600)
            except Exception:
                rollback_failed = True
        if attempted and restart:
            try:
                ctl('start', ENGINE)
                active = ctl('show', ENGINE, '--property=ActiveState', '--value').strip()
                pid = ctl('show', ENGINE, '--property=MainPID', '--value').strip()
                if active != 'active' or not pid.isdecimal() or int(pid) == 0:
                    raise ValueError('engine recovery unverified')
                say('refusal recovery: engine restarted to prior running state')
            except Exception:
                raise ValueError('engine STOPPED or recovery unverified; recover with: sudo systemctl start brrdfeeder-engine.service; inspect Bluetooth state and retained receipt') from error
        if rollback_failed:
            raise ValueError('Bluetooth rollback incomplete; inspect retained receipt and service/config state; recover engine with: sudo systemctl start brrdfeeder-engine.service') from error
        raise


def inventory():
    found = []
    for device in sorted(USB.glob('*')):
        try:
            identity = (device/'idVendor').read_text().strip().lower()+':'+(device/'idProduct').read_text().strip().lower()
        except OSError: continue
        if not re.fullmatch(r'[0-9a-f]{4}:[0-9a-f]{4}', identity): continue
        if identity in SUPPORTED:
            found.append(identity)
            say('supported RID.BLE adapter: Realtek USB '+identity)
    if not found: say('no supported RID.BLE adapter found (automatic selection supports '+', '.join(SUPPORTED)+'); no HCI probe performed')
    return found


def load_config():
    import yaml  # apply only; uninstall/check need Python's standard library only
    class Unique(yaml.SafeLoader): pass
    def mapping(loader, node):
        result = {}
        for key, value in node.value:
            key = loader.construct_object(key)
            if key in result: raise ValueError('duplicate config key')
            result[key] = loader.construct_object(value)
        return result
    Unique.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, mapping)
    safe(CONFIG)
    raw = CONFIG.read_bytes()
    doc = yaml.load(raw, Loader=Unique)
    if not isinstance(doc, dict): raise ValueError('config must be a mapping')
    sensors = doc.setdefault('sensors', {})
    if not isinstance(sensors, dict): raise ValueError('sensors must be a mapping')
    return doc, sensors, raw


def enabled_config(sensors, found):
    changed = False
    if 'rid_ble' not in sensors and found:
        if len(found) > 1: raise ValueError('multiple supported BLE adapters; configure one explicit BD_ADDR before retrying')
        sensors['rid_ble'] = dict(enabled=True, unblock_rfkill=True, adapter=dict(usb_id=found[0]))
        changed = True
    ble = sensors.get('rid_ble', {})
    if not isinstance(ble, dict) or set(ble)-{'enabled','adapter','unblock_rfkill','quiet_window_s'}:
        raise ValueError('invalid sensors.rid_ble config keys')
    enabled = ble.get('enabled', False)
    if type(enabled) is not bool or type(ble.get('unblock_rfkill',False)) is not bool:
        raise ValueError('rid_ble switches must be booleans')
    if enabled:
        a = ble.get('adapter', {})
        if not isinstance(a, dict) or set(a)-{'usb_id','bd_addr'}: raise ValueError('invalid BLE adapter identity')
        identities = [(k,v) for k,v in a.items() if v is not None]
        if len(identities) != 1: raise ValueError('enabled BLE needs exactly one adapter identity')
        k,v = identities[0]
        pattern = r'[0-9a-fA-F]{4}:[0-9a-fA-F]{4}' if k == 'usb_id' else r'(?:[0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}'
        if not isinstance(v,str) or not re.fullmatch(pattern,v): raise ValueError('invalid BLE adapter identity')
        quiet = ble.get('quiet_window_s',30)
        if type(quiet) is not int or not 1 <= quiet <= 2**64-1: raise ValueError('invalid BLE quiet window')
        if 'unblock_rfkill' not in ble:
            ble['unblock_rfkill'] = True
            changed = True
    return enabled, changed


def selected_controller(adapter):
    """Only sysfs identity, never inquiry, discovery or a guessed hci index."""
    matches = []
    for path in sorted(HCI.glob('hci*')):
        if not re.fullmatch(r'hci[0-9]{1,5}',path.name): continue
        index = int(path.name[3:])
        if index >= 65535: continue
        try:
            device = (path/'device').resolve(strict=True)
            if not device.is_relative_to(DEVICES): continue
            usb = None
            for parent in [device,*device.parents]:
                if not parent.is_relative_to(DEVICES): break
                if (parent/'idVendor').is_file() and (parent/'idProduct').is_file():
                    usb = (parent/'idVendor').read_text().strip().lower()+':'+(parent/'idProduct').read_text().strip().lower()
                    break
            if usb not in SUPPORTED: continue
            if adapter.get('usb_id') is not None:
                if usb != adapter['usb_id'].lower(): continue
            elif (path/'address').read_text().strip().lower() != adapter['bd_addr'].lower(): continue
            matches.append((index, device, path.stat().st_ino))
        except OSError: continue
    if len(matches) != 1:
        raise ValueError('BLE DOWN refused: sysfs must identify exactly one configured supported USB controller (0bda:876e, 0bda:a728); check driver/connection/identity')
    return matches[0]


def controller_down(adapter):
    target = selected_controller(adapter)
    engine = ctl('show', ENGINE, '--property=ActiveState', '--value').strip()
    daemon = service_state()
    if engine != 'inactive' or daemon['active'] or daemon['enabled'] not in ('masked','masked-runtime'):
        raise ValueError('BLE DOWN refused: engine must be inactive and bluetoothd inactive/masked')
    # Linux v6.18 include/net/bluetooth/hci_sock.h ABI: _IOW('H',202,int),
    # _IOR('H',211,int); hci_dev_info is 92 bytes, flags at offset 16 (u32).
    # No RESET, inquiry, HCI commands, discovery, device-UP or rfkill writes.
    index = target[0]
    try:
        with socket.socket(socket.AF_BLUETOOTH, socket.SOCK_RAW | socket.SOCK_CLOEXEC, socket.BTPROTO_HCI) as channel:
            if selected_controller(adapter) != target:
                raise ValueError('BLE DOWN refused: controller changed before ioctl')
            try: fcntl.ioctl(channel.fileno(), 0x400448ca, index)  # HCIDEVDOWN only
            except OSError as error:
                if error.errno != errno.EALREADY: raise
            info = bytearray(92)
            struct.pack_into('@H',info,0,index)
            fcntl.ioctl(channel.fileno(), 0x800448d3, info, True)  # HCIGETDEVINFO read
            actual = struct.unpack_from('@H',info,0)[0]
            flags = struct.unpack_from('@I',info,16)[0]
            if actual != index or flags & 1 or selected_controller(adapter) != target:
                raise ValueError('BLE DOWN verification failed: controller is UP or identity changed')
    except OSError as error:
        raise ValueError('BLE DOWN failed: HCIDEVDOWN/HCIGETDEVINFO errno='+str(error.errno)+'; check adapter/driver/permissions') from None
    say(f'BLE DOWN verified: hci{index} adapter={adapter} HCIGETDEVINFO HCI_UP=0; no discovery/reset performed')


def restore(dry=False):
    prior = receipt()
    if prior is None:
        say('nothing to do: Bluetooth ownership receipt absent; service left unchanged')
        return
    say(('WOULD restore' if dry else 'restoring')+' bluetooth.service enabled='+prior['enabled']+' active='+str(prior['active']).lower())
    if dry: return
    with stop_engine():
        set_service_state(prior)
        RECEIPT.unlink()
        sync_parent(RECEIPT)
        say('restored Bluetooth prior state; removed '+str(RECEIPT))


def apply(dry=False):
    import yaml
    found = inventory()
    doc, sensors, raw = load_config()
    enabled, changed = enabled_config(sensors, found)
    prior = receipt()  # validate even if config now disables BLE
    say('rid_ble enabled='+str(enabled).lower()+' config='+('add-missing-key' if changed else 'preserve-explicit-or-absent'))
    if not enabled:
        if prior: restore(dry)
        else: say('bluetooth.service unchanged: RID.BLE disabled')
        return
    if prior is None: prior = service_state()
    if prior['enabled'] in ('absent','masked','masked-runtime') and prior['active']:
        raise ValueError('inconsistent Bluetooth service state; refusing takeover')
    if dry:
        say('WOULD stop engine, add missing rid_ble key if needed, record '+str(RECEIPT)+', stop/disable/mask bluetooth.service, then targeted HCIDEVDOWN and read-verify; existing explicit config preserved')
        return
    with stop_engine():
        # Re-read after stopping the GPS waiter. Preserve a fix it may just have seeded.
        doc, sensors, raw = load_config()
        enabled, changed = enabled_config(sensors, found)
        if not enabled: raise ValueError('BLE config changed during takeover; retry')
        if changed:
            if CONFIG.read_bytes() != raw: raise ValueError('config changed during migration; retry')
            atomic(CONFIG, yaml.safe_dump(doc, sort_keys=False).encode(), stat.S_IMODE(CONFIG.stat().st_mode))
            say('added missing RID.BLE config/unblock_rfkill policy; explicit values preserved')
        if not RECEIPT.exists():
            atomic(RECEIPT, (json.dumps(prior,sort_keys=True)+'\n').encode(), 0o600)
        # Receipt precedes every service mutation; retries never overwrite history.
        if prior['enabled'] != 'absent': ctl('stop', UNIT)
        if prior['enabled'] in ('enabled','enabled-runtime'):
            ctl(*(['--runtime'] if prior['enabled'].endswith('-runtime') else []), 'disable', UNIT)
        ctl('mask', UNIT)  # persist across reboot, even if the prior mask was runtime-only
        now = service_state()
        if now['active'] or now['enabled'] not in ('masked','masked-runtime'):
            raise ValueError('Bluetooth ownership not established; receipt retained')
        controller_down(sensors['rid_ble']['adapter'])
        say('RID.BLE owns Bluetooth; bluetoothd stopped/masked and adapter DOWN verified. Engine still checks rfkill and exclusive USER bind.')


def main():
    if os.geteuid() != 0: raise ValueError('run Bluetooth ownership helper as root')
    args = sys.argv[1:]
    if args == ['check']: receipt()
    elif args in (['apply'], ['apply','--dry-run']): apply(len(args)==2)
    elif args in (['restore'], ['restore','--dry-run']): restore(len(args)==2)
    elif args == ['inventory']: inventory()
    elif args == ['health-summary']: health_summary()
    else: raise ValueError('expected apply|restore [--dry-run], check, inventory, or health-summary')


def summarize_health(status, journal, now=None):
    """D17 has rfkill inventory, NOT BLE lifecycle health. Use current-run journal.

    Freshness proves the engine is still reporting; preflight rfkill observations
    cannot prove unblock success or a Healthy radio. Only engine transitions can.
    """
    now = time.time() if now is None else now
    try:
        interval = status['status_interval_secs']
        written = dt.datetime.fromisoformat(status['written_at'].replace('Z','+00:00'))
        if (type(interval) is not int or interval <= 0 or written.tzinfo is None or
                not 0 <= now-written.timestamp() <= 3*interval):
            return 'RID.BLE Unknown: engine status stale or invalid; not verified healthy'
        if status.get('schema_version') != 1:
            return 'RID.BLE Unknown: unsupported engine status schema'
    except (KeyError, ValueError, TypeError, AttributeError):
        return 'RID.BLE Unknown: engine status missing or invalid; not verified healthy'
    result = 'RID.BLE Unknown: no lifecycle state in current engine invocation; not verified healthy'
    for line in journal.splitlines():
        if line.strip() == '[rid_ble] disabled': result = 'RID.BLE Disabled (engine configuration)'
        match = re.search(r'\[rid_ble\] state=(Initializing|Healthy|Degraded|Failed) detail=(.*)',line)
        if match:
            state, detail = match.groups()
            # Escape all control characters; never print arbitrary terminal commands.
            reason = json.dumps(detail[:500], ensure_ascii=True)
            result = f'RID.BLE {state}: engine detail={reason}'
    return result


def health_summary():
    try:
        unit = dict(line.split('=',1) for line in ctl('show', ENGINE,
            '--property=ActiveState,InvocationID').splitlines() if '=' in line)
        invocation = unit.get('InvocationID','')
        if unit.get('ActiveState') != 'active' or not re.fullmatch('[0-9a-f]{32}',invocation):
            say('RID.BLE Unknown: engine not active or invocation unavailable; not verified healthy'); return
        with STATUS.open('rb') as stream: data = stream.read(1024*1024+1)
        if len(data) > 1024*1024: raise ValueError('oversize engine status')
        status = json.loads(data)
        journal = subprocess.run(['journalctl','--no-pager','-o','cat','-n','200',
            '_SYSTEMD_INVOCATION_ID='+invocation], capture_output=True, text=True, timeout=5)
        if journal.returncode: raise ValueError('current engine journal unavailable')
        # A concurrent restart must not let the previous invocation look healthy.
        if ctl('show', ENGINE, '--property=InvocationID', '--value').strip() != invocation:
            raise ValueError('engine restarted during health check')
        say(summarize_health(status, journal.stdout))
    except (OSError, ValueError, TypeError, AttributeError, subprocess.TimeoutExpired):
        say('RID.BLE Unknown: current engine status/journal unavailable; not verified healthy')


if __name__ == '__main__':
    try: main()
    except Exception as error:
        # YAML errors can quote credentials: report only a bounded class, no raw YAML.
        say('REFUSED: '+(str(error) if isinstance(error,ValueError) and not type(error).__module__.startswith('yaml') else type(error).__name__))
        sys.exit(1)
BLUETOOTH_EOF
  run chmod 0755 "$BLUETOOTH_HELPER"
  run chown root:root "$BLUETOOTH_HELPER"
  run_step bluetooth/ownership python3 "$BLUETOOTH_HELPER" apply
fi
# NOTE: do NOT `systemctl enable` — Quadlet-generated units are transient
# and reject enable ("Unit ... is transient or generated"). Boot-start is
# already handled by the Quadlet's [Install] WantedBy=multi-user.target,
# which the generator honors. `restart` (not `start`) so re-runs of this
# script pick up Quadlet changes.
INSTALL_ENGINE_RESTART_AT=$(date +%s)
run systemctl --no-block restart brrdfeeder-engine.service
ok "brrdfeeder-engine.service startup queued; service will wait for GPS if position is absent"
run console_run systemctl --user daemon-reload
run console_run systemctl --user restart brrdhouse.service
ok "rootless brrdhouse.service started"

# ----------------------------------------------------------------------
# Step 9 — Verify
# ----------------------------------------------------------------------
gate verification "Step 9 — verification"

if [[ $DRY_RUN -eq 1 ]]; then
  say "[dry-run] skipping live verification"
  exit 0
fi

console_run systemctl --user is-active --quiet brrdhouse.service || fatal "Console user service is not active."
console_run podman ps --filter name=^brrdhouse$ --format '{{.Names}}' | grep -qx brrdhouse \
  || fatal "Console container is not running."
curl --noproxy '*' --fail --silent --show-error --max-time 5 --retry 4 --retry-connrefused \
  --retry-delay 1 --retry-max-time 5 "http://$CONSOLE_LISTEN/" >/dev/null \
  || fatal "Console is not serving its configured HTTP address."
[[ $(stat -c '%u:%g:%a' "$STATUS_DIR") == "$TARGET_UID:$TARGET_GID:755" ]] \
  || fatal "Status directory is not service-owned 0755."
ok "Console serving at http://$CONSOLE_LISTEN/ (page reports missing/stale engine status honestly)"

# The supervisor owns the bounded startup wait and LAST terminal screen, after
# command completion/progress cleanup. No successful result before that check.
INSTALL_CONTEXT=$(python3 - "$CONFIG_PATH" "$CONSOLE_LISTEN" "$INSTALL_ENGINE_RESTART_AT" <<'INSTALL_CONTEXT_PY'
import json, sys, yaml
with open(sys.argv[1]) as stream: config = yaml.safe_load(stream)
print(json.dumps(dict(node_id=config['node']['id'], console_url='http://'+sys.argv[2]+'/', started_at=int(sys.argv[3]))))
INSTALL_CONTEXT_PY
) || fatal "Cannot read node identity for the final installation check."
log_event INSTALL_CONTEXT "$INSTALL_CONTEXT"
