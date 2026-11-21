#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Temporarily reinject six historic defects, require runtime reds, restore green.

Never changes expectations. Restores source bytes in finally, even on failure.
Run only in an isolated development checkout without concurrent builds/edits.
"""
import argparse,hashlib,json,pathlib,re,subprocess

HERE=pathlib.Path(__file__).resolve().parent
PROTOCOL=HERE.parents[1]
AVIARY=PROTOCOL.parent
CORE=AVIARY/'cybrrd-rid-core/src/v5.rs'
ADAPT=PROTOCOL/'src/v5/mod.rs'
PRODUCER=PROTOCOL/'src/v5/producer.rs'

def one(pattern,replacement,source):
    out,n=re.subn(pattern,replacement,source,count=1,flags=re.S)
    assert n==1,('mutation source guard',pattern)
    assert out!=source
    return out

def dedup(source):
    start=source.index('pub(super) fn capture_with')
    prefix=source[:start];body=source[start:]
    replacement='''static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    let fresh = SEEN.get_or_init(Default::default).lock().map_err(|_| Error("injected lock failure".into()))?.insert(c.event.identity.to_string());
    let receipt = if fresh { ledger.append(&c.event)? } else { Receipt { ledger_record: "deduplicated".into() } };'''
    return prefix+one(r'let\s+receipt\s*=\s*ledger\.append\(&c\.event\)\?;',replacement,body)

MUTANTS=[
    ('serial_fallback',CORE,lambda s:one(r'let\s+id\s*=\s*b\[1\]\s*>>\s*4;', 'let id = if b[1] >> 4 == 0 { 1 } else { b[1] >> 4 };',s),'independent_message_goldens','basic_id_type_0'),
    ('hae_msl_swap',CORE,lambda s:one(r'("geodetic_altitude_hae"\s*,\s*altitude\([^;]*?)"m_WGS84_HAE"',r'\1"m_MSL"',s),'independent_message_goldens','vertical_raw_0'),
    ('whole_byte_last_page',CORE,lambda s:one(r'("last_page"\s*,\s*code\(b\[2\])\s*&\s*15',r'\1',s),'independent_message_goldens','auth_page_0'),
    ('continuation_first_loss',ADAPT,lambda s:one(r'messages\.push\(m\);','if m.pointer("/auth/data_page_number/raw").and_then(Value::as_u64).is_some_and(|p| p != 0) { continue; }\n        messages.push(m);',s),'independent_observation_goldens','bt5_long_range'),
    ('changed_content_dedup',PRODUCER,dedup,'cases::changed_content_same_identity_is_appended','changed-content dedup lost a capture'),
    ('unchecked_version',CORE,lambda s:one(r'if\s+h\s*&\s*15\s*!=\s*2\s*\{','if false {',s),'independent_message_goldens','header_version_0'),
]

def main():
    p=argparse.ArgumentParser();p.add_argument('--evidence',type=pathlib.Path,required=True);args=p.parse_args();args.evidence.mkdir(parents=True,exist_ok=True)
    original={path:path.read_bytes() for _,path,_,_,_ in MUTANTS}
    results=[]
    base=['cargo','test','--release','--locked','--manifest-path',str(AVIARY/'Cargo.toml'),'-p','cybrrd-rid-protocol','--features','wire-v5','--test','wire_v5']
    try:
        for name,path,mutate,test,reason in MUTANTS:
            path.write_text(mutate(original[path].decode()))
            run=subprocess.run(base+[test,'--','--exact','--nocapture'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
            (args.evidence/(name+'.log')).write_text(run.stdout)
            assert run.returncode!=0 and 'test result: FAILED.' in run.stdout and '1 failed;' in run.stdout and reason in run.stdout,(name,run.stdout[-5000:])
            path.write_bytes(original[path])
            green=subprocess.run(base+[test,'--','--exact','--nocapture'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
            (args.evidence/(name+'-restored.log')).write_text(green.stdout)
            assert green.returncode==0 and '1 passed;' in green.stdout,(name,green.stdout[-5000:])
            results.append({'defect':name,'runtime_red':True,'restored_green':True,'test':test,'reason_observed':reason,'source_sha256':hashlib.sha256(original[path]).hexdigest()})
            print(f'{name}: runtime red; restored 1/1 green',flush=True)
    finally:
        for path,data in original.items():path.write_bytes(data)
    assert all(path.read_bytes()==data for path,data in original.items())
    receipt={'mutants_detected':len(results),'mutants_total':6,'restored_green':len(results),'source_restored_byte_exact':True,'results':results}
    (args.evidence/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n');print(json.dumps(receipt,indent=2))

if __name__=='__main__':main()
