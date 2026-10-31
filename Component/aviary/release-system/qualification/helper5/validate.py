#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Validate registered native evidence; complete evidence still requires review.

Run on the off-box evidence export, never on credentials. The registration and
its SHA must be independently received before testing, not generated from rows.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
from binding import validate as validate_binding

PHASES=('prepared','switched','receipted','floored','committed')
ROWS={'normal','reboot-interrupted','reboot-committed','handback'} | {
    p+'-'+r for p in PHASES for r in ('launcher','preimages')}

def require(ok, reason):
    if not ok: raise ValueError(reason)

def sha(raw): return hashlib.sha256(raw).hexdigest()

def check_pairs(events, syscall=82):
    pending={};pairs=[]
    for event in events:
        require(event['syscall']==syscall,'wrong syscall')
        key=tuple(event['ptid'])
        if event['reason']=='syscall-entry':
            require(key not in pending,'duplicate same-thread entry')
            pending[key]=event
        else:
            require(event['reason']=='syscall-return' and key in pending,'unpaired syscall return')
            entry=pending.pop(key)
            require(all(event[k]==entry[k] for k in ('fd','path','is_directory')),'wrong-thread/descriptor syscall pair')
            pairs.append((entry,event))
    # A killed multithreaded helper may leave other threads at syscall entry.
    return pairs,pending

