// SPDX-License-Identifier: AGPL-3.0-or-later
package main

import (
	"encoding/json"
	"os"
	"strings"
	"testing"
	"time"
)

func TestBluetoothCurrentState(t *testing.T) {
	for _, tc := range []struct {
		name, state, observed, want                 string
		history, soft, hard, missing, invalid, warn bool
	}{
		{name: "blocked then unblocked Healthy", state: "healthy", history: true, want: "Not blocked"},
		{name: "blocked then unblocked quiet", state: "degraded", history: true, want: "Not blocked"},
		{name: "unblock disabled", state: "failed", history: true, soft: true, warn: true, want: "Blocked"},
		{name: "hard blocked", state: "failed", hard: true, warn: true, want: "Blocked"},
		{name: "reblocked before health changes", state: "healthy", soft: true, warn: true, want: "Blocked"},
		{name: "old observation only", state: "healthy", history: true, missing: true, want: "Unknown"},
		{name: "stale current field", state: "healthy", history: true, soft: true, observed: "2026-09-21T11:00:00Z", want: "Unknown"},
		{name: "future observation", state: "healthy", soft: true, observed: "2026-09-21T12:00:01Z", want: "Unknown"},
		{name: "invalid observation", state: "healthy", soft: true, observed: "not a time", want: "Unknown"},
		{name: "incomplete observation", state: "healthy", invalid: true, want: "Unknown"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			c, p := testConsole(t)
			fixture(t, p, time.Second, 30, false)
			b, err := os.ReadFile(p)
			if err != nil {
				t.Fatal(err)
			}
			var doc map[string]any
			if err = json.Unmarshal(b, &doc); err != nil {
				t.Fatal(err)
			}
			ble := doc["inventory"].(map[string]any)["rid_ble"].(map[string]any)
			ble["state"] = tc.state
			ble["rfkill"] = map[string]any{"soft_blocked": tc.history, "hard_blocked": false, "observed_at": "2026-09-21T10:00:00Z"}
			observed := tc.observed
			if observed == "" {
				observed = "2026-09-21T11:59:59Z"
			}
			current := map[string]any{"soft_blocked": tc.soft, "hard_blocked": tc.hard, "observed_at": observed}
			if tc.invalid {
				delete(current, "hard_blocked")
			}
			if !tc.missing {
				ble["current_rfkill"] = current
			} else {
				delete(ble, "current_rfkill")
			}
			b, err = json.Marshal(doc)
			if err != nil {
				t.Fatal(err)
			}
			if err = os.WriteFile(p, b, 0644); err != nil {
				t.Fatal(err)
			}
			v := readStatus(p, proofNow)
			var blocked bool
			for _, r := range v.Repairs {
				if strings.Contains(strings.ToLower(r.Problem), "bluetooth") && strings.Contains(r.Problem, "blocked") {
					blocked = true
				}
			}
			if blocked != tc.warn {
				t.Errorf("current blocked attention=%v, want %v; repairs=%+v", blocked, tc.warn, v.Repairs)
			}
			if !strings.HasPrefix(v.RFKill, tc.want) {
				t.Errorf("current block label=%q, want prefix %q", v.RFKill, tc.want)
			}
			if !tc.warn && len(v.Repairs) != 0 {
				t.Errorf("resolved/unknown history must not create attention: %+v", v.Repairs)
			}
			body := render(c, "/").Body.String()
			if tc.history && !strings.Contains(body, "Historical block observation") {
				t.Error("missing explicit history label")
			}
			evidence(t, strings.ReplaceAll(tc.name, " ", "-")+".html", body)
		})
	}
}

func TestBluetoothFailedWithoutCurrentBlock(t *testing.T) {
	_, p := testConsole(t)
	fixture(t, p, time.Second, 30, false)
	b, err := os.ReadFile(p)
	if err != nil {
		t.Fatal(err)
	}
	b = []byte(strings.ReplaceAll(string(b), `"state":"healthy"`, `"state":"failed"`))
	// Only BLE health should fail; keep GPS healthy to isolate attention.
	var doc map[string]any
	if err = json.Unmarshal(b, &doc); err != nil {
		t.Fatal(err)
	}
	doc["inventory"].(map[string]any)["gps"].(map[string]any)["state"] = "healthy"
	b, err = json.Marshal(doc)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(p, b, 0644); err != nil {
		t.Fatal(err)
	}
	v := readStatus(p, proofNow)
	if v.BLE != "Failed" || v.RFKill != "Not blocked" || len(v.Repairs) != 1 || v.Repairs[0].Problem != "Bluetooth reception has failed." {
		t.Fatalf("failed BLE misreported: %+v", v)
	}
}
