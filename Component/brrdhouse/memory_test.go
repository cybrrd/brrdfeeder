// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestMemoryEventIsVisible(t *testing.T) {
	c, p := testConsole(t)
	fixture(t, p, 0, 30, false)
	b, err := os.ReadFile(p)
	if err != nil {
		t.Fatal(err)
	}
	var doc map[string]any
	if err := json.Unmarshal(b, &doc); err != nil {
		t.Fatal(err)
	}
	doc["heartbeat"].(map[string]any)["memory"] = map[string]any{
		"engine_rss_bytes": 14000000, "memory_cap_events": 2,
		"future_unknown": "PRIVATE_MEMORY_SENTINEL",
	}
	b, _ = json.Marshal(doc)
	if err := os.WriteFile(p, b, 0644); err != nil {
		t.Fatal(err)
	}
	body := render(c, "/").Body.String()
	if !strings.Contains(body, "Memory-cap events") || !strings.Contains(body, "memory limit") {
		t.Fatalf("memory event was silently discarded: %s", body)
	}
	if strings.Contains(body, "PRIVATE_MEMORY_SENTINEL") {
		t.Fatal("unknown memory field leaked")
	}
}

func TestHostMemoryRemainsVisibleWithoutEngine(t *testing.T) {
	dir := t.TempDir()
	snapshot, boot, uptime := filepath.Join(dir, "host.json"), filepath.Join(dir, "boot"), filepath.Join(dir, "uptime")
	for path, value := range map[string]string{snapshot: `{"schema_version":1,"boot_id":"boot-a","sampled_boottime_secs":100,"memory_cap_events":3}`, boot: "boot-a\n", uptime: "190.5 200.0\n"} {
		if err := os.WriteFile(path, []byte(value), 0644); err != nil {
			t.Fatal(err)
		}
	}
	m := readHostMemory(snapshot, boot, uptime)
	if m == nil || m.Events == nil || *m.Events != 3 {
		t.Fatal("host event omitted")
	}
	v := readStatusWithMemory(filepath.Join(dir, "missing-engine.json"), proofNow, m)
	if v.Live || len(v.Repairs) != 2 || !strings.Contains(v.Repairs[1].Problem, "memory limit") {
		t.Fatalf("offline event lost: %+v", v)
	}
	for _, invalidUptime := range []string{"191 200", "99 200", "NaN 200", "bad", "-1 0"} {
		if err := os.WriteFile(uptime, []byte(invalidUptime), 0644); err != nil {
			t.Fatal(err)
		}
		if readHostMemory(snapshot, boot, uptime) != nil {
			t.Fatalf("accepted stale/future/invalid: %s", invalidUptime)
		}
	}
	_ = os.WriteFile(uptime, []byte("100 200"), 0644)
	_ = os.WriteFile(boot, []byte("boot-b"), 0644)
	if readHostMemory(snapshot, boot, uptime) != nil {
		t.Fatal("prior boot accepted")
	}
}
