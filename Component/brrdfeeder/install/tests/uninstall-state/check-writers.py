#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Fail closed when packaged filesystem/path expressions change.

Extract Python AST path operations and Go/Rust token-balanced path calls,
literal path names and string bindings from production sources, including new
files. This is a conservative source census, not a hash of selected files or a
grep for names the uninstaller already knows. New/changed operations require
review against the four-tree inventory and executable uninstall fixtures.

The census includes readers and external writers where a dynamic expression
cannot be resolved statically. It deliberately fails on such changes rather
than guessing they are outside scope. Configuration coverage is the installed
defaults plus S/status.json and S/capture; arbitrary configured paths are not
authorized deletion roots. See README.md for the source-model limits.
"""
import argparse
import ast
from collections import Counter
import copy
import json
import fnmatch
import importlib.util
from pathlib import Path
import re
import sys
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[5]
HERE = Path(__file__).resolve().parent
ROOTS = ('/var/lib/brrdfeeder', '/etc/brrdfeeder')
CALLS = {
    # Path construction is included even when the file is opened via a wrapper.
    'Join', 'join', 'with_extension', 'with_file_name', 'Path', 'PathBuf',
    'CreateTemp', 'MkdirTemp', 'OpenFile', 'WriteFile', 'MkdirAll', 'Mkdir',
    'Rename', 'Link', 'Symlink', 'create_dir', 'create_dir_all', 'create',
    'create_new', 'open', 'rename', 'hard_link', 'symlink', 'write',
    'atomicFile', 'atomicJSON', 'writeFile', 'stage', 'write_atomic', 'atomic_write',
    'mkstemp', 'mkdtemp', 'NamedTemporaryFile', 'TemporaryDirectory',
    'mkdir', 'makedirs', 'write_text', 'write_bytes', 'replace', 'atomic',
}
ALL_ARGS = {'Join','join','with_extension','with_file_name','CreateTemp','MkdirTemp',
            'Rename','Link','Symlink','rename','hard_link','symlink','replace',
            'mkstemp','mkdtemp','NamedTemporaryFile','TemporaryDirectory'}
TOKEN = re.compile(r'''(?sx)
    (?P<space>\s+)|(?P<line>//[^\n]*)|(?P<block>/\*.*?\*/)|
    (?P<rustraw>r(?P<hashes>\#*)".*?"(?P=hashes))|
    (?P<string>"(?:\\.|[^"\\])*"|`[^`]*`)|
    (?P<char>'(?:\\.|[^'\\])')|
    (?P<word>[A-Za-z_][A-Za-z_0-9]*|[0-9]+)|(?P<symbol>::|:=|->|=>|\.\.=|\.\.|.)
''')


def tokens(text):
    return [(m.lastgroup, m.group()) for m in TOKEN.finditer(text)
            if m.lastgroup not in ('space','line','block')]


def balanced(ts, start):
    pairs = {'(':')','[':']','{':'}'}
    stack = [pairs[ts[start][1]]]
    for i in range(start+1,len(ts)):
        value=ts[i][1]
        if ts[i][0] in ('string','rustraw','char'): continue
        if value in pairs: stack.append(pairs[value])
        elif value in pairs.values():
            if not stack or value != stack.pop(): raise ValueError('unbalanced source expression')
            if not stack: return i
    raise ValueError('unterminated source expression')


def without_rust_tests(ts):
    result=[]; i=0
    while i<len(ts):
        if ts[i][1]=='#' and i+1<len(ts) and ts[i+1][1]=='[':
            end=balanced(ts,i+1)
            annotation=''.join(v for _,v in ts[i:end+1])
            if annotation in ('#[cfg(test)]','#[test]','#[tokio::test]'):
                # Skip the annotated item, not the rest of the source file.
                i=end+1
                while i+1<len(ts) and ts[i][1]=='#' and ts[i+1][1]=='[':
                    i=balanced(ts,i+1)+1
                probe=i
                if probe<len(ts) and ts[probe][1]=='pub': probe+=1
                if probe+1<len(ts) and ts[probe+1][1]==':':
                    # cfg(test) also decorates individual struct fields. Keep
                    # the enclosing closing brace, including inline literals.
                    while i<len(ts) and ts[i][1] not in (',','}'):
                        if ts[i][1] in ('(','[','{'): i=balanced(ts,i)
                        i+=1
                    if i<len(ts) and ts[i][1]==',': i+=1
                    continue
                while i<len(ts) and ts[i][1] not in ('{',';'): i+=1
                if i<len(ts) and ts[i][1]=='{': i=balanced(ts,i)+1
                else: i+=1
                continue
        result.append(ts[i]); i+=1
    return result


def text(ts):
    # Separators preserve token boundaries without freezing formatting/comments.
    return ' '.join(value for _,value in ts)


def arguments(ts, start, end):
    result=[]; begin=start+1; i=begin
    while i<end:
        if ts[i][0] not in ('string','rustraw','char'):
            if ts[i][1] in ('(','[','{'): i=balanced(ts,i)+1; continue
            if ts[i][1]==',': result.append(ts[begin:i]); begin=i+1
        i+=1
    if begin<end: result.append(ts[begin:end])
    return result


def path_literal(value):
    return (value.startswith('/') and not any(c.isspace() for c in value)
            or re.fullmatch(r'[^\s]+\.(?:json|yaml|creds|token|pcap|lock|tmp|container)', value)
            or value.startswith(('.tmp.', '.update-', '.pending-', '.host-recover-', '.renew-',
                                 '.bluetooth-', '.brrdfeeder-atomic-', '.config-status-', '.release-config-')))


def functions(ts):
    """Named Go/Rust function bodies, including Go receiver methods."""
    result={}
    for i,(_,value) in enumerate(ts):
        if value not in ('fn','func') or i+1>=len(ts): continue
        start=i+1
        if ts[start][1]=='(': start=balanced(ts,start)+1  # Go receiver
        if start>=len(ts) or ts[start][0]!='word': continue
        name=ts[start][1]; body=start+1
        while body<len(ts) and ts[body][1] not in ('{',';'):
            if ts[body][1] in ('(','['): body=balanced(ts,body)
            body+=1
        if body<len(ts) and ts[body][1]=='{':
            end=balanced(ts,body)
            result.setdefault(name,[]).extend(ts[body+1:end])
    return result


def wrapper_calls(sources):
    """Conservatively follow project wrappers around path/filesystem operations.

    A newly added call to an existing writer wrapper must be registered even
    when its path is a bare name, a variable, or a formatted argument.
    """
    graph={}
    for source,rust in sources:
        ts=tokens(source)
        if rust: ts=without_rust_tests(ts)
        for name,body in functions(ts).items():
            calls={body[i-1][1] for i in range(1,len(body)) if body[i][1]=='('}
            graph.setdefault(name,set()).update(calls)
    writers=set(CALLS)
    while True:
        added={name for name,calls in graph.items() if calls & writers}-writers
        if not added: return writers-set(CALLS)
        writers.update(added)


def lexical(source, rust=False, wrappers=()):
    ts=tokens(source)
    if rust: ts=without_rust_tests(ts)
    result=[]
    for i,(kind,value) in enumerate(ts):
        if kind in ('string','rustraw'):
            if path_literal(value.strip('"`')): result.append('literal '+value)
            if i>=2 and ts[i-1][1] in ('=',':=','=>','return'):
                result.append('binding '+text(ts[max(0,i-2):i+1]))
        if value in ('..','..=') and i and i+1<len(ts):
            result.append('range '+text(ts[i-1:i+2]))
        # Literal arrays used by lock-name loops (and other finite families).
        if value=='[':
            end=balanced(ts,i)
            if any(k in ('string','rustraw') for k,_ in ts[i:end+1]):
                result.append('array '+text(ts[i:end+1]))
        if value=='{' and i>=3 and text(ts[i-3:i])=='[ ] string':
            end=balanced(ts,i); result.append('array '+text(ts[i-3:end+1]))
        if value!='(' or not i: continue
        name=ts[i-1][1]
        constructor=name in ('new','from') and i>=3 and ts[i-3][1] in ('Path','PathBuf','CString')
        # Ignore declarations; calls have a real receiver/argument expression.
        if i>=2 and ts[i-2][1] in ('fn','func'): continue
        end=balanced(ts,i)
        args=arguments(ts,i,end)
        literal_argument=any(kind in ('string','rustraw') for arg in args for kind,_ in arg)
        if name not in CALLS and name not in wrappers and not constructor and not literal_argument: continue
        selected=args if name in ALL_ARGS or name in wrappers or name not in CALLS else args[:1]
        receiver=text(ts[max(0,i-3):i]) if i>=2 and ts[i-2][1] in ('.','::') else name
        result.append('call '+receiver+' ('+', '.join(text(a) for a in selected)+')')
    return dict(sorted(Counter(result).items()))


class StableFStrings(ast.NodeTransformer):
    def visit_JoinedStr(self,node):
        # Python 3.12/3.13 choose different f-string delimiters when nested
        # expressions contain quotes. Compare their structure, not that printer
        # choice. No synthetic call is executed; this is a census rendering.
        values=[]
        for value in node.values:
            if isinstance(value,ast.FormattedValue):
                values.append(ast.Tuple(elts=[self.visit(value.value),ast.Constant(value.conversion),
                              self.visit(value.format_spec) if value.format_spec else ast.Constant(None)],ctx=ast.Load()))
            else: values.append(value)
        return ast.Call(func=ast.Name(id='fstring',ctx=ast.Load()),args=values,keywords=[])


def render(node):
    if any(isinstance(n,ast.JoinedStr) for n in ast.walk(node)):
        node=StableFStrings().visit(copy.deepcopy(node))
    return ast.unparse(node)


def python(source):
    tree=ast.parse(source)
    result=[]
    for node in ast.walk(tree):
        if isinstance(node,ast.Constant) and isinstance(node.value,str) and path_literal(node.value):
            result.append('literal '+repr(node.value))
        elif isinstance(node,(ast.Assign,ast.AnnAssign)) and isinstance(node.value,ast.Constant) and isinstance(node.value.value,str):
            result.append('binding '+render(node))
        elif isinstance(node,ast.Return) and isinstance(node.value,ast.Constant) and isinstance(node.value.value,str):
            result.append('return '+render(node.value))
        elif isinstance(node,ast.BinOp) and isinstance(node.op,ast.Div):
            result.append('path/div '+render(node))
        elif isinstance(node,ast.Call):
            name=node.func.attr if isinstance(node.func,ast.Attribute) else node.func.id if isinstance(node.func,ast.Name) else ''
            literal_argument=any(isinstance(n,ast.Constant) and isinstance(n.value,str) for arg in node.args for n in ast.walk(arg))
            if name not in CALLS and not literal_argument: continue
            args=node.args if name in ALL_ARGS or name not in CALLS else node.args[:1]
            # Path.write_* takes data, not a path; its receiver identifies it.
            if name in ('write_text','write_bytes'): args=[]
            result.append('call '+render(node.func)+' ('+', '.join(render(a) for a in args)
                          + (', ' if args and node.keywords else '')
                          +', '.join(render(k) for k in node.keywords)+')')
    return dict(sorted(Counter(result).items()))


def census(root):
    paths=[]
    for relative,extension in [('Component/aviary/engine/src','.rs'),
                               ('Component/aviary/release-system','.go'),
                               ('Component/brrdhouse','.go'),
                               ('Component/aviary/release-system','.py'),
                               ('Component/aviary/deploy/memory','.py'),
                               ('Component/brrdfeeder/install','.py')]:
        for path in (root/relative).rglob('*'+extension):
            parts=path.relative_to(root/relative).parts
            if any(p in ('tests','target','__pycache__') for p in parts): continue
            if path.name.endswith(('_test.go','_test.py','_tests.rs')) or path.name.startswith(('test_','test-','mutate_','uninstall')): continue
            paths.append(path)
    result={}
    canonical_python=[p.read_text().strip() for p in (root/'Component/brrdfeeder/install').glob('*.py')]
    wrappers=wrapper_calls([(p.read_text(),p.suffix=='.rs') for p in paths if p.suffix in ('.go','.rs')])
    for path in sorted(set(paths)):
        source=path.read_text()
        if path.suffix=='.py':
            canonical_python.append(source.strip()); items=python(source)
        else: items=lexical(source,path.suffix=='.rs',wrappers)
        result[path.relative_to(root).as_posix()]=items
    # The installer contains additional Python transforms, beyond canonical
    # embedded helpers. Parse every heredoc that is valid Python; no execution.
    install=root/'Component/brrdfeeder/install/brrdfeeder-install.sh'
    source=install.read_text()
    for match in re.finditer(r"<<-?'?([A-Z][A-Z0-9_]*)'?[^\n]*\n",source):
        label=match[1]; end=re.search(r'^'+re.escape(label)+r'$',source[match.end():],re.M)
        if not end: continue
        body=source[match.end():match.end()+end.start()].strip()
        if body in canonical_python: continue
        try: items=python(body)
        except (SyntaxError,ValueError): continue
        if items:
            key=install.relative_to(root).as_posix()+'::'+label
            if key in result: raise ValueError('ambiguous Python heredoc '+key)
            result[key]=items
    return result


def destinations(extracted):
    """Resolve literal roots/Go joins and expand reviewed dynamic writer families.

    Unresolved expressions still participate in the fail-closed census. These
    projections cover installed defaults and the two documented optional roots;
    they are not an attempt to evaluate arbitrary caller configuration.
    """
    scopes=('/var/lib/brrdfeeder','/var/lib/brrdfeeder-updater',
            '/var/lib/brrdfeeder-status','/etc/brrdfeeder')
    s,p,t,e=map(Path,scopes)
    output=set()
    def add(value):
        if any(value==base or value.startswith(base+'/') for base in scopes):
            output.add(Path(value))
    for source,items in extracted.items():
        for expression in items:
            if expression.startswith('literal '):
                try: value=ast.literal_eval(expression[len('literal '):])
                except (ValueError,SyntaxError): continue
                if isinstance(value,str) and not any(c in value for c in '*{}\n'):
                    add(value)
            if expression.startswith('call filepath . Join ('):
                match=re.fullmatch(r'call filepath \. Join \(([^,]+), "([^"/]+)"\)',expression)
                if match:
                    base,name=match.groups()
                    if 'StateDir' in base: output.add(s/name)
                    elif 'PrivateDir' in base or base=='p . private': output.add(p/name)
                    elif base=='dir' and source.endswith('/updater.go'):
                        output.add(p/'rejected-pending-123'/name)
    # Samples come from the extracted writer spellings, not uninstall's list.
    # Fixed aliases below resolve generic dir/path parameters in these writers.
    for source,items in extracted.items():
        joined='\n'.join(items)
        if source.endswith('/storage.go'):
            for prefix,parents in [('.update-',(s,p,e)),('.pending-',(p,))]:
                if '"'+prefix+'*"' in joined:
                    output.update(parent/(prefix+'123') for parent in parents)
        if source.endswith('/launcher.py') and "'.host-recover-'" in joined:
            output.update(parent/'.host-recover-abcd_123' for parent in (p,e))
        if source.endswith('/credential_renewal.rs'):
            # ACTIVE supplies the actual credential directory; joins supply the
            # child names. The public-key placeholder is a valid synthetic key.
            active=next((path for path in output if path.name=='brrdfeeder.creds' and path.parent.name=='credentials'),None)
            if active:
                output.add(active.parent)
                for expression in items:
                    match=re.fullmatch(r'call dir \. join \("([^"/]+)"\)',expression)
                    if match: output.add(active.parent/match[1])
                if '".renew-{}"' in joined:
                    from fixture import RENEW
                    output.add(active.parent/RENEW)
        if source.endswith('/forensic.rs') and '"capture-{slot:02}.pcap"' in joined:
            output.update(s/'capture'/f'capture-{n:02}.pcap' for n in range(16))
        if source.endswith('/status.rs') and '".tmp.{}.{}"' in joined:
            for parent in (s,t):
                output.update((parent/'status.json',parent/'status.json.tmp.101.0'))
        if source.endswith('/upward.rs'):
            for name in ('operational','red'):
                if '=> "'+name+'"' in joined:
                    output.update(s/'upward'/(name+'.'+suffix) for suffix in ('json','lock','tmp'))
        if source.endswith(('/outage_spool.rs','/telemetry_loss.rs')):
            output.update(s/'outage'/name for name in ('lock','cursor.json','cursor.json.tmp','gaps.slots','00000000000000000001.seg'))
        if source.endswith('/blue_policy.rs') and '".{fname}.tmp"' in joined:
            output.add(s/'.policy_state.json.tmp')
        if source.endswith('/gps-seed.py') and "prefix='.' + path.name + '-'" in joined:
            output.update((e/'.config.yaml-abcd_123',t/'.startup.json-abcd_123'))
    return output


def dispatch(helper,tree,name):
    marker='for entry in '+str(tree)+'/*; do\n'
    body=helper.split(marker,1)[1].split('\ndone',1)[0]
    body=body.split('case ${entry##*/} in',1)[1].split('esac',1)[0]
    for match in re.finditer(r'(?:^|\n)\s*([^\n)]+)\)\s*(.*?);;',body,re.S):
        patterns,action=match.groups()
        if any(fnmatch.fnmatchcase(name,p.strip()) for p in patterns.split('|')):
            if 'die ' in action and action.lstrip().startswith('die '):
                raise ValueError('uncovered writer path: '+str(tree/name))
            if action.lstrip().startswith('refuse_entry '):
                raise ValueError('uncovered writer path: '+str(tree/name))
            return action
    raise ValueError('no uninstall dispatch for '+str(tree/name))


def check_policy(root,paths):
    install=root/'Component/brrdfeeder/install'
    helper=(install/'uninstall.sh').read_text()
    validator=(install/'uninstall-state.py').read_text()
    embedded=helper.split("<<'PACKAGE_STATE_EOF'",1)[1].split('\n',1)[1].split('\nPACKAGE_STATE_EOF',1)[0]+'\n'
    if embedded!=validator: raise ValueError('embedded package-state validator differs')
    spec=importlib.util.spec_from_file_location('state_validator',install/'uninstall-state.py')
    module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    s,p,t,e=map(Path,('/var/lib/brrdfeeder','/var/lib/brrdfeeder-updater','/var/lib/brrdfeeder-status','/etc/brrdfeeder'))
    for path in sorted(paths):
        tree=next((base for base in (s,p,t,e) if path==base or path.is_relative_to(base)),None)
        if tree is None: raise ValueError('invalid inventory projection '+str(path))
        if path==tree or tree==t: continue  # T narrowing is explicitly PR B.
        relative=path.relative_to(tree)
        action=dispatch(helper,tree,relative.parts[0])
        if 'safe_added_state' in action:
            # Exercise actual name/config dispatch without privileged metadata.
            # Real ownership, modes, mounts and removal run in test-container.py.
            entry=tree/relative.parts[0]
            children=[path] if path!=entry else []
            with patch.object(module,'owned_file',return_value=SimpleNamespace(st_nlink=1)), \
                 patch.object(module,'owned_directory'),patch.object(module,'configured',return_value=True), \
                 patch.object(Path,'iterdir',return_value=iter(children)):
                module.validate(entry,999,985)
        elif len(relative.parts)>1:
            if tree==s and relative.parts[0]=='upward': dispatch(helper,s/'upward',relative.parts[1])
            elif tree==e and relative.parts[0]=='secrets': dispatch(helper,e/'secrets',relative.parts[1])
            elif tree==p and relative.parts[0].startswith('rejected-pending-'): pass  # PR B provenance.
            else: raise ValueError('unmapped writer subtree: '+str(path))


def check(root):
    path=root/'Component/brrdfeeder/install/tests/uninstall-state/writer-paths.json'
    expected=json.loads(path.read_text())
    actual=census(root)
    if expected!=actual:
        changes=[]
        for source in sorted(set(expected)|set(actual)):
            old=expected.get(source,{}); new=actual.get(source,{})
            for expression in sorted(set(old)|set(new)):
                if old.get(expression)!=new.get(expression):
                    changes.append(source+': '+expression+f' [{old.get(expression,0)} -> {new.get(expression,0)}]')
        raise ValueError('unreviewed writer/path expression; extend the uninstall inventory and fixtures before registering it:\n'
                         +'\n'.join(changes[:25]))
    paths=destinations(actual)
    check_policy(root,paths)
    return len(actual),sum(len(v) for v in actual.values()),len(paths)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,default=ROOT)
    args=parser.parse_args()
    try:
        files,expressions,paths=check(args.root)
        print(f'Writer path census PASS: {files} source units, {expressions} extracted expressions, {paths} projected paths accepted by uninstall; see configured-path limits in README.md.')
    except (OSError,ValueError) as error:
        print(str(error),file=sys.stderr); sys.exit(1)
