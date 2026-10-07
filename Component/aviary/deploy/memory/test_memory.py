#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
import importlib.util
import json
import os
import re
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
ROOT = HERE.parents[3]
spec = importlib.util.spec_from_file_location('host_memory', HERE / 'host-memory.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class MemoryContracts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.state = self.root / 'state'
        self.state.mkdir()
        self.state.chmod(0o755)

    def test_only_oom_counts_once_per_invocation_and_survives_reload(self):
        for result in ('success', 'signal', 'exit-code', 'timeout', ''):
            self.assertFalse(m.record_stop(self.state, result, 'a' * 32))
        self.assertFalse((self.state / 'events.json').exists())
        self.assertTrue(m.record_stop(self.state, 'oom-kill', 'a' * 32))
        self.assertFalse(m.record_stop(self.state, 'oom-kill', 'a' * 32))
        self.assertEqual(m.counter(self.state)['memory_cap_events'], 1)
        self.assertTrue(m.record_stop(self.state, 'oom-kill', 'b' * 32))
        self.assertEqual(json.loads((self.state / 'events.json').read_text())['memory_cap_events'], 2)

    def test_corruption_untrusted_paths_and_overflow_never_reset(self):
        record = self.state / 'events.json'
        for raw in ('broken', '{}', 'null', '[]', '{"schema_version":1,"memory_cap_events":-1}',
                    json.dumps({'schema_version': 1, 'memory_cap_events': 2**64 - 1, 'last_invocation': 'a' * 32})):
            record.write_text(raw)
            with self.assertRaises(ValueError):
                m.record_stop(self.state, 'oom-kill', 'b' * 32)
            self.assertEqual(record.read_text(), raw)
        record.unlink()
        victim = self.root / 'victim'
        victim.write_text('untouched')
        record.symlink_to(victim)
        with self.assertRaises(OSError):
            m.record_stop(self.state, 'oom-kill', 'c' * 32)
        self.assertEqual(victim.read_text(), 'untouched')
        self.state.chmod(0o777)
        with self.assertRaises(ValueError):
            m.record_stop(self.state, 'oom-kill', 'c' * 32)

    def test_units_and_unavailable_are_absent(self):
        self.assertEqual(m.kib('MemTotal: 2000 kB\n', 'MemTotal'), 2048000)
        for raw in ('', 'MemTotal: -1 kB', 'MemTotal: 1 MB', 'MemTotal: 1 kB extra', 'MemTotal: 1 kB\nMemTotal: 2 kB'):
            self.assertIsNone(m.kib(raw, 'MemTotal'))
        proc = self.root / 'proc'
        boot = proc / 'sys/kernel/random'
        boot.mkdir(parents=True)
        (boot / 'boot_id').write_text('a-boot-id\n')
        snapshot = m.sample(proc, self.root / 'missing-journal', self.state, None)
        self.assertNotIn('console_rss_bytes', snapshot)
        self.assertNotIn('volatile_journal_bytes', snapshot)
        self.assertNotIn('host_mem_total_bytes', snapshot)
        self.assertEqual(snapshot['memory_cap_events'], 0)
        (self.state / 'events.json').write_text('broken')
        self.assertNotIn('memory_cap_events', m.sample(proc, self.root / 'missing', self.state, None))

    def test_journal_counts_allocated_regular_journal_files_only(self):
        journal = self.root / 'journal'
        journal.mkdir()
        f = journal / 'system.journal'
        f.write_bytes(b'x' * 5000)
        (journal / 'unrelated').write_bytes(b'x' * 9000)
        (journal / 'linked.journal').symlink_to(f)
        self.assertEqual(m.journal_bytes(journal), f.stat().st_blocks * 512)

    def test_console_requires_uid_unit_and_executable(self):
        proc = self.root / 'proc'
        process = proc / '100'
        process.mkdir(parents=True)
        (process / 'comm').write_text('brrdhouse\n')
        (process / 'status').write_text('VmRSS: 8000 kB\n')
        group = process / 'cgroup'
        group.write_text('0::/user.slice/brrdhouse.service/libpod-x\n')
        self.assertEqual(m.console_rss(proc, os.geteuid()), 8192000)
        self.assertIsNone(m.console_rss(proc, os.geteuid() + 1))
        group.write_text('0::/user.slice/not-brrdhouse.service/libpod-x\n')
        self.assertIsNone(m.console_rss(proc, os.geteuid()))


class DeploymentContracts(unittest.TestCase):
    def test_schema_lists_exact_optional_rust_fields(self):
        schema = json.loads((HERE / 'memory.schema.json').read_text())
        rust = (ROOT / 'Component/aviary/engine/src/memory.rs').read_text()
        fields = re.findall(r'pub (\w+): Option<u64>', rust)
        self.assertEqual(set(schema['properties']), set(fields))
        self.assertTrue(schema['additionalProperties'])
        self.assertFalse(schema.get('required'))
        self.assertEqual(schema['$defs']['bytes']['minimum'], 0)
        self.assertEqual(schema['$defs']['bytes']['maximum'], 2**64 - 1)

    def test_installer_embeds_exact_helpers_units_and_uninstaller(self):
        installer = (ROOT / 'Component/brrdfeeder/install/brrdfeeder-install.sh').read_text()
        for name, marker in (('host-memory.py', 'MEMORY_HELPER_EOF'),
                             ('brrdfeeder-memory.service', 'MEMORY_SERVICE_EOF'),
                             ('brrdfeeder-memory.timer', 'MEMORY_TIMER_EOF')):
            body = installer.split("<<'" + marker + "'\n", 1)[1].split('\n' + marker, 1)[0]
            self.assertEqual(body + '\n', (HERE / name).read_text())
        uninstaller = installer.split("<<'UNINSTALL_EOF'\n", 1)[1].split('\nUNINSTALL_EOF', 1)[0]
        self.assertEqual(uninstaller + '\n', (ROOT / 'Component/brrdfeeder/install/uninstall.sh').read_text())

    def test_generated_unit_caps_payload_and_restart_policy(self):
        loader = importlib.util.spec_from_file_location('quadlet_check', ROOT / 'Component/aviary/tools/check-quadlet-cidfile.py')
        q = importlib.util.module_from_spec(loader)
        loader.loader.exec_module(q)
        rendered = q.render(HERE.parent / 'quadlet/brrdfeeder-engine.container')
        service = rendered.split('[Service]', 1)[1]
        for line in ('MemoryMax=256M', 'MemorySwapMax=0', 'OOMPolicy=kill',
                     'Restart=always', 'ExecStopPost=/usr/local/libexec/brrdfeeder-host-memory stop'):
            self.assertIn(line, service.splitlines())
        self.assertRegex(service, r'--cgroups(?:=| )split(?: |$)')
        self.assertIn('StartLimitBurst=3', rendered)
        self.assertIn('StartLimitIntervalSec=300', rendered)
        self.assertIn('/run/brrdfeeder-memory:/run/brrdfeeder-memory:ro', rendered)
        # Verify the shipping installer block, not just the reference template.
        installer = (ROOT / 'Component/brrdfeeder/install/brrdfeeder-install.sh').read_text()
        unit = installer.split('NEW_QUADLET=$(cat <<EOF\n', 1)[1].split('\nEOF', 1)[0]
        for line in ('MemoryMax=256M', 'MemorySwapMax=0', 'OOMPolicy=kill', 'CgroupsMode=split',
                     'StartLimitBurst=3', 'StartLimitIntervalSec=300'):
            self.assertIn(line, unit.splitlines())


if __name__ == '__main__':
    unittest.main()
