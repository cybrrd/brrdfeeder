// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
    "encoding/json"
    "os"
    "strings"
    "testing"
)

func TestMemoryEventIsVisible(t *testing.T) {
    c, p := testConsole(t)
    fixture(t, p, 0, 30, false)
    b, err := os.ReadFile(p)
    if err != nil { t.Fatal(err) }
    var doc map[string]any
    if err := json.Unmarshal(b, &doc); err != nil { t.Fatal(err) }
    doc["heartbeat"].(map[string]any)["memory"] = map[string]any{
        "engine_rss_bytes": 14000000, "memory_cap_events": 2,
        "future_unknown": "PRIVATE_MEMORY_SENTINEL",
    }
    b, _ = json.Marshal(doc)
    if err := os.WriteFile(p, b, 0644); err != nil { t.Fatal(err) }
    body := render(c, "/").Body.String()
    if !strings.Contains(body, "Memory-cap events") || !strings.Contains(body, "memory limit") {
        t.Fatalf("memory event was silently discarded: %s", body)
    }
    if strings.Contains(body, "PRIVATE_MEMORY_SENTINEL") { t.Fatal("unknown memory field leaked") }
}
