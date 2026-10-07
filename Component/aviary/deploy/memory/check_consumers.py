#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Optional compatibility proof against a supplied CYB1 checkout; no broker.

Copies the actual consumer modules, adds tests only in temporary copies, and
prints the source commit. It never changes the supplied checkout or deploys.
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile

COMMAND = r'''package main
import ("encoding/json"; "reflect"; "testing")
func TestMemoryUnknownFieldsCompatibility(t *testing.T) {
    raw := []byte(`{"node_id":"memory-fixture","channel":"dev","radio_status":"up","os_clock_trusted":true}`)
    var baseline Heartbeat
    if err := json.Unmarshal(raw, &baseline); err != nil || !validHeartbeat(baseline) { t.Fatal(err) }
    for _, memory := range []string{`{"engine_rss_bytes":14336000,"memory_cap_events":2,"future_unknown":true}`, `null`, `{"future_unknown":[1,2]}`} {
        augmented := append(append([]byte{}, raw[:len(raw)-1]...), []byte(`,"memory":`+memory+`}`)...)
        var got Heartbeat
        if err := json.Unmarshal(augmented, &got); err != nil || !validHeartbeat(got) { t.Fatal(err) }
        if !reflect.DeepEqual(got, baseline) { t.Fatalf("unknown memory altered projection: %+v", got) }
    }
}
'''
GLOBE = r'''package nats
import ("testing"; gonats "github.com/nats-io/nats.go")
func TestMemoryUnknownFieldsCompatibility(t *testing.T) {
    sink := &recordingHeartbeatSink{}
    consumer := NewHeartbeatConsumer(nil, sink, quietLogger())
    for _, memory := range []string{`{"engine_rss_bytes":14336000,"memory_cap_events":2,"future_unknown":true}`, `null`, `{"future_unknown":[1,2]}`} {
        consumer.handle(&gonats.Msg{Subject:"cybrrd.system.node.heartbeat.memory-fixture", Data:[]byte(`{"node_id":"memory-fixture","timestamp_utc":1,"radio_status":"up","memory":`+memory+`}`)})
    }
    if sink.count() != 3 || consumer.Stats().Accepted != 3 || consumer.Stats().Rejected != 0 || consumer.Stats().ParseFailed != 0 { t.Fatalf("memory broke actual heartbeat handler: %+v", consumer.Stats()) }
}
'''

parser = argparse.ArgumentParser()
parser.add_argument('--cyb1-root', type=Path, required=True)
args = parser.parse_args()
root = args.cyb1_root.resolve()
print('Consumer checkout: ' + subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(), flush=True)
with tempfile.TemporaryDirectory(prefix='brrd-memory-consumers-') as folder:
    for component, test_path, source, package in (
        ('command', 'memory_compatibility_test.go', COMMAND, '.'),
        ('globe-backend', 'internal/nats/memory_compatibility_test.go', GLOBE, './internal/nats'),
    ):
        target = Path(folder) / component
        shutil.copytree(root / 'Component' / component, target,
                        ignore=shutil.ignore_patterns('.git', 'target', '__pycache__', 'node_modules'))
        (target / test_path).write_text(source)
        subprocess.run(['go', 'test', '-race', '-count=1', '-run', '^TestMemoryUnknownFieldsCompatibility$', '-v', package], cwd=target, check=True, timeout=180)
