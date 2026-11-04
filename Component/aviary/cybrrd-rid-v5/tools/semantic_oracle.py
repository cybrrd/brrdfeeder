# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Independent pinned-design semantic oracle, run against offline decoder output.

Display and lake registry checks are contract checks, not application execution.
"""
import copy,json,re
import jsonschema

GROUPS={'bt4_legacy','bt5_primary','bt5_secondary','wifi_nan_sync','wifi_nan_sdf','wifi_beacon'}
NETWORK_FIELDS={'operating_area_polygon','operating_area_type','operation_area_start','operation_area_end'}
PAGE0={'page0_reserved','last_page','length','timestamp','auth_data_page0'}
BODIES={'basic_id','location','auth','self_id','system','operator_id'}

def na(v):
    assert v['status']=='not_applicable' and v['raw'] is None and v['decoded'] is None
    assert isinstance(v['reason'],str) and v['reason']

def text_rule(raw,empty):
    prefix,sep,tail=raw.partition(b'\0')
    if raw==b'\0'*len(raw):return '', 'unknown', empty
    if any(n<32 or n>126 for n in prefix):return None,'out_of_range','invalid_ascii_text'
    if sep and any(tail):return prefix.decode('ascii'),'out_of_range','nonzero_after_nul'
    if not prefix.strip(b' '):return prefix.decode('ascii'),'unknown','whitespace_only_text'
    return prefix.decode('ascii'),'known',None

def registry_check(columns):
    paths={r['column_path'] for r in columns};assert len(paths)==len(columns)
    prefixes={p[:m.start()] for p in paths for m in re.finditer(r'\[\]',p)}
    for stem in prefixes:
        assert {stem+'.$present',stem+'.$length',stem+'[].$ordinal'}<=paths,stem
    return sorted(prefixes)

def run(schema,messages,fixtures,columns,display_cases,association_schema,association_event):
    obs_validator=jsonschema.Draft202012Validator(schema)
    msg_schema=schema['properties']['rid_units']['items']['properties']['messages']['items']
    msg_validator=jsonschema.Draft202012Validator(msg_schema)
    by_name={v['name']:v for v in messages}
    checks=0
    def schema_columns(node,path):
        if 'anyOf' in node:
            node=next(n for n in node['anyOf'] if n.get('type')!='null')
        kind=node.get('type')
        if kind=='object':
            yield path+'.$present'
            for key,child in node['properties'].items():yield from schema_columns(child,path+'.'+key)
        elif kind=='array':
            yield path+'.$present';yield path+'.$length';yield path+'[].$ordinal'
            yield from schema_columns(node['items'],path+'[]')
        else:yield path
    required_columns=set(schema_columns(schema,'v5'))|set(schema_columns(association_schema,'capture_association'))
    network_columns=sorted(p for p in required_columns if p.startswith('v5.network_information'))
    assert len(network_columns)==34
    def registry_full_check(rows):
        assert required_columns<={r['column_path'] for r in rows},'named schema column missing'
        return registry_check(rows)

    def message_check(m):
        msg_validator.validate(m);raw=bytes.fromhex(m['raw_hex']);assert len(raw)==25
        assert 'header'in m
        version=raw[0]&15
        assert m['header']['protocol_version']['raw']==version
        assert m['header']['protocol_version']['status']==('known' if version==2 else 'private_use' if version==15 else 'unsupported_version')
        if version!=2:
            assert m['parse_status']=='unsupported_version' and not BODIES.intersection(m)
            return
        typ=raw[0]>>4
        if typ==0:
            b=m['basic_id'];v=b['uas_id'];r=raw[2:22];k=raw[1]>>4
            assert v['raw']==r.hex() and b['id_type']['raw']==k
            if k in (1,2):expected=text_rule(r,'empty_identifier')
            elif k==0:expected=(r.hex(),'unknown','id_type_none_empty' if not any(r) else 'id_type_none_with_bytes')
            elif not any(r):expected=(r.hex(),'unknown','empty_identifier')
            elif k>=5:expected=(r.hex(),'reserved','reserved_id_type_opaque_bytes')
            elif k==4:expected=(r.hex(),'out_of_range' if r[0]==0 else 'private_use' if r[0]>=225 else 'known','session_method_reserved' if r[0]==0 else 'session_method_private' if r[0]>=225 else 'session_method_profile_required')
            else:expected=(r.hex(),'known',None)
            assert (v['decoded'],v['status'],v['reason'])==expected,(k,expected,v)
        elif typ in (1,4):
            body='location' if typ==1 else 'system'
            fields=[('pressure_altitude',13),('geodetic_altitude_hae',15),('height',17)] if typ==1 else [('area_ceiling_hae',13),('area_floor_hae',15),('operator_altitude_hae',18)]
            from decimal import Decimal
            for name,offset in fields:
                value=m[body][name];code=int.from_bytes(raw[offset:offset+2],'little')
                assert value['raw']==code
                expected=format((Decimal(code)/2-1000).normalize(),'f')
                assert value['decoded']==value['arithmetic_value']==expected
                assert value['status']==('unknown' if code==0 else 'out_of_stated_range' if code==65535 else 'known')
                if code==65535:assert value['reason']=='arithmetic_exceeds_stated_range'
        elif typ==2:
            a=m['auth'];page=raw[1]&15;k=raw[1]>>4
            assert PAGE0|{'auth_type','data_page_number','auth_data_pages_1_15'}<=set(a)
            active=PAGE0 if page==0 else {'auth_data_pages_1_15'}
            inactive={'auth_data_pages_1_15'} if page==0 else PAGE0
            for key in inactive:na(a[key])
            for key in active:assert a[key]['status']!='not_applicable' and a[key]['raw'] is not None
            key='auth_data_page0' if page==0 else 'auth_data_pages_1_15';v=a[key];r=raw[8:] if page==0 else raw[2:]
            assert v['raw']==r.hex() and v['decoded']==r.hex()
            if k==0:expected=('unknown','auth_type_none_empty') if not any(r) else ('out_of_range','auth_type_none_with_data')
            elif k==4:expected=('known','network_auth_requires_null_data') if not any(r) else ('out_of_range','network_auth_nonzero_data')
            else:expected=('known',None)
            assert (v['status'],v['reason'])==expected
        elif typ in (3,5):
            body,key,stop,empty=('self_id','description',25,'empty_description') if typ==3 else ('operator_id','operator_id',22,'empty_operator_identifier')
            v=m[body][key];r=raw[2:stop];assert v['raw']==r.hex()
            assert (v['decoded'],v['status'],v['reason'])==text_rule(r,empty)

    captures={json.dumps(f['record']['captures'][0]['capture_id'],sort_keys=True):f['record']['captures'][0] for f in fixtures}
    assert len(captures)==len(fixtures)
    pair_kinds={'nan_sdf_to_sync':('wifi_nan_sdf','wifi_nan_sync'),'nan_sync_to_sdf':('wifi_nan_sync','wifi_nan_sdf'),
      'bt5_secondary_to_primary':('bt5_secondary','bt5_primary'),'bt5_primary_to_secondary':('bt5_primary','bt5_secondary')}
    def edge_check(e,source_id=None):
        if source_id is not None:assert e['source_capture_id']==source_id
        source=captures.get(json.dumps(e['source_capture_id'],sort_keys=True))
        target=captures.get(json.dumps(e['target_capture_id'],sort_keys=True))
        assert source is not None and target is not None,'fixture relation must resolve both real capture records'
        assert e['source_capture_id']!=e['target_capture_id']
        assert (source['transport_kind'],target['transport_kind'])==pair_kinds[e['relationship']]
        if e['order_basis']=='receiver_capture_sequence':
            a=e['source_capture_id']['observation_id'];b=e['target_capture_id']['observation_id']
            assert (a['node_id'],a['boot_id'])==(b['node_id'],b['boot_id'])
            assert e['source_receive_order']==a['capture_sequence'] and e['target_receive_order']==b['capture_sequence']
        else:assert e['source_receive_order'] is None and e['target_receive_order'] is None
        assert e['evidence'] and e['provenance_version']

    def observation_check(x):
        obs_validator.validate(x);assert len(x['captures'])==1
        assert set(x['network_information'])==NETWORK_FIELDS
        for value in x['network_information'].values():
            na(value);assert value['reason']=='not_applicable_to_broadcast'
        c=x['captures'][0];assert c['capture_id']=={'observation_id':x['observation_id'],'capture_index':c['index']}
        assert set(c['transport_fields'])==GROUPS
        for group,fields in c['transport_fields'].items():
            if group!=c['transport_kind']:
                for v in fields.values():na(v)
            else:
                for v in fields.values():assert v['status']!='not_applicable'
        for i,e in enumerate(c['related_captures']):assert e['index']==i;edge_check(e,c['capture_id'])
        for u in x['rid_units']:
            for m in u['messages']:message_check(m)

    for v in messages:message_check(v['expected']);checks+=1
    for f in fixtures:observation_check(f['record']);checks+=1
    jsonschema.validate(association_event,association_schema);edge_check(association_event['edge']);checks+=1
    prefixes=registry_full_check(columns);checks+=1

    def display_result(case):
        candidates={k:[] for k in range(1,5)};raw_rows=[];operators=[]
        for ref in case['message_vectors']:
            raw=bytes.fromhex(by_name[ref]['raw_message_hex']);typ=raw[0]>>4
            if typ==0:
                kind=raw[1]>>4;r=raw[2:22]
                raw_rows.append({'vector':ref,'raw_hex':r.hex()})
                if kind in (1,2):
                    value,state,_=text_rule(r,'empty_identifier')
                    if state=='known':candidates[kind].append(value)
                elif kind==3 and any(r):
                    h=r[:16].hex();candidates[3].append('-'.join((h[:8],h[8:12],h[12:16],h[16:20],h[20:])))
                elif kind==4 and r[0]!=0:candidates[4].append(r.hex())
            elif typ==5:operators.append(text_rule(raw[2:22],'empty_operator_identifier')[0])
        serial_values=set(candidates[1]);hardware=next(iter(serial_values)) if len(serial_values)==1 else None
        kind_names={1:'serial',2:'caa_registration',3:'utm_uuid',4:'session_id'}
        selected=next((k for k in (1,2,3,4) if candidates[k]),None)
        if selected is None:
            drone=case['source_mac'] or case['entity_ref'];kind='source_mac' if case['source_mac'] else 'entity_ref'
        elif len(set(candidates[selected]))>1:drone=case['entity_ref'];kind='conflicting_'+kind_names[selected]
        else:drone=candidates[selected][0];kind=kind_names[selected]
        return {'hardware_serial':hardware,'drone_id':drone,'drone_id_kind':kind,'serial_texts':candidates[1],
          'basic_id_occurrences':len(raw_rows),'raw_identifier_rows':raw_rows,'operator_id_texts':operators,
          'hidden_fields':['operator_position'],
          'visible_identifier_fields':['hardware_serial','caa_registration','utm_uuid','session_id','basic_id_raw','source_MAC','operator_id'],
          'positive_browser_surfaces':['selected_aircraft_detail','identifier_panel','copy_value','accessibility_text','identifier_export']}
    def display_check(c):assert display_result(c)==c['expected'],c['name']
    for c in display_cases:display_check(c);checks+=1

    negatives=[]
    def reject(name,base,mutate,verify):
        changed=copy.deepcopy(base);mutate(changed)
        try:verify(changed)
        except (AssertionError,jsonschema.ValidationError):pass
        else:raise AssertionError('semantic mutation escaped: '+name)
        negatives.append({'case':name,'expected':'reject','observed':'rejected'})
    def msg(name):return by_name[name]['expected']
    assert not any('alternative_numeric_pending_Cy' in name for name in by_name)
    for vector,body,fields in [('location_alt_raw_65535','location',['pressure_altitude','geodetic_altitude_hae','height']),('system_alt_raw_65535','system',['area_ceiling_hae','area_floor_hae','operator_altitude_hae'])]:
        for field in fields:
            for label,change in [('null_decoded',{'decoded':None}),('wrong_decoded',{'decoded':'31767'}),('wrong_arithmetic',{'arithmetic_value':'31767'}),('known_status',{'status':'known'}),('ambiguous_status',{'status':'ambiguous'}),('missing_reason',{'reason':None}),('changed_raw',{'raw':65534})]:
                reject('ratified_altitude_B:'+body+'.'+field+':'+label,msg(vector),lambda m,b=body,f=field,c=change:m[b][f].update(c),message_check)
    for name,body,field in [('basic_id_type_0','basic_id','uas_id'),('basic_id_type_0_nul_only','basic_id','uas_id'),('serial_nul_only','basic_id','uas_id'),('auth_type_0','auth','auth_data_page0'),('self_id_nul_only','self_id','description'),('operator_id_nul_only','operator_id','operator_id'),('serial_whitespace','basic_id','uas_id'),('serial_invalid_text','basic_id','uas_id'),('serial_embedded_nul','basic_id','uas_id')]:
        reject('sentinel_wrongly_known:'+name,msg(name),lambda m,b=body,f=field:m[b][f].update(status='known',reason=None),message_check)
    reject('valid_type4_null_auth_wrongly_unknown',msg('auth_type_4'),lambda m:m['auth']['auth_data_page0'].update(status='unknown'),message_check)
    reject('binary_uuid_zero_prefix_treated_as_empty',msg('uuid_zero_prefix'),lambda m:m['basic_id']['uas_id'].update(status='unknown',decoded=''),message_check)
    reject('private_version15_wrong_cell_status',msg('header_version_15'),lambda m:m['header']['protocol_version'].update(status='unsupported_version'),message_check)
    for key in sorted(PAGE0):
        reject('continuation_missing_NA:'+key,msg('auth_page_1'),lambda m,k=key:m['auth'].pop(k),message_check)
    reject('page0_missing_continuation_NA',msg('auth_page_0'),lambda m:m['auth'].pop('auth_data_pages_1_15'),message_check)
    reject('NA_with_invented_value',msg('auth_page_1'),lambda m:m['auth']['last_page'].update(raw=0,decoded=0),message_check)
    reject('NA_without_reason',msg('auth_page_1'),lambda m:m['auth']['last_page'].update(reason=None),message_check)
    nan=next(f['record'] for f in fixtures if f['name']=='wifi_nan')
    reject('broadcast_missing_network_information',nan,lambda x:x.pop('network_information'),observation_check)
    for field in sorted(NETWORK_FIELDS):
        reject('network_field_missing:'+field,nan,lambda x,k=field:x['network_information'].pop(k),observation_check)
        for label,change in [('not_exposed',{'status':'not_exposed'}),('wrong_reason',{'reason':'unknown'}),('no_reason',{'reason':None}),('fabricated_raw',{'raw':'00'})]:
            reject('broadcast_network:'+field+':'+label,nan,lambda x,k=field,c=change:x['network_information'][k].update(c),observation_check)
        fabricated=[] if field=='operating_area_polygon' else 'cylinder' if field=='operating_area_type' else '2000-01-01T00:00:00Z'
        reject('broadcast_network:'+field+':fabricated_decoded',nan,lambda x,k=field,v=fabricated:x['network_information'][k].update(decoded=v),observation_check)
    for group in sorted(GROUPS):reject('capture_missing_group:'+group,nan,lambda x,g=group:x['captures'][0]['transport_fields'].pop(g),observation_check)
    reject('NAN_sync_folded_into_SDF',nan,lambda x:x['captures'][0]['transport_fields']['wifi_nan_sync']['element_id'].update(raw=221,decoded=221,status='known',reason=None),observation_check)
    reject('combined_capture_events',nan,lambda x:x['captures'].append(copy.deepcopy(next(f['record']['captures'][0] for f in fixtures if f['name']=='wifi_nan_sync_companion'))),observation_check)
    reject('association_missing_source_id',nan,lambda x:x['captures'][0]['related_captures'][0].pop('source_capture_id'),observation_check)
    reject('association_wrong_source_id',nan,lambda x:x['captures'][0]['related_captures'][0]['source_capture_id']['observation_id'].update(capture_sequence='99'),observation_check)
    reject('association_missing_target_capture',nan,lambda x:x['captures'][0]['related_captures'][0]['target_capture_id']['observation_id'].update(capture_sequence='99'),observation_check)
    reject('association_wrong_receive_order',nan,lambda x:x['captures'][0]['related_captures'][0].update(target_receive_order='99'),observation_check)
    reject('association_wrong_relationship',nan,lambda x:x['captures'][0]['related_captures'][0].update(relationship='bt5_secondary_to_primary'),observation_check)
    for prefix in prefixes:
        for suffix in ('.$present','.$length','[].$ordinal'):
            path=prefix+suffix
            reject('lake_missing_list_metadata:'+path,columns,lambda x,p=path:x.__setitem__(slice(None),[r for r in x if r['column_path']!=p]),registry_full_check)
    for path in network_columns:
        reject('network_named_column_missing:'+path,columns,lambda x,p=path:x.__setitem__(slice(None),[r for r in x if r['column_path']!=p]),registry_full_check)
    cases={x['name']:x for x in display_cases}
    reject('serial_display_truncated',cases['complete_serial'],lambda x:x['expected']['serial_texts'].__setitem__(0,'0000F…'),display_check)
    for kind in (0,*range(2,16)):
        reject('nonserial_fallback:'+str(kind),cases['display_basic_id_type_'+str(kind)],lambda x:x['expected'].update(hardware_serial='FALSE-SERIAL',drone_id_kind='serial'),display_check)
    reject('silent_conflict_winner',cases['conflicting_serials'],lambda x:x['expected'].update(hardware_serial='0000FTEST00000000001',drone_id='0000FTEST00000000001',drone_id_kind='serial'),display_check)
    reject('lost_repeated_ID',cases['equal_repeated_serial'],lambda x:x['message_vectors'].pop(),display_check)
    for field in ('hardware_serial','operator_id','operator'):
        reject('overbroad_Globe_hide:'+field,cases['all_identifiers_with_operator_position'],lambda x,f=field:x['expected']['hidden_fields'].append(f),display_check)
    reject('operator_ID_display_missing',cases['all_identifiers_with_operator_position'],lambda x:x['expected']['operator_id_texts'].clear(),display_check)
    return {'semantic_positive_checks':checks,'semantic_negative_controls':len(negatives),'repeated_paths_checked':len(prefixes),'display_cases':len(display_cases),'network_named_columns_checked':len(network_columns),'negative_cases':negatives}
