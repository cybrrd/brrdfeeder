#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Mutation controls for the extracted filter; never executes the installer."""
import importlib.util
import io
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('contract', Path(__file__).with_name('test-contract.py'))
contract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)
original = contract.SOURCE
mutants = {
    'migration-noop': ("sys.stdout.buffer.write(changed)", "sys.stdout.buffer.write(raw)"),
    'custom-min-removed': ("('lock_on_min_ms', '500')", "('lock_on_min_ms', '700')"),
    'ignore-aliases': ('if any(isinstance(t,', 'if False and any(isinstance(t,'),
    'lost-wiring': ('\n  migrate_template_lock_on\n', '\n  :\n'),
}
for name, (before, after) in mutants.items():
    assert before in original, name
    contract.SOURCE = original.replace(before, after, 1)
    output = io.StringIO()
    result = unittest.TextTestRunner(stream=output).run(unittest.defaultTestLoader.loadTestsFromTestCase(contract.Contract))
    assert result.failures and not result.errors, (name, output.getvalue())
    print(name + ': KILLED by behavioral assertion')
