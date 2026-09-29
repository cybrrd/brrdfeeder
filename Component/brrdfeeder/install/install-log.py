#!/usr/bin/env python3
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
ENDPOINTS = [('ingest.cybrrd.com', 4222), ('hospitality.cybrrd.com', 443),
             ('ghcr.io', 443), ('globe.cybrrd.com', 443)]
ANSI = re.compile(r'\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-_]')


def now():
    return dt.datetime.now(dt.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')


class Redactor:
    def __init__(self):
        self.known = {}
        self.nats = False

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


def environment(probe=False):
    result = [f'collected={now()}', f'host={os.uname().nodename}', f'uid={os.geteuid()}']
    commands = [('uname', ['uname', '-a']), ('kernel', ['uname', '-r']), ('arch', ['uname', '-m']),
                ('memory', ['free', '-m']), ('disk', ['df', '-h']),
                ('podman', ['podman', '--version']),
                ('clock', ['timedatectl']), ('usb', ['lsusb']),
                ('net', ['ip', '-brief', 'address']), ('route', ['ip', 'route']),
                ('phys', ['iw', 'phy']), ('wireless_devices', ['iw', 'dev'])]
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
        except OSError:
            self.fallback()
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
                if info.st_uid not in (0, os.geteuid()) or (info.st_mode & 0o022 and not sticky_root):
                    raise OSError('unsafe log parent')
        parent.mkdir(mode=0o750, parents=True, exist_ok=True)

    def fallback(self):
        try:
            self.fd, path = tempfile.mkstemp(prefix=f'brrdfeeder-{self.mode}-{self.run_id}-', suffix='.log', dir='/tmp')
            self.path = Path(path)
            os.fchmod(self.fd, 0o640)
        except OSError:
            self.fd = None
            self.path = Path('/tmp/LOG-UNAVAILABLE')
        if not self.warned:
            print(f'[brrdfeeder-install] logging degraded to {self.path}' +
                  (' (no writable log file; retain terminal output)' if self.fd is None else ''), flush=True)
            self.warned = True

    def write(self, safe):
        self.history.append(safe)
        if self.fd is None:
            return
        try:
            data = safe.encode('utf-8', 'replace')
            while data:
                data = data[os.write(self.fd, data):]
        except OSError:
            os.close(self.fd)
            self.fallback()
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


def final_install_screen(context, readiness, log_path, run_id):
    title = ('BRRDfeeder is installed and running.' if readiness['running'] else
             'BRRDfeeder is installed. Engine startup is still waiting for GPS.')
    return '\n'.join([title, 'Node ID: '+context['node_id'], 'Console: '+context['console_url'],
                      *(key+': '+readiness['states'][key] for key in SYSTEMS),
                      'Self-Update checks signed updates automatically and restores the previous version if an update fails its health checks.',
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
                pass  # No display: child keeps the manual-creds fallback.
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
    quiet = '--no-verbose' in args and (mode == 'install' or '--uninstall' in args)
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