def validate(evidence, registration, registration_sha, root):
    require(sha(registration)==registration_sha,'registration hash changed')
    reg=json.loads(registration);b=validate_binding(reg['binding'])
    for key in ('preimages_sha256','protected_sha256'):
        require(bool(re.fullmatch('[0-9a-f]{64}',reg.get(key,''))),'unregistered '+key)
    require(evidence['registration_sha256']==registration_sha,'evidence registration mismatch')
    require(evidence['host']==b['hostname'] and evidence['node_id']==b['node_id'],'wrong host/node')
    require(evidence['from_build']==4 and evidence['to_build']==5,'wrong transition')
    require(evidence['binding']==b,'artifact/source binding mismatch')
    def artifact(ref):
        path=Path(ref['path'])
        require(not path.is_absolute() and '..' not in path.parts,'unsafe evidence path')
        raw=(root/path).read_bytes()
        require(sha(raw)==ref['sha256'],'evidence hash changed')
        return raw
    def document(ref): return json.loads(artifact(ref))
    preimage_raw=artifact(evidence['preimages'])
    require(sha(preimage_raw)==reg['preimages_sha256'],'preimage manifest hash mismatch')
    preimages=json.loads(preimage_raw)
    binary='/usr/local/libexec/brrdfeeder-release'
    receipt='/etc/brrdfeeder/.updater-helper.sha256'
    floor='/var/lib/brrdfeeder-updater/host-state.json'
    paths={binary,receipt,floor,binary+'.previous',binary+'.previous.sha256',binary+'-launch'}
    require(set(preimages)==paths,'not exactly six helper preimages')
    for item in preimages.values():
        if item is not None:
            require(sha(base64.b64decode(item['data'],validate=True))==item['sha256'] and item['uid']==0 and not item['mode'] & 0o022,'bad registered preimage')
    require(preimages[binary]['sha256']==b['baseline_helper_sha256'],'wrong baseline helper preimage')
    prior_floor=base64.b64decode(preimages[floor]['data']) if preimages[floor] else b''
    require(not prior_floor or 0<=json.loads(prior_floor)['build_seq']<=4,'baseline floor exceeds 4')
    expected_floor=json.loads(prior_floor) if prior_floor else {'next_poll':'0001-01-01T00:00:00Z','seen':{'sequence':0,'hash':''},'attempted':{}}
    expected_floor['build_seq']=5
    # Receipts bind actual captures, not PASS filenames.
    calibration=document(evidence['calibration'])
    require(calibration==dict(status='PASS',native_arm64=True,mode='calibrate',registers=['$x0','$x0'],
                              syscall=82,threads=2,pairs=18,pending=0),'wrong native calibration')
    pairs,pending=check_pairs(document(evidence['calibration_events']))
    require(len(pairs)==18 and not pending and len({tuple(p[0]['ptid']) for p in pairs})==2,'calibration pairing')
    for thread in {tuple(p[0]['ptid']) for p in pairs}:
        actual=[p for p in pairs if tuple(p[0]['ptid'])==thread]
        require(len(actual)==9,'calibration per-thread count')
        for kind,ret in ((True,0),(False,0),(False,-9)):
            require(sum(e['is_directory']==kind and r['return_value']==ret for e,r in actual)==3,'calibration return distribution')
    require(set(evidence['rows'])==ROWS,'missing/extra native phase or route')
    for name,row in evidence['rows'].items():
        require(row['host']==b['hostname'] and row['node_id']==b['node_id'],'row host/node mismatch')
        require(row['registration_sha256']==registration_sha and row['preimages_sha256']==reg['preimages_sha256'],'row registration mismatch')
        require(row['protected_before_sha256']==reg['protected_sha256'] and row['protected_after_sha256']==reg['protected_sha256'],'protected state changed')
        require(row['start']<=row['end'],'row time order')
        for side in ('before','after'):
            health=document(row['health_'+side])
            require(health['host']==b['hostname'] and health['node_id']==b['node_id'],'health identity')
            require(abs(health['observed_at']-row['start' if side=='before' else 'end'])<=120,'stale health')
            require(all(health[k] is True for k in ('radio','gps','clock','engine','console','broker')),'unhealthy witness')
        require(artifact(row['final_preimages'])==preimage_raw,'restored six preimages differ')
        state=document(row['final_state'])
        require(state['helper_sha256']==b['baseline_helper_sha256'] and state['build']==4 and state['effective_floor']==4 and state['journal'] is None,'handback is not registered helper 4')
        require(state['six_preimages_sha256']==reg['preimages_sha256'] and state['floor_preimage_exact'] is True,'restored-state mismatch')
        if name=='normal':
            for key in ('normal_trace','idempotent_trace'):
                trace=artifact(row[key])
                require(b'execve(' in trace and not re.search(rb'AF_INET6?',trace),'missing exec/network trace or network call')
            first=document(row['installed_first']);second=document(row['installed_second'])
            require(first==second,'idempotence metadata changed')
            require(first['helper_sha256']==b['candidate_sha256'] and first['build']==5 and first['floor']==5 and first['journal'] is None,'normal build/floor/hash mismatch')
        elif name.startswith('reboot-'):
            require(bool(re.fullmatch('[a-f0-9-]{36}',row['boot_before'])) and bool(re.fullmatch('[a-f0-9-]{36}',row['boot_after'])) and row['boot_before']!=row['boot_after'],'no boot change')
            boot=document(row['boot_service'])
            require(boot['boot_id']==row['boot_after'] and boot['result']=='success' and boot['exit_status']==0 and boot['started_monotonic']>0 and bool(boot['invocation_id']),'missing current-boot recovery execution')
            require(boot['restored_locally']==(name=='reboot-interrupted'),'wrong boot recovery route')
            require(boot['build']==(4 if name=='reboot-interrupted' else 5),'wrong postboot build')
            require(boot['helper_sha256']==b['baseline_helper_sha256' if name=='reboot-interrupted' else 'candidate_sha256'],'wrong postboot hash')
        elif name!='handback':
            phase,route=name.split('-')
            require(row['phase']==phase and row['route']==route,'wrong recovery route/phase')
            stop=document(row['at_stop']);events=document(row['events']);pairs,_=check_pairs(events)
            require(bool(pairs),'no paired durable syscall')
            entry,ret=pairs[-1]
            require(stop['phase']==phase and stop['all_threads_stopped'] is True and stop['syscall_return']==0,'not stopped at durable phase')
            require(ret['phase']==phase and ret['return_value']==0 and entry['is_directory'] is True and stop['entry']==entry,'wrong fsync boundary')
            require(entry['path']==('/usr/local/libexec' if phase=='switched' else '/etc/brrdfeeder' if phase=='receipted' else '/var/lib/brrdfeeder-updater'),'wrong durable directory')
            snapshot=stop['state']
            expected_binary=b['baseline_helper_sha256' if phase=='prepared' else 'candidate_sha256']
            expected_receipt=b['baseline_helper_sha256' if phase in ('prepared','switched') else 'candidate_sha256']
            require(snapshot[binary]['sha256']==expected_binary,'phase binary mismatch')
            require(snapshot[receipt]['sha256']==sha((expected_receipt+'  '+binary+'\n').encode()),'phase receipt mismatch')
            raw_floor=artifact(row['floor_at_stop'])
            if phase in ('floored','committed'):
                require(bool(raw_floor) and json.loads(raw_floor)==expected_floor,'phase floor mismatch')
            else: require(raw_floor==prior_floor,'phase changed original floor')
            require((snapshot[floor] is None and not raw_floor) or (snapshot[floor] is not None and snapshot[floor]['sha256']==sha(raw_floor)),'phase floor capture mismatch')
            journal='/var/lib/brrdfeeder-updater/host-transaction.json'
            require((snapshot[journal] is None)==(phase=='committed'),'phase journal presence mismatch')
            result=document(row['gdb_result'])
            require(result['native_arm64'] is True and result['killed'] is True and result['phase']==phase and result['seen']==list(PHASES[:PHASES.index(phase)+1]),'not a native boundary kill')
            require(row['recovery_build']==(5 if phase=='committed' and route=='launcher' else 4),'wrong recovery state')
    require(evidence['rows']['handback']['timers_restored'] is True and evidence['rows']['handback']['normal_cycle_observed'] is True,'incomplete handback')
    return 'EVIDENCE_COMPLETE_REVIEW_REQUIRED'

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('evidence',type=Path);p.add_argument('registration',type=Path)
    p.add_argument('--registration-sha256',required=True)
    a=p.parse_args()
    print(validate(json.loads(a.evidence.read_bytes()),a.registration.read_bytes(),a.registration_sha256,a.evidence.parent))

if __name__=='__main__': main()
