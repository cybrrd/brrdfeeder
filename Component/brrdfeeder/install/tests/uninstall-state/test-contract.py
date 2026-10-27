#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Writer-derived coverage, parity and conservative scanner negative controls."""
import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest

HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[4]
INSTALL=ROOT/'Component/brrdfeeder/install'


def load(path,name):
    spec=importlib.util.spec_from_file_location(name,path)
    module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    return module


scan=load(HERE/'check-writers.py','writer_scan')


def copy_sources(destination):
    paths={key.split('::')[0] for key in scan.census(ROOT)}
    paths.update('Component/brrdfeeder/install/'+name for name in
                 ('uninstall.sh','uninstall-state.py','uninstall-account.py','brrdfeeder-install.sh',
                  'tests/uninstall-state/writer-paths.json'))
    for name in paths:
        target=destination/name; target.parent.mkdir(parents=True,exist_ok=True)
        shutil.copyfile(ROOT/name,target)


class Contract(unittest.TestCase):
    def test_embedded_copies_exact(self):
        installer=(INSTALL/'brrdfeeder-install.sh').read_text()
        helper=(INSTALL/'uninstall.sh').read_text()
        self.assertEqual(installer.split("<<'UNINSTALL_EOF'\n",1)[1].split('\nUNINSTALL_EOF',1)[0]+'\n',helper)
        self.assertEqual(helper.split("<<'PACKAGE_STATE_EOF'",1)[1].split('\n',1)[1].split('\nPACKAGE_STATE_EOF',1)[0]+'\n',
                         (INSTALL/'uninstall-state.py').read_text())

    def test_extracted_inventory_is_covered(self):
        files,expressions,paths=scan.check(ROOT)
        self.assertGreater(files,30); self.assertGreater(expressions,100); self.assertGreater(paths,50)
        projected={str(p) for p in scan.destinations(scan.census(ROOT))}
        # A census refresh alone cannot silently drop the dynamic projections.
        expected=['/var/lib/brrdfeeder/release_attempt.json','/var/lib/brrdfeeder-updater/attempt.json',
                  '/var/lib/brrdfeeder/credentials/brrdfeeder.creds','/var/lib/brrdfeeder/credentials/previous.creds',
                  '/var/lib/brrdfeeder/credentials/.renew.lock','/var/lib/brrdfeeder/capture/capture-15.pcap',
                  '/var/lib/brrdfeeder/status.json','/var/lib/brrdfeeder/status.json.tmp.101.0',
                  '/var/lib/brrdfeeder-updater/.pending-123','/var/lib/brrdfeeder-updater/.host-recover-abcd_123',
                  '/etc/brrdfeeder/.host-recover-abcd_123','/etc/brrdfeeder/.update-123',
                  '/etc/brrdfeeder/.config.yaml-abcd_123']
        self.assertTrue(set(expected)<=projected,set(expected)-projected)

    def test_new_writer_paths_fail_with_clean_control_before_each(self):
        additions=[
            ('Component/aviary/release-system/attempt.go',
             '\nfunc newWriter(u *Updater) { atomicJSON(filepath.Join(u.cfg.StateDir, "future.json"), nil, 0644) }\n'),
            ('Component/aviary/engine/src/forensic.rs',
             '\nfn new_writer() { std::fs::write("/var/lib/brrdfeeder/new-sensor.json", b"{}"); }\n'),
            ('Component/brrdfeeder/install/gps-seed.py',
             '\ndef new_writer():\n    Path("/etc/brrdfeeder/new-helper.json").write_bytes(b"{}")\n'),
            ('Component/aviary/release-system/future_writer.go',
             'package main\nfunc future() { os.WriteFile("/var/lib/brrdfeeder-updater/future", nil, 0600) }\n'),
        ]
        for name,addition in additions:
            with self.subTest(source=name),tempfile.TemporaryDirectory() as folder:
                root=Path(folder); copy_sources(root)
                scan.check(root)  # clean control immediately before this mutation
                path=root/name
                with path.open('a') as stream: stream.write(addition)
                with self.assertRaisesRegex(ValueError,'unreviewed writer/path expression'): scan.check(root)

    def test_changed_loop_name_and_capture_range_fail(self):
        mutations=[
            ('Component/aviary/release-system/local_upgrade.go','[]string{"launch.lock", "lock"}',
             '[]string{"launch.lock", "new-lock"}'),
            ('Component/aviary/engine/src/node_config.rs','(1..=16).contains(&self.max_files)',
             '(1..=17).contains(&self.max_files)'),
        ]
        for name,old,new in mutations:
            with self.subTest(source=name),tempfile.TemporaryDirectory() as folder:
                root=Path(folder); copy_sources(root); scan.check(root)
                path=root/name; text=path.read_text(); self.assertEqual(text.count(old),1)
                path.write_text(text.replace(old,new))
                with self.assertRaisesRegex(ValueError,'unreviewed writer/path expression'): scan.check(root)

    def test_test_only_and_comment_paths_do_not_become_product_writers(self):
        original='fn persist() { root.join("real.json"); }\n'
        added=original+'// root.join("comment.json");\n#[cfg(test)]\nfn fixture() { root.join("fake.json"); }\n'
        self.assertEqual(scan.lexical(original,True),scan.lexical(added,True))
        # Production code after a cfg(test) item must still be scanned.
        self.assertNotEqual(scan.lexical(original,True),scan.lexical(added+'fn next() { root.join("new.json"); }',True))
        inline='fn start() { let c = Config { path: "real.json", #[cfg(test)] test_user: None }; persist(c); }'
        stripped=scan.without_rust_tests(scan.tokens(inline))
        self.assertIn('start',scan.functions(stripped))

    def test_existing_writer_wrapper_new_call_is_detected(self):
        source='func save(path string) { atomicFile(path, nil, 0600) }\nfunc caller(path string) {}\n'
        wrappers=scan.wrapper_calls([(source,False)])
        self.assertIn('save',wrappers)
        baseline=scan.lexical(source,wrappers=wrappers)
        for added in ('save("new-bare-name")','save(path)'):
            with self.subTest(call=added):
                self.assertEqual(scan.lexical(source,wrappers=wrappers),baseline)  # clean control
                self.assertNotEqual(scan.lexical(source.replace('caller(path string) {}','caller(path string) { '+added+' }'),
                                                 wrappers=wrappers),baseline)

    def test_python_fstring_census_uses_structure_not_printer_quotes(self):
        import ast
        expression=ast.parse('f"result={\'FAILED\' if rc else \'OK\'}"',mode='eval').body
        self.assertEqual(scan.render(expression),"fstring('result=', ('FAILED' if rc else 'OK', -1, None))")
        self.assertEqual(scan.python('out.write(f"result={\'FAILED\' if rc else \'OK\'}")'),
                         {'call out.write (fstring(\'result=\', (\'FAILED\' if rc else \'OK\', -1, None)))':1})

    def test_census_refresh_cannot_hide_literal_uncovered_go_destination(self):
        # Even if a reviewer updates the expression census, the actual generated
        # path must dispatch through the uninstaller's current policy.
        import json
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); copy_sources(root); scan.check(root)
            path=root/'Component/aviary/release-system/attempt.go'
            path.write_text(path.read_text()+'\nfunc future(u *Updater) { atomicJSON(filepath.Join(u.cfg.StateDir, "uncovered.json"), nil, 0644) }\n')
            (root/'Component/brrdfeeder/install/tests/uninstall-state/writer-paths.json').write_text(json.dumps(scan.census(root)))
            with self.assertRaisesRegex(ValueError,'uncovered writer path'): scan.check(root)


if __name__=='__main__': unittest.main(verbosity=2)
