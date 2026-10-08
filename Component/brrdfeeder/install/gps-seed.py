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
