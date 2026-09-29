// SPDX-License-Identifier: AGPL-3.0-or-later
package main

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestIndependentAndPairedReleases(t *testing.T) {
	for _, mode := range []string{"engine", "console", "pair", "pair-console-fails"} {
		t.Run(mode, func(t *testing.T) {
			u, h, key := setup(t)
			m := manifest(u)
			if mode == "console" {
				m.Digest = oldDigest
				m.Version = oldVersion
				m.BuildSeq = 1
			}
			if mode != "engine" {
				m.ConsoleDigest = newDigest
				m.ConsoleVersion = newVersion
				m.ConsoleBuild = 2
			}
			h.consoleBad = mode == "pair-console-fails"
			server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write(sign(m, key)) }))
			defer server.Close()
			u.client = server.Client()
			u.cfg.BaseURL = server.URL
			if e := u.execute("poll"); e != nil {
				t.Fatal(e)
			}
			if h.consoleBad {
				if h.active.Digest != oldDigest || h.consoleActive.Digest != oldDigest || outcome(t, u)["kind"] != "rolled_back" || u.state.RunningSequence != 0 {
					t.Fatal("paired failure did not restore both outgoing components")
				}
			} else {
				if h.active.Digest != m.Digest || h.consoleActive.Digest != m.ConsoleDigest || u.state.RunningSequence != m.Sequence {
					t.Fatal("package did not converge")
				}
				if mode == "engine" && h.consoleRestarts != 0 {
					t.Fatal("unchanged console restarted")
				}
				if mode == "console" && h.restarts != 0 {
					t.Fatal("unchanged engine restarted")
				}
				raw, e := os.ReadFile(filepath.Join(u.cfg.StateDir, "release_currency.json"))
				if e != nil {
					t.Fatal(e)
				}
				var currency map[string]any
				if json.Unmarshal(raw, &currency) != nil || currency["release_seq_seen"] != float64(10) || currency["running_release_seq"] != float64(10) || currency["last_release_check_at"] == nil || currency["update_outcome"] != "applied" {
					t.Fatal("currency missing or untruthful", string(raw))
				}
			}
			evidence(t, "package-"+mode+".json", map[string]any{"test_only": true, "calls": h.events, "outcome": outcome(t, u), "state": u.state})
		})
	}
}
func TestConsoleQuarantineSurvivesNewReleaseMetadata(t *testing.T) {
	u, h, key := setup(t)
	h.consoleBad = true
	m := manifest(u)
	m.Digest = oldDigest
	m.Version = oldVersion
	m.BuildSeq = 1
	m.ConsoleDigest = newDigest
	m.ConsoleBuild = 2
	for i := 0; i < 3; i++ {
		m.Sequence++
		m.Salt = fmt.Sprintf("salt-%d", i)
		queue(t, u, sign(m, key))
		if e := u.execute("apply"); e != nil {
			t.Fatal(e)
		}
	}
	if h.pulls != 2 || !u.state.Quarantined[packageTarget(m)] || outcome(t, u)["kind"] != "quarantined" || h.restarts != 2 || h.consoleActive.Digest != oldDigest {
		t.Fatal("bad console not bounded/recovered", h.pulls)
	}
}
func TestSignedConsoleTamperTagAndFloors(t *testing.T) {
	u, h, key := setup(t)
	m := manifest(u)
	raw := sign(m, key)
	tampered := []byte(strings.Replace(string(raw), `"console_build_seq":1`, `"console_build_seq":9`, 1))
	if _, e := verifyRelease(tampered, u.keys, u.now()); e == nil {
		t.Fatal("console field not signed")
	}
	m.ConsoleDigest = "latest"
	if _, e := verifyRelease(sign(m, key), u.keys, u.now()); e == nil {
		t.Fatal("console tag accepted")
	}
	m = manifest(u)
	m.ConsoleDigest = newDigest
	m.ConsoleBuild = 1
	queue(t, u, sign(m, key))
	if e := u.execute("apply"); e == nil || h.pulls != 0 {
		t.Fatal("console equal-sequence changed digest accepted")
	}
}
func TestConsoleHealthIsRenderedCurrentEngine(t *testing.T) {
	for _, mode := range []string{"missing-timestamp", "stale-page", "wrong-digest", "offline", "500", "redirect", "fresh"} {
		t.Run(mode, func(t *testing.T) {
			u, _, _ := setup(t)
			since := u.now().Add(-time.Second)
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				s, _ := u.status()
				written := s.Written
				digest := s.Heartbeat.Digest
				state := "healthy"
				switch mode {
				case "stale-page":
					written = written.Add(-time.Hour)
				case "wrong-digest":
					digest = newDigest
				case "offline":
					state = "offline"
				case "500":
					w.WriteHeader(500)
				case "redirect":
					w.Header().Set("Location", "/status")
					w.WriteHeader(302)
				}
				if mode == "missing-timestamp" {
					fmt.Fprint(w, `<section data-engine-state="healthy">`)
				} else {
					fmt.Fprintf(w, `<section data-engine-state="%s" data-engine-written-at="%s">`, state, written.Format(time.RFC3339Nano))
				}
				fmt.Fprintf(w, "<dd>%s</dd><dd>%s</dd></section>", digest, s.Heartbeat.Version)
			}))
			defer server.Close()
			u.cfg.ConsoleURL = server.URL
			if got := u.consoleHealthy(oldDigest, oldVersion, since); got != (mode == "fresh") {
				t.Fatalf("%s healthy=%v", mode, got)
			}
		})
	}
}
func TestConfiguredFleetRings(t *testing.T) {
	for node, ring := range map[string]string{"brrdfeeder-saker-brrdg1s1": "dev", "brrdfeeder-saker-brrdg2s1": "staging", "brrdfeeder-open-brrdg3s2-001": "general"} {
		u, h, key := setup(t)
		u.cfg.Node = node
		u.cfg.Ring = ring
		h.write()
		m := manifest(u)
		if !m.eligible(node, ring) {
			t.Fatal("configured fleet ring excluded", node)
		}
		for _, other := range []string{"dev", "staging", "general"} {
			if other != ring && m.eligible(node, other) {
				t.Fatal("cross-ring accepted")
			}
		}
		m.Ring = "general"
		m.Audience = audienceFor("general")
		if ring != "general" {
			if _, e := u.releaseRequest(sign(m, key)); e == nil {
				t.Fatal("signed wrong ring accepted")
			}
		}
	}
}
func TestNoPathExistsAndFailedFetchFloor(t *testing.T) {
	for _, name := range []string{"brrdfeeder-updater.path", "brrdfeeder-release-poll.timer"} {
		b, _ := units.ReadFile("units/" + name)
		if strings.Contains(string(b), "\nPathExists=") {
			t.Fatal("retained mailbox can hot-loop")
		}
	}
	u, _, _ := setup(t)
	hits := 0
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { hits++; http.Error(w, "offline", 503) }))
	defer server.Close()
	u.client = server.Client()
	u.cfg.BaseURL = server.URL
	if e := u.execute("poll"); e == nil {
		t.Fatal("HTTP failure ignored")
	}
	if e := u.execute("poll"); e != nil {
		t.Fatal(e)
	}
	if hits != 1 || !u.state.LastReleaseCheck.IsZero() {
		t.Fatal("failed fetch bypassed floor or reported success")
	}
	queue(t, u, []byte("invalid"))
	_ = u.execute("poll")
	if hits != 1 {
		t.Fatal("invalid retained mailbox fetched again")
	}
}
