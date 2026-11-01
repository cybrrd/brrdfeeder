#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Isolated negative controls; never edit the live checkout or use real radios."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[5]
AVIARY = ROOT/'Component/aviary'
INSTALL = ROOT/'Component/brrdfeeder/install'
out = Path(os.environ.get('D44_EVIDENCE_DIR', tempfile.mkdtemp(prefix='wifi-mutation-results-')))
out.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, CARGO_NET_OFFLINE='true', PYTHONDONTWRITEBYTECODE='1')
records = []


def record(name, command, cwd, expected_failure=False, expected_test=None):
    result = subprocess.run(command, cwd=cwd, env=env, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=600)
    (out/(name+'.log')).write_text(result.stdout)
    if expected_failure:
        assert result.returncode != 0, name+' mutation escaped'
        if expected_test:
            assert re.search(r'test .*'+re.escape(expected_test)+r' .*FAILED', result.stdout), name+' did not reach its intended test refusal'
        assert 'error: could not compile' not in result.stdout, name+' was a compile failure, not a negative control'
    else:
        assert result.returncode == 0, name+' baseline/restored failure; STOP: '+str(out/(name+'.log'))
    records.append({'name': name, 'exit': result.returncode, 'expected_negative': expected_failure, 'result': 'PASS'})
    (out/'summary.json').write_text(json.dumps(records, indent=2)+'\n')
    print(name+': PASS'+(' (mutant refused)' if expected_failure else ''), flush=True)


with tempfile.TemporaryDirectory(prefix='wifi-identity-mutants-') as temp:
    temp = Path(temp)
    copied = temp/'Component/aviary'
    shutil.copytree(AVIARY, copied, ignore=shutil.ignore_patterns('target', '__pycache__', '.git'))
    # Existing engine contract tests embed the sibling installer at compile
    # time. Preserve that dependency in the isolated source fixture as well.
    shutil.copytree(INSTALL, temp/'Component/brrdfeeder/install', ignore=shutil.ignore_patterns('__pycache__'))
    env['CARGO_TARGET_DIR'] = str(Path(env.get('CARGO_TARGET_DIR', AVIARY/'target')).resolve())
    command = ['cargo', 'test', '--offline', '--locked', '-p', 'engine', 'wifi_', '--', '--nocapture']
    record('rust-baseline', command, copied)
    path = copied/'engine/src/capture_identity.rs'
    original = path.read_text()
    mutations = [
        ('ambiguity-first-match', 'if candidates.len() > 1 {', 'if false {', 'wifi_two_capable_adapters_or_two_interfaces_never_administers_a_first_match'),
        ('capability-bypass', '} else if !capable.contains(&adapter.phy) {', '} else if false {', 'wifi_unsupported_unknown_and_missing_driver_refuse_without_administration'),
        ('stale-identity-bypass', 'if observed.as_ref() != Some(expected) {', 'if false {', 'wifi_same_name_and_reused_index_do_not_authorize_a_different_device_or_driver'),
        ('explicit-name-conversion', 'Ok(Self::Interface(name.into()))', 'Ok(Self::Usb(None))', 'wifi_selector_keeps_explicit_name_and_refuses_conflicting_configuration'),
        ('active-rescan-ignores-ambiguity', 'Ok(Err(e)) => return e,', 'Ok(Err(_)) => {},', 'wifi_active_rescan_detects_added_ambiguity_without_waiting_for_a_packet'),
    ]
    for name, needle, replacement, test in mutations:
        assert original.count(needle) == 1, name+' source anchor drift'
        path.write_text(original.replace(needle, replacement, 1))
        try:
            record(name, command, copied, True, test)
        finally:
            path.write_text(original)
    hunter = copied/'engine/src/hunter.rs'
    text = hunter.read_text()
    assert text.count('*parked_set = false;') == 1
    hunter.write_text(text.replace('*parked_set = false;', '*parked_set = true;', 1))
    try:
        record('parked-state-retained', command, copied, True, 'wifi_parked_schedule_rearms_on_replacement_even_with_reused_ifindex')
    finally:
        hunter.write_text(text)
    record('rust-restored', command, copied)

    # Python helper mutations execute the actual behavioral tests against a
    # temporary installer copy, including updated embedded bytes/checksums.
    copied_install = temp/'python/Component/brrdfeeder/install'
    shutil.copytree(INSTALL, copied_install, ignore=shutil.ignore_patterns('__pycache__'))
    py_command = ['python3', '-B', str(copied_install/'tests/wifi-capture-identity/test-contract.py')]
    record('python-baseline', py_command, temp)
    source = (copied_install/'capture-usb.py').read_text()
    for name, needle, replacement in [
        ('python-first-match', 'if len(candidates) > 1:', 'if False:'),
        ('python-monitor-bypass', 'if capability(phy):', 'if True:'),
        ('python-driver-bypass', "if ue.get('DEVTYPE') != 'usb_interface' or ue.get('DRIVER') != driver.name:", 'if False:'),
    ]:
        assert source.count(needle) == 1
        (copied_install/'capture-usb.py').write_text(source.replace(needle,replacement,1))
        try:
            # A failing embed/pin assertion alone would not prove the behavioral
            # protection: run the targeted hardware-free selector tests only.
            targeted = py_command+['CaptureIdentity.test_two_capable_adapters_and_virtual_interfaces_refuse'] if name=='python-first-match' else py_command+['CaptureIdentity.test_absent_and_unknown_capability_refuse'] if name=='python-monitor-bypass' else py_command+['CaptureIdentity.test_missing_or_inconsistent_driver_and_usb_uevent_refuse']
            record(name, targeted, temp, True)
        finally:
            (copied_install/'capture-usb.py').write_text(source)
    record('python-restored', py_command, temp)
print('All negative controls detected; restored sources passed. Evidence: '+str(out), flush=True)
