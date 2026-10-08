// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestStartupNeverClaimsEngineHealthy(t *testing.T) {
	for _, state := range []string{"gps-missing", "gps-waiting", "gps-busy", "gps-fix"} {
		t.Run(state, func(t *testing.T) {
			c, path := testConsole(t)
			fixture(t, path, time.Second, 30, false) // old engine could still look fresh
			p := filepath.Join(filepath.Dir(path), "startup.json")
			b, _ := json.Marshal(map[string]any{"schema_version": 1, "state": state,
				"written_at": proofNow, "status_interval_secs": 5, "private": attack})
			if err := os.WriteFile(p, b, 0644); err != nil {
				t.Fatal(err)
			}
			body := render(c, "/").Body.String()
			assertState(t, body, "offline")
			if strings.Contains(body, attack) {
				t.Fatal("raw startup field exposed")
			}
			if state == "gps-missing" && !strings.Contains(body, "GPS not detected") {
				t.Fatal(body)
			}
			if state == "gps-waiting" && !strings.Contains(body, "Waiting for GPS fix") {
				t.Fatal(body)
			}
			if err := os.Remove(p); err != nil {
				t.Fatal(err)
			}
			assertState(t, render(c, "/").Body.String(), "healthy")
		})
	}
}

func TestMissingGPSNamesOnlyValidatedAdapterIDs(t *testing.T) {
	c, path := testConsole(t)
	b, _ := json.Marshal(map[string]any{"schema_version": 1, "state": "gps-missing",
		"written_at": proofNow, "status_interval_secs": 5,
		"usb_adapter_ids": []string{"10c4:ea60", "0403:6001", attack}})
	if err := os.WriteFile(filepath.Join(filepath.Dir(path), "startup.json"), b, 0644); err != nil {
		t.Fatal(err)
	}
	body := render(c, "/").Body.String()
	assertState(t, body, "offline")
	if !strings.Contains(body, "10c4:ea60") || !strings.Contains(body, "0403:6001") || strings.Contains(body, attack) {
		t.Fatal(body)
	}
}

func TestStartupFreshnessAndInvalidRecordsFailClosed(t *testing.T) {
	for _, age := range []time.Duration{15 * time.Second, 15*time.Second + time.Nanosecond, -time.Second} {
		c, path := testConsole(t)
		fixture(t, path, time.Second, 30, false)
		p := filepath.Join(filepath.Dir(path), "startup.json")
		b, _ := json.Marshal(map[string]any{"schema_version": 1, "state": "gps-waiting", "written_at": proofNow.Add(-age), "status_interval_secs": 5})
		if err := os.WriteFile(p, b, 0644); err != nil {
			t.Fatal(err)
		}
		body := render(c, "/").Body.String()
		assertState(t, body, "offline")
		if strings.Contains(body, "Waiting for GPS fix") != (age == 15*time.Second) {
			t.Fatal(body)
		}
	}
	for _, invalid := range []string{`{}`, `not-json`, strings.Repeat("x", 4097), `{"schema_version":1,"state":"<script>evil</script>","written_at":"2026-09-21T12:00:00Z","status_interval_secs":5}`} {
		c, path := testConsole(t)
		fixture(t, path, time.Second, 30, false)
		if err := os.WriteFile(filepath.Join(filepath.Dir(path), "startup.json"), []byte(invalid), 0644); err != nil {
			t.Fatal(err)
		}
		assertState(t, render(c, "/").Body.String(), "offline")
	}
}

func TestStartupGPSDiagnostics(t *testing.T) {
	c, path := testConsole(t)
	b, _ := json.Marshal(map[string]any{"schema_version": 1, "state": "gps-waiting", "written_at": proofNow, "status_interval_secs": 5,
		"gps": map[string]any{"satellites_used": 0, "satellites_in_view": 12, "fix_quality": 0, "fix_mode": 1, "snr_max_dbhz": 37, "snr_avg_dbhz": 23.5, "nmea_age_secs": 2, "hdop": 4.2}})
	p := filepath.Join(filepath.Dir(path), "startup.json")
	if err := os.WriteFile(p, b, 0644); err != nil {
		t.Fatal(err)
	}
	body := render(c, "/").Body.String()
	assertState(t, body, "offline")
	if !strings.Contains(body, "HDOP</dt><dd>4.2") {
		t.Fatal("waiter HDOP missing")
	}
	for _, want := range []string{"GPS receiver observations", "Satellites used</dt><dd>0", "Satellites in view</dt><dd>12", "Fix quality</dt><dd>0", "Fix mode</dt><dd>1", "Maximum SNR</dt><dd>37", "Average SNR</dt><dd>23.5", "Last valid NMEA age</dt><dd>2"} {
		if !strings.Contains(body, want) {
			t.Errorf("missing %q", want)
		}
	}
	if evidence := os.Getenv("P0_EVIDENCE"); evidence != "" {
		if err := os.WriteFile(filepath.Join(evidence, "startup-rendered.html"), []byte(body), 0644); err != nil {
			t.Fatal(err)
		}
	}
	// The startup file expires regardless of the diagnostic contents.
	c.now = func() time.Time { return proofNow.Add(16 * time.Second) }
	if strings.Contains(render(c, "/").Body.String(), "Satellites in view</dt><dd>12") {
		t.Fatal("stale diagnostics shown")
	}
}

func TestStartupGPSMissingAndHostileDiagnostics(t *testing.T) {
	for _, gps := range []any{nil, map[string]any{}, map[string]any{"fix_quality": 99, "snr_avg_dbhz": -1}} {
		c, path := testConsole(t)
		b, _ := json.Marshal(map[string]any{"schema_version": 1, "state": "gps-waiting", "written_at": proofNow, "status_interval_secs": 5, "gps": gps})
		if err := os.WriteFile(filepath.Join(filepath.Dir(path), "startup.json"), b, 0644); err != nil {
			t.Fatal(err)
		}
		body := render(c, "/").Body.String()
		assertState(t, body, "offline")
		if !strings.Contains(body, "Fix quality</dt><dd>Unknown") {
			t.Fatal("missing/out-of-range quality not unknown")
		}
	}
	c, path := testConsole(t)
	b, _ := json.Marshal(map[string]any{"schema_version": 1, "state": "gps-waiting", "written_at": proofNow, "status_interval_secs": 5, "gps": map[string]any{"fix_mode": attack}})
	if err := os.WriteFile(filepath.Join(filepath.Dir(path), "startup.json"), b, 0644); err != nil {
		t.Fatal(err)
	}
	body := render(c, "/").Body.String()
	assertState(t, body, "offline")
	if strings.Contains(body, attack) {
		t.Fatal("hostile diagnostic rendered")
	}
}
