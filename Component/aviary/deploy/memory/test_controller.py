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
        self.proc=self.root/'proc/cmdline'
        self.proc.parent.mkdir()
        self.proc.write_bytes(self.original)
        self.controllers=self.root/'sys/fs/cgroup/cgroup.controllers'
        self.controllers.parent.mkdir(parents=True)
        self.controllers.write_text('cpu io pids\n')

    def tearDown(self):
        self.temp.cleanup()

    def test_idempotent_backed_up_and_byte_exact_restore(self):
        self.assertIn('REBOOT REQUIRED',controller.edit(self.root))
        expected=b'console=serial0,115200 root=PARTUUID=fixture quiet cgroup_enable=memory\n'
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
        self.assertEqual(controller.enabled_cmdline(b'a cgroup_disable=cpu,memory,pids b\n'),b'a cgroup_disable=cpu,pids b cgroup_enable=memory\n')
        self.assertEqual(controller.enabled_cmdline(b'a  cgroup_disable=cpu b\n'),b'a cgroup_disable=cpu b cgroup_enable=memory\n')

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
        base=self.root/'sys/fs/cgroup'
        (base/'cgroup.controllers').write_text('cpu io\n')
        with self.assertRaisesRegex(ValueError,'absent'):controller.check(self.root)
        (base/'cgroup.controllers').write_text('cpu io memory\n')
        cap=base/'system.slice/brrdfeeder-engine.service/memory.max';cap.parent.mkdir(parents=True)
        for value in ('max','536870912','0'):
            cap.write_text(value+'\n')
            with self.assertRaisesRegex(ValueError,'NOT verified'):controller.check(self.root)
        cap.write_text('268435456\n')
        self.assertIn('cap enforced',controller.check(self.root))

    def test_firmware_injected_disable_stages_later_enable(self):
        original=b'console=serial0,115200 root=PARTUUID=fixture quiet\n'
        self.cmd.write_bytes(original)
        self.proc.write_bytes(b'cgroup_disable=memory '+original)
        result=controller.edit(self.root)
        self.assertEqual(self.cmd.read_bytes(),original.rstrip()+b' cgroup_enable=memory\n')
        self.assertEqual(self.cmd.with_name('cmdline.txt.brrdfeeder-memory.bak').read_bytes(),original)
        self.assertIn('REBOOT REQUIRED',result)
        self.assertIn('runtime memory flags: cgroup_disable=memory',result)
        with self.assertRaisesRegex(ValueError,'MemoryMax is NOT enforced'):
            controller.check(self.root)

    def test_staged_enable_is_idempotent_until_reboot(self):
        self.cmd.write_bytes(b'quiet cgroup_enable=memory\n')
        before=self.cmd.stat().st_mtime_ns
        self.assertIn('REBOOT REQUIRED',controller.edit(self.root))
        self.assertEqual(self.cmd.stat().st_mtime_ns,before)
        self.assertFalse(self.cmd.with_name('cmdline.txt.brrdfeeder-memory.bak').exists())

    def test_available_runtime_is_authoritative_and_enable_does_not_claim_cap(self):
        self.controllers.write_text('cpu memory pids\n')
        before=self.cmd.stat().st_mtime_ns
        result=controller.edit(self.root)
        self.assertIn('memory controller available',result)
        self.assertNotIn('cap enforced',result)
        self.assertEqual(self.cmd.stat().st_mtime_ns,before)
        self.assertFalse(self.cmd.with_name('cmdline.txt.brrdfeeder-memory.bak').exists())

    def test_reboot_with_later_enable_verifies_live_cap(self):
        controller.edit(self.root)
        self.proc.write_bytes(b'cgroup_disable=memory '+self.cmd.read_bytes())
        self.controllers.write_text('cpu memory pids\n')
        cap=self.controllers.parent/'system.slice/brrdfeeder-engine.service/memory.max'
        cap.parent.mkdir(parents=True)
        cap.write_text('268435456\n')
        self.assertIn('cap enforced',controller.check(self.root))
        self.assertNotIn('REBOOT REQUIRED',controller.edit(self.root))

    def test_running_enable_without_controller_reports_kernel_problem(self):
        self.cmd.write_bytes(b'quiet cgroup_enable=memory\n')
        self.proc.write_bytes(b'cgroup_disable=memory '+self.cmd.read_bytes())
        self.assertIn('inspect kernel/cgroup support',controller.edit(self.root))
        with self.assertRaisesRegex(ValueError,'inspect kernel/cgroup support'):
            controller.check(self.root)

    def test_later_runtime_disable_is_not_treated_as_enabled(self):
        self.cmd.write_bytes(b'quiet cgroup_enable=memory\n')
        self.proc.write_bytes(b'cgroup_enable=memory cgroup_disable=cpu,memory\n')
        self.assertIn('REBOOT REQUIRED',controller.edit(self.root))
        with self.assertRaisesRegex(ValueError,'run enable, then reboot'):
            controller.check(self.root)

    def test_no_runtime_flag_still_repairs_absent_controller(self):
        self.proc.write_bytes(b'quiet\n')
        self.cmd.write_bytes(b'quiet\n')
        self.assertIn('REBOOT REQUIRED',controller.edit(self.root))
        self.assertEqual(self.cmd.read_bytes(),b'quiet cgroup_enable=memory\n')

    def test_enable_tokens_deduplicated_and_unrelated_controllers_preserved(self):
        raw=b'quiet cgroup_enable=memory,cpu cgroup_disable=cpu,memory,pids cgroup_enable=memory\n'
        expected=b'quiet cgroup_enable=cpu cgroup_disable=cpu,pids cgroup_enable=memory\n'
        self.assertEqual(controller.enabled_cmdline(raw),expected)
        self.assertEqual(controller.enabled_cmdline(expected),expected)
        self.assertEqual(expected.count(b'\n'),1)

    def test_empty_and_multiline_cmdline_refused_without_backup(self):
        for raw in (b'',b'\n',b'quiet\nother\n',b'quiet\n\n',b'quiet\r\n'):
            with self.subTest(raw=raw):
                self.cmd.write_bytes(raw)
                with self.assertRaisesRegex(ValueError,'one line'):
                    controller.edit(self.root)
                self.assertEqual(self.cmd.read_bytes(),raw)
                self.assertFalse(self.cmd.with_name('cmdline.txt.brrdfeeder-memory.bak').exists())

    def test_missing_runtime_evidence_refuses_edit(self):
        for path in (self.proc,self.controllers):
            raw=path.read_bytes()
            path.unlink()
            with self.assertRaises(OSError):controller.edit(self.root)
            self.assertEqual(self.cmd.read_bytes(),self.original)
            self.assertFalse(self.cmd.with_name('cmdline.txt.brrdfeeder-memory.bak').exists())
            path.write_bytes(raw)

    def test_prior_backup_is_not_overwritten(self):
        backup=self.cmd.with_name('cmdline.txt.brrdfeeder-memory.bak')
        backup.write_bytes(b'earlier boot command line\n')
        with self.assertRaisesRegex(ValueError,'prior backup differs'):
            controller.edit(self.root)
        self.assertEqual(self.cmd.read_bytes(),self.original)
        self.assertEqual(backup.read_bytes(),b'earlier boot command line\n')

    def test_previous_version_repair_can_still_be_restored(self):
        self.cmd.with_name('cmdline.txt.brrdfeeder-memory.bak').write_bytes(self.original)
        self.cmd.write_bytes(b'console=serial0,115200 root=PARTUUID=fixture quiet\n')
        self.assertIn('REBOOT REQUIRED',controller.edit(self.root,restore=True))
        self.assertEqual(self.cmd.read_bytes(),self.original)

    def test_previous_version_repair_can_be_upgraded_without_losing_backup(self):
        backup=self.cmd.with_name('cmdline.txt.brrdfeeder-memory.bak')
        backup.write_bytes(self.original)
        before=backup.stat().st_mtime_ns
        self.cmd.write_bytes(b'console=serial0,115200 root=PARTUUID=fixture quiet\n')
        self.assertIn('REBOOT REQUIRED',controller.edit(self.root))
        self.assertEqual(self.cmd.read_bytes(),b'console=serial0,115200 root=PARTUUID=fixture quiet cgroup_enable=memory\n')
        self.assertEqual(backup.read_bytes(),self.original)
        self.assertEqual(backup.stat().st_mtime_ns,before)
        controller.edit(self.root,restore=True)
        self.assertEqual(self.cmd.read_bytes(),self.original)

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
