// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"os"
	"strings"
	"testing"
	"time"
)

type gateRunner struct {
	Runner
	after func(string, []string, []byte, error) ([]byte, error)
}

func (r gateRunner) Run(bin string, args ...string) ([]byte, error) {
	b, e := r.Runner.Run(bin, args...)
	if r.after != nil {
		return r.after(bin, args, b, e)
	}
	return b, e
}

func TestDownloadsRecheckClockBeforeAnySwitch(t *testing.T) {
	for _, mode := range []string{"expired", "clock-false", "clock-stale", "clock-backward"} {
		t.Run(mode, func(t *testing.T) {
			u, h, key := setup(t)
			m := manifest(u)
			if mode == "expired" {
				m.Expires = u.now().Add(time.Second).Format(time.RFC3339)
			}
			queue(t, u, sign(m, key))
			changed := false
			u.run = gateRunner{Runner: h, after: func(bin string, args []string, b []byte, e error) ([]byte, error) {
				if !changed && bin == "podman" && args[0] == "pull" {
					changed = true
					switch mode {
					case "expired":
						u.sleep(2 * time.Second)
					case "clock-backward":
						prior := u.now
						u.now = func() time.Time { return prior().Add(-time.Hour) }
						h.write()
					default:
						s, err := u.status()
						if err != nil {
							t.Fatal(err)
						}
						if mode == "clock-false" {
							trusted := false
							s.Heartbeat.Trusted = &trusted
						} else {
							s.Written = s.Written.Add(-time.Hour)
						}
						if err = atomicJSON(u.cfg.StatusFile, s, 0644); err != nil {
							t.Fatal(err)
						}
					}
				}
				return b, e
			}}
			oldEngine, _ := os.ReadFile(u.cfg.Quadlet)
			oldConsole, _ := os.ReadFile(u.cfg.ConsoleQuadlet)
			err := u.execute("apply")
			engine, _ := os.ReadFile(u.cfg.Quadlet)
			console, _ := os.ReadFile(u.cfg.ConsoleQuadlet)
			if err == nil || !changed || string(engine) != string(oldEngine) || string(console) != string(oldConsole) || h.restarts != 0 || h.consoleRestarts != 0 || len(h.anchors) != 0 || u.state.Active != nil {
				t.Fatal("download invalidated trust but transaction switched", err, h.events)
			}
			if u.state.Attempts[packageTarget(m)] != 0 || u.state.Quarantined[packageTarget(m)] {
				t.Fatal("clock outage counted as bad image")
			}
		})
	}
}

func TestConsoleTupleBoundBeforeSwitch(t *testing.T) {
	for _, mode := range []string{"unchanged-build", "unchanged-revision", "changed-revision"} {
		t.Run(mode, func(t *testing.T) {
			u, h, key := setup(t)
			m := manifest(u)
			switch mode {
			case "unchanged-build":
				m.ConsoleBuild = 99
			case "unchanged-revision":
				m.ConsoleVersion = strings.Repeat("c", 40)
			case "changed-revision":
				m.ConsoleDigest = newDigest
				m.ConsoleBuild = 2
				m.ConsoleVersion = oldVersion
			}
			queue(t, u, sign(m, key))
			err := u.execute("apply")
			if err == nil || h.restarts != 0 || h.consoleRestarts != 0 || len(h.anchors) != 0 || u.state.Active != nil || u.state.AppliedConsoleBuild != 0 {
				t.Fatal("wrong console tuple switched or raised floor", err, h.events)
			}
		})
	}
}
