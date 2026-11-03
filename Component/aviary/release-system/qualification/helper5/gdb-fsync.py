# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Run via GDB 16 Python. Native helper bytes stay unmodified."""
import gdb, json, os, platform, sys, time
from pathlib import Path
KIT=Path(__file__).resolve().parent
sys.path.insert(0,str(KIT))
mode=os.environ['B03_MODE']
out=Path(os.environ['B03_GDB_OUT']);out.mkdir(mode=0o700,parents=True,exist_ok=True)
pending={};events=[];pairs=[];seen=[];last={};exited=[]
portable=(mode=='calibrate-portable')
calibrate=mode in ('calibrate','calibrate-portable')

def save(name,value):
    with (out/name).open('w') as f:
        json.dump(value,f,sort_keys=True,indent=2);f.write('\n');f.flush();os.fsync(f.fileno())
def signed64(value):
    n=int(value)&((1<<64)-1)
    return n-(1<<64) if n>>63 else n

def main():
    from binding import guard
    guard()
    native=platform.machine() in ('aarch64','arm64')
    if not native and not portable:raise RuntimeError('native ARM64 required; portable calibration is not qualification')
    argreg,retreg,number=('$x0','$x0',82) if native else ('$rdi','$rax',74)
    if not calibrate:
        import fixture
        fixture.guard();fixture.check_baseline(os.environ['B03_PROFILE'])
        target=os.environ['B03_PHASE'];assert target in fixture.PHASES
    def stop(event):
        last.clear();last.update(getattr(event,'details',{}))
    def exit_event(event):exited.append(getattr(event,'exit_code',None))
    gdb.events.stop.connect(stop);gdb.events.exited.connect(exit_event)
    for c in ['set confirm off','set pagination off','set non-stop off','set follow-fork-mode parent','set detach-on-fork on','set print thread-events off','set disable-randomization off','handle SIGURG nostop noprint pass']:
        gdb.execute(c)
    gdb.execute('catch syscall fsync')
    catch=gdb.breakpoints()[-1].number
    command='run'
    for _ in range(2048):
        last.clear();gdb.execute(command,to_string=True);command='continue'
        if exited:
            if not calibrate:raise RuntimeError('helper exited before requested durable point')
            assert exited[-1]==0 and not pending,'calibrator exit/pairing failure'
            expected=[json.loads(x) for x in (out/'expected.jsonl').read_text().splitlines()]
            assert len(expected)==2 and len(pairs)==18
            for row in expected:
                got=[x for x in pairs if x['ptid'][1]==row['tid']]
                assert len(got)==9
                for fd,path,ret in [(row['dirfd'],row['dir'],0),(row['filefd'],row['file'],0),(-1,None,-9)]:
                    assert sum(x['fd']==fd and x['path']==path and x['return']==ret for x in got)==3,(row,fd,got)
            save('result.json',dict(status='PASS',native_arm64=native,mode=mode,registers=[argreg,retreg],syscall=number,threads=2,pairs=len(pairs),pending=0))
            return
        reason=last.get('reason')
        if reason not in ('syscall-entry','syscall-return') or int(last.get('syscall-number',-1))!=number or int(last.get('bkptno',-1))!=catch:
            raise RuntimeError('unexpected stop; refuse to infer entry/return: '+repr(last))
        inf=gdb.selected_inferior();thread=gdb.selected_thread();key=tuple(thread.ptid)
        assert all(t.is_stopped() for t in inf.threads()),'not all-stop'
        event=dict(reason=reason,ptid=list(key),thread=thread.global_num,syscall=number)
        if reason=='syscall-entry':
            assert key not in pending,'duplicate entry for same OS thread'
            fd=signed64(gdb.parse_and_eval(argreg))
            try:
                path=os.readlink(f'/proc/{inf.pid}/fd/{fd}')
                isdir=os.path.isdir(f'/proc/{inf.pid}/fd/{fd}')
            except FileNotFoundError:
                if not calibrate or fd!=-1:raise
                path=None;isdir=False
            event.update(fd=fd,path=path,is_directory=isdir)
            pending[key]=event.copy()
        else:
            if key not in pending:raise RuntimeError('unpaired return for OS thread '+str(key))
            entry=pending.pop(key);ret=signed64(gdb.parse_and_eval(retreg))
            event.update(fd=entry['fd'],path=entry['path'],is_directory=entry['is_directory'],return_value=ret)
            pair=dict(entry);pair['return']=ret;pairs.append(pair)
            if not calibrate:
                if ret!=0:raise RuntimeError('failed fsync; no durable-boundary claim')
                if entry['is_directory']:
                    phase=fixture.state_phase(os.environ['B03_PROFILE'])
                    dirs={'prepared':str(fixture.U),'switched':str(fixture.B.parent),'receipted':str(fixture.C),'floored':str(fixture.U),'committed':str(fixture.U)}
                    if phase and entry['path']==dirs[phase]:
                        if phase not in seen:
                            assert phase==fixture.PHASES[len(seen)],('phase skipped',seen,phase)
                            seen.append(phase)
                        event['phase']=phase
                        if phase==target:
                            fixture.check_protected()
                            events.append(event);save('events.json',events);save('pairs.json',pairs)
                            save('at-stop.json',dict(phase=phase,profile=os.environ['B03_PROFILE'],seen=seen,syscall_return=ret,entry=entry,all_threads_stopped=True,state=fixture.snapshot()))
                            # No continue between this successful directory return and kill.
                            gdb.execute('kill',to_string=True)
                            assert not gdb.selected_inferior().pid,'inferior still alive'
                            save('result.json',dict(status='PASS',mode=mode,phase=phase,profile=os.environ['B03_PROFILE'],native_arm64=True,seen=seen,killed=True))
                            return
        events.append(event)
    raise RuntimeError('too many syscall stops')
try:
    main();save('events.json',events);save('pairs.json',pairs)
except BaseException as error:
    save('events.json',events);save('pairs.json',pairs)
    save('result.json',dict(status='FAIL',error=str(error),mode=mode,seen=seen,pending=[str(x) for x in pending]))
    try:
        if gdb.selected_inferior().pid:gdb.execute('kill',to_string=True)
    finally:gdb.execute('quit 1')
