#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""In-memory mutations of extracted enrollment; never run host installer."""
import importlib.util
import io
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('fixture', Path(__file__).parents[1]/'pi-native-p0/test-contract.py')
fixture = importlib.util.module_from_spec(spec); spec.loader.exec_module(fixture)
original = fixture.SOURCE
for label, before, after, test in [
    ('no-refresh', 'for attempt in 1 2 3; do', 'for attempt in 1; do', 'test_expired_code_reissues_in_place'),
    ('no-outcome-log', 'log_event DETAIL "device-flow outcome=$1 attempt=$attempt"', ':', 'test_pending_and_slowdown_transitions'),
    ('no-slowdown', 'interval=$(( interval + 5 ))', ':', 'test_pending_and_slowdown_transitions'),
    ('poll-after-deadline', '(( $(date +%s) < deadline )) || break', ':', 'test_local_deadline_reissues_without_polling_expired_code'),
]:
    assert original.count(before) == 1, label
    fixture.SOURCE = original.replace(before, after)
    output = io.StringIO()
    result = unittest.TextTestRunner(stream=output).run(fixture.Contract(test))
    assert result.failures and not result.errors, (label, output.getvalue())
    print(label+': KILLED by behavioral assertion')
