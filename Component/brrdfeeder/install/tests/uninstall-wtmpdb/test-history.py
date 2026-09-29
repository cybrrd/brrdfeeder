#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Root-owned temporary fixtures ONLY; never inspects host login history."""
import importlib.util
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[5]

@unittest.skipUnless(os.geteuid()==0, 'Run in disposable root container for genuine ownership checks')
class History(unittest.TestCase):
    def setUp(self):
        spec=importlib.util.spec_from_file_location('audit',ROOT/'Component/brrdfeeder/install/uninstall-account.py')
        self.audit=importlib.util.module_from_spec(spec);spec.loader.exec_module(self.audit)
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name);self.log=self.root/'var/log';self.lib=self.root/'var/lib'
        self.log.mkdir(parents=True);(self.lib/'wtmpdb').mkdir(parents=True)
        self.audit.LOG=self.log;self.audit.WTMPDB=self.lib/'wtmpdb';self.audit.LASTLOG2=self.lib/'lastlog'
        self.audit.HISTORY_ROOTS=(self.log,self.lib)
        self.db=self.log/'wtmp.db'
        with sqlite3.connect(self.db) as db:
            db.execute('CREATE TABLE wtmp(ID INTEGER PRIMARY KEY, Type INTEGER, User TEXT, Login INTEGER, Logout INTEGER, TTY TEXT, RemoteHost TEXT, Service TEXT)')
            db.executemany('INSERT INTO wtmp(User,Login) VALUES(?,1)',[('alice',),('operator',)])
        self.db.chmod(0o644)
        self.alias=self.lib/'wtmpdb/wtmp.db';self.alias.symlink_to('../../log/wtmp.db')
        (self.log/'wtmp').write_bytes(bytes(6400))
        (self.log/'lastlog').write_bytes(bytes(296888))

    def history(self):
        with patch.object(self.audit.platform,'machine',return_value='aarch64'):
            self.audit.history('brrdfeeder',999)

    def test_trixie_exact_layout_human_rows_accepted(self):
        before={p:p.read_bytes() for p in [self.db,self.log/'wtmp',self.log/'lastlog']}
        self.history()
        self.assertEqual(before,{p:p.read_bytes() for p in before})

    def test_service_login_refused_by_history_not_alias(self):
        with sqlite3.connect(self.db) as db: db.execute('INSERT INTO wtmp(User,Login) VALUES(?,1)',('brrdfeeder',))
        with self.assertRaisesRegex(ValueError,'login history exists for brrdfeeder'): self.history()

    def test_dangling_link_refused(self):
        self.alias.unlink();self.alias.symlink_to('../../log/missing.db')
        with self.assertRaises((ValueError,OSError)): self.history()

    def test_outside_tree_refused(self):
        path=self.root/'tmp/x';path.parent.mkdir();path.write_bytes(self.db.read_bytes())
        self.alias.unlink();self.alias.symlink_to(path)
        with self.assertRaises((ValueError,OSError)): self.history()

    def test_writable_target_refused(self):
        self.db.chmod(0o666)
        with self.assertRaises((ValueError,OSError)): self.history()

    def test_nonroot_target_refused(self):
        os.chown(self.db,12345,12345)
        with self.assertRaises((ValueError,OSError)): self.history()

    def test_nonregular_target_refused(self):
        self.alias.unlink();self.alias.symlink_to(self.log)
        with self.assertRaises((ValueError,OSError)): self.history()

    def test_loop_refused(self):
        other=self.lib/'wtmpdb/wtmp-loop.db'
        self.alias.unlink();self.alias.symlink_to(other.name);other.symlink_to(self.alias.name)
        with self.assertRaises((ValueError,OSError,RuntimeError)): self.history()

    def test_target_added_to_scan_even_when_not_globbed(self):
        other=self.lib/'archive.db';self.db.rename(other)
        self.alias.unlink();self.alias.symlink_to('../archive.db')
        self.history()
        with sqlite3.connect(other) as db: db.execute('INSERT INTO wtmp(User,Login) VALUES(?,1)',('brrdfeeder',))
        with self.assertRaisesRegex(ValueError,'login history exists'): self.history()

    def test_each_inode_scanned_once_including_hardlinks(self):
        os.link(self.db,self.log/'wtmp-hard.db')
        with patch.object(self.audit.sqlite3,'connect',wraps=sqlite3.connect) as connect:
            self.history();self.assertEqual(connect.call_count,1)

    def test_unknown_format_refuses(self):
        self.db.write_bytes(b'not a database')
        with self.assertRaises((ValueError,OSError)): self.history()

if __name__=='__main__': unittest.main(verbosity=2)
