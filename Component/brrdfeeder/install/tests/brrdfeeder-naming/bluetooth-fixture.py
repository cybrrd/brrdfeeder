#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Test-only boundaries for disposable installer containers, never host effectors.

Load the installer's actual embedded helper, retaining inventory, config,
service-state validation and ownership transactions. Replace only sysfs and
systemd/controller I/O. The legacy rfkill helper uses its existing path flags.
No production environment switch or installed-file rewrite is introduced.
"""
import importlib.util
import json
import os
from pathlib import Path
import shlex
import sys


def executable(path, source):
    path.write_text(source)
    path.chmod(0o755)


def setup(binary, kit=None, case='no-adapter'):
    assert os.geteuid() == 0 and Path('/run/.containerenv').exists()
    root = binary/'bluetooth-fixture'
    root.mkdir(exist_ok=True)
    for name in ('usb', 'hci', 'devices', 'rfkill', 'saved'):
        (root/name).mkdir(exist_ok=True)
    (root/'case').write_text(case)
    (root/'calls').write_text('')
    if case != 'no-adapter':
        usb = root/'devices/usb1'
        usb.mkdir(exist_ok=True)
        (usb/'idVendor').write_text('0bda\n')
        (usb/'idProduct').write_text('876e\n')
        (root/'usb/1-1').symlink_to(usb)
        hci = root/'hci/hci7'
        hci.mkdir()
        (hci/'device').symlink_to(usb)
        (hci/'address').write_text('02:00:00:00:00:07\n')
        rf = root/'rfkill/rfkill7'
        rf.mkdir()
        (rf/'type').write_text('bluetooth\n')
        (rf/'soft').write_text('1\n')
        (root/'saved/fixture:bluetooth').write_text('1\n')
    # Absolute interpreter for the fixture prevents recursion through this shim.
    bridge = shlex.quote(str(Path(__file__).resolve()))
    executable(binary/'python3', '#!/bin/sh\n'
        'if [ "$1" = /usr/local/libexec/brrdfeeder-bluetooth ]; then\n'
        f'  exec /usr/bin/python3 {bridge} {shlex.quote(str(root))} "$@"\n'
        'fi\nexec /usr/bin/python3 "$@"\n')
    if kit is not None:
        helper = kit/'brrdfeeder-rfkill-boot-state.sh'
        original = kit/'rfkill-original.sh'
        helper.rename(original)
        command = binary/'fixture-rfkill'
        if case != 'rfkill-missing':
            executable(command, '#!/usr/bin/python3\n'
                'import sys\nfrom pathlib import Path\n'
                f'root = Path({str(root)!r})\n'
                'assert sys.argv[1:] == ["unblock", "bluetooth"]\n'
                'with (root/"calls").open("a") as log: log.write("rfkill unblock bluetooth\\n")\n'
                '(root/"rfkill/rfkill7/soft").write_text("0\\n")\n')
        executable(helper, '#!/bin/sh\nexec bash '+shlex.quote(str(original))+
            ' --sysfs-root '+shlex.quote(str(root/'rfkill'))+
            ' --state-dir '+shlex.quote(str(root/'saved'))+
            ' --rfkill-command '+shlex.quote(str(command))+' "$@"\n')
    return root


def dispatch(root, source, args):
    assert os.geteuid() == 0 and Path('/run/.containerenv').exists()
    assert source == '/usr/local/libexec/brrdfeeder-bluetooth'
    spec = importlib.util.spec_from_file_location('installed_bluetooth', source)
    # Extensionless executable: use the explicit source loader.
    if spec is None:
        from importlib.machinery import SourceFileLoader
        spec = importlib.util.spec_from_loader('installed_bluetooth', SourceFileLoader('installed_bluetooth', source))
    ble = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(ble)
    ble.USB, ble.HCI, ble.DEVICES = (root/name for name in ('usb', 'hci', 'devices'))
    case = (root/'case').read_text()
    state = dict(enabled='enabled', active='activating' if case == 'transitional' else 'active')

    def record(value):
        with (root/'calls').open('a') as log: log.write(value+'\n')

    def ctl(*argv):
        record('systemctl '+' '.join(argv))
        if argv[0] == 'show':
            if argv[1] == ble.UNIT:
                return 'LoadState=loaded\nUnitFileState='+state['enabled']+'\nActiveState='+state['active']+'\n'
            assert argv[1] == ble.ENGINE, argv
            return {'--property=LoadState': 'not-found', '--property=ActiveState': 'inactive',
                    '--property=MainPID': '0'}[argv[2]]+'\n'
        assert argv[-1] == ble.UNIT, argv
        if argv[0] == 'stop': state['active'] = 'inactive'
        elif argv[0] in ('disable', 'mask'): state['enabled'] = {'disable':'disabled', 'mask':'masked'}[argv[0]]
        else: raise AssertionError('unexpected service mutation: '+repr(argv))
        return ''

    def controller_down(adapter):
        assert ble.selected_controller(adapter)[0] == 7
        assert state == dict(enabled='masked', active='inactive')
        prior = json.loads(ble.RECEIPT.read_text())
        assert prior['active'] and prior['enabled'] == 'enabled'
        record('controller DOWN hci7 (synthetic)')

    def forbidden(*unused, **kwargs):
        raise AssertionError('real controller boundary reached')

    ble.ctl = ctl
    ble.controller_down = controller_down
    ble.socket.socket = forbidden
    sys.argv = [source, *args]
    try:
        ble.main()
    except ValueError as error:
        ble.say('REFUSED: '+str(error))
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(dispatch(Path(sys.argv[1]), sys.argv[2], sys.argv[3:]))
