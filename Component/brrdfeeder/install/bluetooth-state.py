#!/usr/bin/env python3
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

CONFIG = Path('/etc/brrdfeeder/config.yaml')
RECEIPT = Path('/etc/brrdfeeder/.bluetooth-prior.json')
USB = Path('/sys/bus/usb/devices')
HCI = Path('/sys/class/bluetooth')
DEVICES = Path('/sys/devices')
SUPPORTED = '0bda:876e'  # finite automatic RID.BLE selection; no HCI discovery
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


def stop_engine():
    # Stop the waiter too, so no concurrent seed can overwrite the config migration.
    output = ctl('show', ENGINE, '--property=LoadState', '--value').strip()
    if output == 'not-found': return
    if output != 'loaded': raise ValueError('cannot establish engine service identity for BLE takeover')
    ctl('stop', ENGINE)
    if ctl('show', ENGINE, '--property=ActiveState', '--value').strip() != 'inactive':
        raise ValueError('engine did not stop; refusing Bluetooth ownership change')


def inventory():
    found = []
    for device in sorted(USB.glob('*')):
        try:
            identity = (device/'idVendor').read_text().strip().lower()+':'+(device/'idProduct').read_text().strip().lower()
        except OSError: continue
        if not re.fullmatch(r'[0-9a-f]{4}:[0-9a-f]{4}', identity): continue
        if identity == SUPPORTED:
            found.append(identity)
            say('supported RID.BLE adapter: Realtek USB '+identity)
    if not found: say('no supported RID.BLE adapter found (automatic selection supports '+SUPPORTED+'); no HCI probe performed')
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
        sensors['rid_ble'] = dict(enabled=True, unblock_rfkill=True, adapter=dict(usb_id=SUPPORTED))
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
            if usb != SUPPORTED: continue
            if adapter.get('usb_id') is not None:
                if usb != adapter['usb_id'].lower(): continue
            elif (path/'address').read_text().strip().lower() != adapter['bd_addr'].lower(): continue
            matches.append((index, device, path.stat().st_ino))
        except OSError: continue
    if len(matches) != 1:
        raise ValueError('BLE DOWN refused: sysfs must identify exactly one configured supported USB controller (0bda:876e); check driver/connection/identity')
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
                raise ValueError('BLE DOWN verification failed: controller is UP or identity changed; engine not restarted')
    except OSError as error:
        raise ValueError('BLE DOWN failed: HCIDEVDOWN/HCIGETDEVINFO errno='+str(error.errno)+'; check adapter/driver/permissions; engine not restarted') from None
    say(f'BLE DOWN verified: hci{index} USB={SUPPORTED} HCIGETDEVINFO HCI_UP=0; no discovery/reset performed')


def restore(dry=False):
    prior = receipt()
    if prior is None:
        say('nothing to do: Bluetooth ownership receipt absent; service left unchanged')
        return
    say(('WOULD restore' if dry else 'restoring')+' bluetooth.service enabled='+prior['enabled']+' active='+str(prior['active']).lower())
    if dry: return
    stop_engine()
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
    stop_engine()
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
