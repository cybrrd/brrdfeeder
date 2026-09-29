// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"flag"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestGPSRatingPolicy(t *testing.T) {
	p := defaultGPSPolicy()
	for _, tc := range []struct {
		fix  bool
		sats int
		hdop float64
		want string
	}{
		{true, 6, 2, "Good"}, {true, 5, 1.78, "Marginal"}, {true, 6, 2.01, "Marginal"},
		{true, 4, 5, "Marginal"}, {true, 3, 1, "Poor"}, {true, 8, 5.01, "Poor"},
		{false, 12, 1, "No fix"}, {true, 8, 0, "Poor"}, {true, 8, -1, "Poor"},
		{true, 8, math.NaN(), "Poor"}, {true, 8, math.Inf(1), "Poor"}, {true, 100, 1, "Poor"},
	} {
		if got := p.rate(tc.fix, tc.sats, &tc.hdop); got != tc.want {
			t.Errorf("%+v: %s", tc, got)
		}
	}
	if p.rate(true, 8, nil) != "Poor" {
		t.Fatal("missing HDOP rated optimistically")
	}
	f := flag.NewFlagSet("test", flag.ContinueOnError)
	p.flags(f)
	if err := f.Parse([]string{"--gps-good-min-sats=8", "--gps-good-max-hdop=1"}); err != nil {
		t.Fatal(err)
	}
	h := 1.5
	if p.validate() != nil || p.rate(true, 6, &h) != "Marginal" {
		t.Fatal("tuning ignored")
	}
	for _, bad := range []gpsPolicy{{3, 4, 2, 5}, {6, 0, 2, 5}, {6, 4, 6, 5}, {6, 4, 0, 5}, {6, 4, 2, math.NaN()}, {100, 4, 2, 5}} {
		if bad.validate() == nil {
			t.Fatalf("accepted %+v", bad)
		}
	}
}

func TestGPSRatingEngineRendering(t *testing.T) {
	for _, tc := range []struct {
		name        string
		sats        int
		hdop        any
		quality     int
		state, want string
	}{
		{"good", 6, 2, 1, "healthy", "Good"}, {"yard", 5, 1.78, 1, "healthy", "Marginal"},
		{"poor", 3, 6, 1, "healthy", "Poor"}, {"window", 1, 9, 0, "waiting", "No fix"},
		{"absent-hdop", 8, nil, 1, "healthy", "Poor"}, {"bad-hdop", 8, -1, 1, "healthy", "Poor"},
		{"bad-quality", 8, 1, 99, "healthy", "No fix"}, {"not-healthy", 8, 1, 1, "stale", "No fix"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			c, path := testConsole(t)
			fixture(t, path, time.Second, 30, false)
			b, _ := os.ReadFile(path)
			var doc map[string]any
			_ = json.Unmarshal(b, &doc)
			g := doc["inventory"].(map[string]any)["gps"].(map[string]any)
			g["sat_count"], g["hdop"], g["fix_quality"], g["state"] = tc.sats, tc.hdop, tc.quality, tc.state
			b, _ = json.Marshal(doc)
			if err := os.WriteFile(path, b, 0644); err != nil {
				t.Fatal(err)
			}
			body := render(c, "/").Body.String()
			assertState(t, body, "healthy")
			if !strings.Contains(body, `data-gps-rating="`+tc.want+`"`) {
				t.Fatal(body)
			}
			if strings.Contains(body, gpsPlacement) != (tc.want != "Good") {
				t.Fatal("incorrect placement guidance")
			}
			if !strings.Contains(body, "not reported by this engine version") || !strings.Contains(body, "Fix quality</dt>") || !strings.Contains(body, "HDOP</dt>") {
				t.Fatal(body)
			}
			if tc.name == "yard" && !strings.Contains(body, "HDOP</dt><dd>1.78") {
				t.Fatal("HDOP lost")
			}
			if dir := os.Getenv("P0_EVIDENCE"); dir != "" {
				if err := os.WriteFile(filepath.Join(dir, "gps-"+tc.name+".html"), []byte(body), 0644); err != nil {
					t.Fatal(err)
				}
			}
			c.now = func() time.Time { return proofNow.Add(91 * time.Second) }
			body = render(c, "/").Body.String()
			assertState(t, body, "offline")
			if strings.Contains(body, "data-gps-rating=") {
				t.Fatal("stale GPS graded")
			}
		})
	}
}

func TestGPSRatingWaiterRendering(t *testing.T) {
	for _, quality := range []int{0, 1} {
		c, path := testConsole(t)
		b, _ := json.Marshal(map[string]any{"schema_version": 1, "state": "gps-waiting", "written_at": proofNow, "status_interval_secs": 5,
			"gps": map[string]any{"satellites_used": 5, "satellites_in_view": 9, "fix_quality": quality, "snr_max_dbhz": 30, "snr_avg_dbhz": 20}})
		if err := os.WriteFile(filepath.Join(filepath.Dir(path), "startup.json"), b, 0644); err != nil {
			t.Fatal(err)
		}
		body := render(c, "/").Body.String()
		assertState(t, body, "offline")
		want := "No fix"
		if quality > 0 {
			want = "Poor"
		}
		for _, text := range []string{`data-gps-rating="` + want + `"`, gpsPlacement, "Satellites in view</dt><dd>9", "Maximum SNR</dt><dd>30", "Not reported by this waiter version"} {
			if !strings.Contains(body, text) {
				t.Fatalf("missing %q", text)
			}
		}
	}
}
