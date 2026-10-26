#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Boot repair fixtures: no host boot file or service is modified."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[3]
spec=importlib.util.spec_from_file_location('controller',HERE/'memory-controller.py')
controller=importlib.util.module_from_spec(spec)
spec.loader.exec_module(controller)


class Controller(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='memory-controller-')
        self.root=Path(self.temp.name)
        self.cmd=self.root/'boot/firmware/cmdline.txt'
        self.cmd.parent.mkdir(parents=True)
        self.original=b'console=serial0,115200 root=PARTUUID=fixture  cgroup_disable=memory quiet\n'
        self.cmd.write_bytes(self.original)
        self.model=self.root/'sys/firmware/devicetree/base/model'
        self.model.parent.mkdir(parents=True)
        self.model.write_bytes(b'Raspberry Pi 5 Model B Rev 1.0\0')

    def tearDown(self):
        self.temp.cleanup()

    def test_idempotent_backed_up_and_byte_exact_restore(self):
        self.assertIn('REBOOT REQUIRED',controller.edit(self.root))
        expected=b'console=serial0,115200 root=PARTUUID=fixture quiet\n'
        self.assertEqual(self.cmd.read_bytes(),expected)
        backup=self.cmd.with_name('cmdline.txt.brrdfeeder-memory.bak')
        self.assertEqual(backup.read_bytes(),self.original)
        before=(self.cmd.stat().st_mtime_ns,backup.stat().st_mtime_ns)
        controller.edit(self.root)
        self.assertEqual((self.cmd.stat().st_mtime_ns,backup.stat().st_mtime_ns),before)
        self.assertIn('REBOOT REQUIRED',controller.edit(self.root,restore=True))
        self.assertEqual(self.cmd.read_bytes(),self.original)
        controller.edit(self.root,restore=True)
        self.assertEqual(backup.read_bytes(),self.original)

    def test_non_pi_and_unrelated_controllers_are_preserved(self):
        self.model.write_bytes(b'Unrelated arm64 board')
        controller.edit(self.root)
        self.assertEqual(self.cmd.read_bytes(),self.original)
        self.assertEqual(controller.enabled_cmdline(b'a cgroup_disable=cpu,memory,pids b\n'),b'a cgroup_disable=cpu,pids b\n')
        self.assertEqual(controller.enabled_cmdline(b'a  cgroup_disable=cpu b\n'),b'a  cgroup_disable=cpu b\n')

    def test_external_edit_prevents_destructive_restore(self):
        controller.edit(self.root)
        self.cmd.write_bytes(self.cmd.read_bytes().rstrip()+b' unrelated=keep\n')
        before=self.cmd.read_bytes()
        with self.assertRaisesRegex(ValueError,'changed since repair'):
            controller.edit(self.root,restore=True)
        self.assertEqual(self.cmd.read_bytes(),before)

    def test_symlink_cmdline_and_backup_refused(self):
        target=self.root/'outside';target.write_bytes(self.original)
        self.cmd.unlink();self.cmd.symlink_to(target)
        with self.assertRaises(OSError):controller.edit(self.root)
        self.assertEqual(target.read_bytes(),self.original)
        self.cmd.unlink();self.cmd.write_bytes(self.original)
        self.cmd.with_name('cmdline.txt.brrdfeeder-memory.bak').symlink_to(target)
        with self.assertRaises(OSError):controller.edit(self.root)
        self.assertEqual(self.cmd.read_bytes(),self.original)

    def test_check_requires_live_controller_and_actual_finite_cap(self):
        base=self.root/'sys/fs/cgroup';base.mkdir(parents=True)
        (base/'cgroup.controllers').write_text('cpu io\n')
        with self.assertRaisesRegex(ValueError,'absent'):controller.check(self.root)
        (base/'cgroup.controllers').write_text('cpu io memory\n')
        cap=base/'system.slice/brrdfeeder-engine.service/memory.max';cap.parent.mkdir(parents=True)
        for value in ('max','536870912','0'):
            cap.write_text(value+'\n')
            with self.assertRaisesRegex(ValueError,'NOT verified'):controller.check(self.root)
        cap.write_text('268435456\n')
        self.assertIn('cap enforced',controller.check(self.root))

    def test_shipping_copies_and_restart_contract(self):
        installer=(ROOT/'Component/brrdfeeder/install/brrdfeeder-install.sh').read_text()
        embedded=installer.split("<<'MEMORY_CONTROLLER_EOF'\n",1)[1].split('\nMEMORY_CONTROLLER_EOF',1)[0]+'\n'
        self.assertEqual(embedded,(HERE/'memory-controller.py').read_text())
        for path in ('Component/aviary/deploy/quadlet/brrdfeeder-engine.container',
                     'Component/aviary/release-system/units/brrdfeeder-updater.service',
                     'Component/aviary/deploy/host-updater/brrdfeeder-updater.service'):
            unit=(ROOT/path).read_text()
            self.assertIn('StartLimitIntervalSec=0\n',unit)
            self.assertNotIn('StartLimitBurst=3',unit)
        self.assertIn('Restart=always\n',(ROOT/'Component/aviary/deploy/quadlet/brrdfeeder-engine.container').read_text())
        self.assertNotIn('systemctl reboot',embedded)
        self.assertIn('/usr/local/libexec/brrdfeeder-memory-controller enable || fatal',installer)
        self.assertIn('/usr/local/libexec/brrdfeeder-memory-controller check || warn',installer)
        self.assertLess(installer.index('# B06: enable the Pi controller'),installer.index('gate quadlets '))


if __name__=='__main__':unittest.main()
