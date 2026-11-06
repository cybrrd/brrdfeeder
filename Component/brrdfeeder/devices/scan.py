#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Metadata-only USB inventory; never invoke a runtime qualification/claim path."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import re
import sys

ROLES = {'gps', 'rid-ble', 'wifi-capture', 'legacy-serial'}
PATHS = {'gps-runtime', 'bluetooth-hotplug', 'capture-watchdog', 'legacy-unqualified'}

def catalog(raw):
    if len(raw) > 65536:
        raise ValueError('registry too large')
    doc = json.loads(raw)
    if type(doc.get('schema_version')) is not int or doc.get('schema_version') != 1 or not isinstance(doc.get('catalog_version'), str):
        raise ValueError('unsupported registry schema')
    entries = doc.get('profiles')
    if not isinstance(entries, list) or not 1 <= len(entries) <= 128:
        raise ValueError('invalid registry profiles')
    seen = set()
    for entry in entries:
        key = entry['id']
        if not isinstance(key, str) or not re.fullmatch(r'[a-z0-9-]{1,64}', key) or key in seen:
            raise ValueError('invalid/duplicate profile')
        seen.add(key)
        if entry['role'] not in ROLES or entry['qualification_path'] not in PATHS:
            raise ValueError('invalid registry role/path')
        if entry['qualification'] not in ('candidate', 'tested', 'recommended', 'deprecated', 'blocked'):
            raise ValueError('invalid qualification')
        if not isinstance(entry['label'], str) or not 1 <= len(entry['label']) <= 160:
            raise ValueError('invalid label')
        if not entry['usb_ids'] or not all(isinstance(s,str) and re.fullmatch(r'[0-9a-f]{4}:[0-9a-f]{4}', s) for s in entry['usb_ids']):
            raise ValueError('invalid USB identity')
        if not entry['drivers'] or not all(isinstance(s,str) and re.fullmatch(r'[a-z0-9_]{1,40}', s) for s in entry['drivers']):
            raise ValueError('invalid driver')
    return doc


def text(path):
    # Only fixed sysfs attributes; no device nodes, status/config or process FDs.
    try:
        return path.read_text()[:64].strip()
    except OSError:
        return ''


def scan(sysfs, raw):
    doc = catalog(raw)
    devices = sysfs/'devices'
    usb = sysfs/'bus/usb/devices'
    if not usb.is_dir():
        raise ValueError('USB inventory unavailable')
    rows = []
    partial = False
    links = sorted(usb.iterdir())
    partial = len(links) > 512
    for link in links[:512]:
        try:
            node = link.resolve(strict=True)
            if not node.is_relative_to(devices.resolve()) or not re.fullmatch(r'[0-9a-f]{4}', text(node/'idVendor')):
                continue
            vidpid = text(node/'idVendor')+':'+text(node/'idProduct')
            if not re.fullmatch(r'[0-9a-f]{4}:[0-9a-f]{4}', vidpid):
                partial = True
                continue
            drivers = set()
            endpoints = set()
            for interface in sorted(node.glob('*:*')):
                for group in ('tty','net','bluetooth'):
                    parent = interface/group
                    if parent.is_dir():
                        endpoints.update(p.name for p in sorted(parent.iterdir())[:128])
                endpoints.update(p.name for p in interface.glob('ttyUSB*'))
                endpoints.update(p.name for p in interface.glob('ttyACM*'))
                driver = interface/'driver'
                if driver.is_symlink():
                    drivers.add(driver.resolve().name)
            matches = [e for e in doc['profiles'] if vidpid in e['usb_ids']]
            profiles = []
            for e in matches:
                state = 'candidate' if drivers.intersection(e['drivers']) else ('driver-mismatch' if drivers else 'driver-missing')
                if e['qualification'] in ('blocked','deprecated'):
                    state = e['qualification']
                profiles.append(dict(id=e['id'],role=e['role'],label=e['label'],state=state,
                                     qualification=e['qualification'],qualification_path=e['qualification_path']))
            rows.append(dict(instance=link.name,usb_id=vidpid,drivers=sorted(drivers),endpoints=sorted(endpoints),profiles=profiles,
                             state='recognized' if profiles else 'unsupported',
                             capability='unverified',ownership='unverified',health='unverified'))
        except OSError:
            partial = True
    counts = {}
    for row in rows:
        for p in row['profiles']:
            if p['state'] == 'candidate': counts[p['role']] = counts.get(p['role'],0)+1
    return dict(schema_version=1,catalog_version=doc['catalog_version'],catalog_sha256=hashlib.sha256(raw).hexdigest(),
                observed_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),partial=partial,
                snapshot_only=True,ambiguous_roles=sorted(k for k,v in counts.items() if v>1),devices=rows)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,prog='brrdfeeder devices')
    parser.add_argument('command',choices=('scan','add','remove','replace'))
    parser.add_argument('--json',action='store_true')
    args = parser.parse_args(argv)
    if args.command != 'scan':
        parser.error('device changes are not implemented; scan is read-only')
    try:
        raw = globals().get('REGISTRY_BYTES')
        if raw is None: raw = Path(__file__).with_name('supported-devices.json').read_bytes()
        result = scan(Path('/sys'), raw)
    except (OSError,ValueError,KeyError,TypeError):
        print('Device scan refused: registry or USB metadata unavailable/invalid.',file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(result,sort_keys=True))
    else:
        print('Metadata snapshot only: capability, ownership and health are unverified.')
        for row in result['devices']:
            descriptions = ', '.join(p['role']+':'+p['state'] for p in row['profiles']) or 'unsupported'
            print(row['instance']+' '+row['usb_id']+' '+descriptions)
        if result['ambiguous_roles']: print('Ambiguous roles: '+', '.join(result['ambiguous_roles'])+'; no device selected.')
        if result['partial']: print('Inventory changed or incomplete; scan again before planning any action.')
    return 0

if __name__ == '__main__':
    sys.exit(main())
