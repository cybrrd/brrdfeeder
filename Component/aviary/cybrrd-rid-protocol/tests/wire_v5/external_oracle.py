#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Independent codec/schema oracle and source-bound u32 arithmetic proof.

Run with the dependencies pinned in oracle-requirements.txt and a compiled
wire_v5_oracle_bridge executable. All inputs are synthetic public fixtures.
"""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import cbor2
import jsonschema
import z3

HERE = pathlib.Path(__file__).resolve().parent
PROTOCOL = HERE.parents[1]
AVIARY = PROTOCOL.parent
SCHEMA = json.loads((PROTOCOL / 'src/v5/contract/observation.schema.json').read_text())
MESSAGE = SCHEMA['properties']['rid_units']['items']['properties']['messages']['items']
KEYS = json.loads((PROTOCOL / 'src/v5/contract/cbor-keys.json').read_text())['keys']
REVERSE = {v:k for k,v in KEYS.items()}

def child(s, key):
    if key in s.get('properties', {}):
        return s['properties'][key]
    for branch in s.get('anyOf', []):
        result = child(branch, key)
        if result:
            return result
    return {}

def item(s):
    if 'items' in s:
        return s['items']
    return next((item(b) for b in s.get('anyOf', []) if item(b)), {})

def byte_path(s):
    return s.get('pattern') == '^(?:[0-9a-f]{2})*$' or any(byte_path(b) for b in s.get('anyOf', []))

def to_cbor(value, s):
    if isinstance(value, dict):
        return {KEYS.get(k,k):to_cbor(v,child(s,k)) for k,v in value.items()}
    if isinstance(value, list):
        return [to_cbor(v,item(s)) for v in value]
    if isinstance(value,str) and byte_path(s):
        return cbor2.CBORTag(60000,bytes.fromhex(value))
    return value

def from_cbor(value):
    if isinstance(value,dict):
        return {REVERSE[k] if isinstance(k,int) else k:from_cbor(v) for k,v in value.items()}
    if isinstance(value,(list,tuple)):
        return [from_cbor(v) for v in value]
    if isinstance(value,cbor2.CBORTag):
        assert value.tag==60000 and isinstance(value.value,bytes)
        return value.value.hex()
    return value

def proof():
    source_path=AVIARY/'cybrrd-rid-core/src/v5.rs'
    source=source_path.read_text()
    compact=re.sub(r'\s+','',source)
    expression=re.search(r'pubconstfnunix_seconds\(raw:u32\)->u64\{rawasu64\+([0-9_]+)\}',compact)
    assert expression, 'proof source guard: epoch operation changed'
    offset=int(expression[1].replace('_',''))
    assert re.search(r'pubfnepoch_time\(raw:u32\)->Cell<\'static>\{number\(raw\.into\(\),raw\.into\(\),0,"known","s_since_2019_01_01","1",?\)\}',compact), 'proof source guard: epoch cell changed'
    raw=z3.BitVec('received_u32',32)
    logical=z3.BV2Int(raw)
    actual=z3.BV2Int(z3.ZeroExt(32,raw)+z3.BitVecVal(offset,64))
    solver=z3.Solver();solver.add(actual != logical+1546300800)
    assert solver.check()==z3.unsat
    signed=z3.BV2Int(z3.ZeroExt(32,raw),is_signed=True)
    solver=z3.Solver();solver.add(signed != logical)
    assert solver.check()==z3.unsat
    octets=[z3.BitVec(f'octet_{i}',8) for i in range(4)]
    assembled=z3.Concat(*reversed(octets))
    independent=sum(z3.BV2Int(b)*256**i for i,b in enumerate(octets))
    solver=z3.Solver();solver.add(z3.BV2Int(assembled)!=independent)
    assert solver.check()==z3.unsat
    # Prove the obligation is sensitive to the defect it claims to exclude.
    solver=z3.Solver();solver.add(z3.BV2Int(raw+z3.BitVecVal(offset,32))!=logical+1546300800)
    assert solver.check()==z3.sat
    witness=solver.model().eval(raw).as_long()
    return {'obligations_passed':3,'obligations_total':3,'domain_per_obligation':2**32,
        'wrong_width_mutant':'SAT','wrong_width_witness':witness,
        'source_sha256':hashlib.sha256(source_path.read_bytes()).hexdigest(),
        'scope':'u32 arithmetic, widening and four-byte extraction; executable cases check actual field offsets and statuses'}

def main():
    parser=argparse.ArgumentParser();parser.add_argument('bridge');args=parser.parse_args()
    bridge=subprocess.Popen([args.bridge],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
    def ask(command):
        bridge.stdin.write(json.dumps(command,separators=(',',':'))+'\n');bridge.stdin.flush()
        result=json.loads(bridge.stdout.readline())
        assert 'ok' in result,result
        return result['ok']
    vectors=json.loads((HERE/'fixtures/golden-messages.json').read_text())['vectors']
    observations=json.loads((HERE/'fixtures/golden-observations.json').read_text())['fixtures']
    observations+=json.loads((HERE/'fixtures/supplemental-observations.json').read_text())['fixtures']
    records=[(x['name'],x['expected'],True) for x in vectors]+[(x['name'],x['record'],False) for x in observations]
    assert len(records)==182
    validator=jsonschema.Draft202012Validator
    for name,record,message in records:
        s=MESSAGE if message else SCHEMA
        errors=list(validator(s).iter_errors(record));assert not errors,(name,errors[:2])
        encoded=ask({'op':'codec','record':record,'message':message})
        expected_json=json.dumps(record,ensure_ascii=False,sort_keys=True,separators=(',',':')).encode()
        assert bytes.fromhex(encoded['json_hex'])==expected_json,name
        expected_cbor=cbor2.dumps(to_cbor(record,s),canonical=True)
        assert bytes.fromhex(encoded['cbor_hex'])==expected_cbor,name
        assert from_cbor(cbor2.loads(bytes.fromhex(encoded['cbor_hex'])))==record,name
        assert ask({'op':'decode_cbor','hex':expected_cbor.hex(),'message':message})==record,name
    for vector in vectors:
        assert ask({'op':'decode_message','hex':vector['raw_message_hex']})==vector['expected'],vector['name']
    bridge.stdin.close();assert bridge.wait()==0
    print(json.dumps({'independent_schema':{'passed':182,'total':182},'independent_codec':{'passed':182,'total':182},
        'independent_message_expected':{'passed':175,'total':175},'u32_proof':proof()},indent=2))

if __name__=='__main__':
    main()
