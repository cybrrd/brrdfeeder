#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""In-memory mutations; never alters product source or touches service accounts."""
import importlib.util
import io
import json
import os
from pathlib import Path
import unittest

HERE=Path(__file__).resolve().parent
OUT=Path(os.environ.get('D44_EVIDENCE_DIR',str(HERE/'evidence')))
OUT.mkdir(parents=True,exist_ok=True)
cases=[]
for name,old,new,test in [
    ('tty-bypass',"die 'No terminal for confirmation.","return 0 # die 'No terminal for confirmation.",'Confirmation.test_no_terminal_refuses_before_mutation'),
    ('prompt-bypass','  notice \'Remove BRRDfeeder from this Pi? [y/N]\'','  return 0 # bypass confirmation','Confirmation.test_one_prompt_yes_and_cancel'),
    ('uid-floor','$id -ge 100','$id -ge 1','Profile.test_each_bad_profile_refuses_even_with_expert_flag'),
    ('shell-check','[[ $shell == /usr/sbin/nologin ]]','true','Profile.test_each_bad_profile_refuses_even_with_expert_flag'),
    ('home-check','[[ $account_home == "$home" ]]','true','Profile.test_each_bad_profile_refuses_even_with_expert_flag'),
    ('password-check','[[ $(passwd -S "$user" | awk \'{print $2}\') == L ]]','true','Profile.test_each_bad_profile_refuses_even_with_expert_flag'),
    ('privileged-groups','[[ " $groups " != *" $privilege "* ]]','true','Profile.test_each_bad_profile_refuses_even_with_expert_flag'),
    ('history-process-hook','audit_legacy_account "$user" "$id" ||','true ||','Profile.test_each_bad_profile_refuses_even_with_expert_flag'),
]:
    spec=importlib.util.spec_from_file_location('tests',HERE/'test-behavior.py')
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    assert module.HELPER.count(old)==1,(name,module.HELPER.count(old))
    module.HELPER=module.HELPER.replace(old,new)
    output=io.StringIO()
    result=unittest.TextTestRunner(stream=output,verbosity=2).run(unittest.defaultTestLoader.loadTestsFromName(test,module))
    (OUT/('mutation-'+name+'.log')).write_text(output.getvalue())
    cases.append({'control':name,'detected':not result.wasSuccessful(),'tests':result.testsRun})
(OUT/'mutations.json').write_text(json.dumps(cases,indent=2)+'\n')
print(json.dumps(cases,indent=2))
raise SystemExit(not all(case['detected'] for case in cases))
