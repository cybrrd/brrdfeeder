// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestExitCodeOnlyAcceptsPureClockDefer(t *testing.T) {
	for _, tc := range []struct {
		name string
		err  error
		want int
	}{
		{"success", nil, 0},
		{"clock", errClockUntrusted, 75},
		{"wrapped", fmt.Errorf("poll: %w", errClockUntrusted), 75},
		{"joined-nil", errors.Join(errClockUntrusted, nil), 75},
		{"joined-clock", errors.Join(errClockUntrusted, fmt.Errorf("poll: %w", errClockUntrusted)), 75},
		{"disk-failure", errors.Join(errClockUntrusted, syscall.ENOSPC), 1},
		{"wrapped-disk-failure", fmt.Errorf("attempt: %w", errors.Join(errClockUntrusted, syscall.EIO)), 1},
		{"lookalike-text", errors.New(errClockUntrusted.Error()), 1},
		{"real-error", errors.New("helper HTTP 404"), 1},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := exitCode(tc.err); got != tc.want {
				t.Fatalf("exit=%d want=%d: %v", got, tc.want, tc.err)
			}
		})
	}
	for _, text := range []string{"engine has not reported a fresh trusted clock", "os_clock_trusted=true", "re-run the installer to update"} {
		if !strings.Contains(errClockUntrusted.Error(), text) {
			t.Fatal("missing defer guidance", text)
		}
	}
}

func TestClockDeferKeepsGateClosedOnRepeatedPolls(t *testing.T) {
	for _, kind := range []string{"false", "absent", "stale", "future", "missing", "malformed"} {
		for _, mode := range []string{"poll", "poll-updater"} {
			for _, pending := range []bool{false, true} {
				t.Run(fmt.Sprintf("%s/%s/pending=%t", kind, mode, pending), func(t *testing.T) {
					u, h, key := setup(t)
					var staged []byte
					if pending {
						staged = sign(manifest(u), key)
						queue(t, u, staged)
					}
					oldEngine, _ := os.ReadFile(u.cfg.Quadlet)
					oldConsole, _ := os.ReadFile(u.cfg.ConsoleQuadlet)
					before, _ := json.Marshal(u.state)
					status, err := u.status()
					if err != nil {
						t.Fatal(err)
					}
					switch kind {
					case "false":
						trust := false
						status.Heartbeat.Trusted = &trust
					case "absent":
						status.Heartbeat.Trusted = nil
					case "stale":
						status.Written = status.Written.Add(-time.Hour)
					case "future":
						status.Written = status.Written.Add(time.Hour)
					}
					if err = atomicJSON(u.cfg.StatusFile, status, 0644); err != nil {
						t.Fatal(err)
					}
					if kind == "missing" {
						if err = os.Remove(u.cfg.StatusFile); err != nil {
							t.Fatal(err)
						}
					}
					if kind == "malformed" {
						if err = os.WriteFile(u.cfg.StatusFile, []byte("{invalid"), 0644); err != nil {
							t.Fatal(err)
						}
					}
					hits := 0
					u.client = &http.Client{Transport: receiptTransport(func(*http.Request) (*http.Response, error) {
						hits++
						return nil, errors.New("network must not be called")
					})}
					for tick := 0; tick < 2; tick++ {
						err = u.execute(mode)
						if exitCode(err) != 75 {
							t.Fatalf("tick %d exit=%d: %v", tick, exitCode(err), err)
						}
					}
					after, _ := json.Marshal(u.state)
					engine, _ := os.ReadFile(u.cfg.Quadlet)
					console, _ := os.ReadFile(u.cfg.ConsoleQuadlet)
					if hits != 0 || len(h.events) != 0 || !bytes.Equal(before, after) || !bytes.Equal(oldEngine, engine) || !bytes.Equal(oldConsole, console) {
						t.Fatal("clock defer performed network/workload/floor changes", hits, h.events)
					}
					if pending {
						raw, err := os.ReadFile(u.pending())
						if err != nil || !bytes.Equal(raw, staged) {
							t.Fatal("defer discarded pending release", err)
						}
					}
					if _, err = os.Stat(filepath.Join(u.cfg.PrivateDir, "host-state.json")); !os.IsNotExist(err) {
						t.Fatal("clock defer advanced host floor", err)
					}
				})
			}
		}
	}
}

func TestClockDeferReceiptWriteFailureStillFails(t *testing.T) {
	u, _, _ := setup(t)
	if err := os.Remove(u.cfg.StatusFile); err != nil {
		t.Fatal(err)
	}
	u.write = func(string, []byte, os.FileMode) error { return syscall.ENOSPC }
	err := u.execute("poll")
	if !errors.Is(err, errClockUntrusted) || !errors.Is(err, syscall.ENOSPC) || exitCode(err) != 1 {
		t.Fatal("real receipt failure hidden by clock defer", err)
	}
}

func TestOnlyTimerPollUnitsAcceptClockDefer(t *testing.T) {
	entries, err := units.ReadDir("units")
	if err != nil {
		t.Fatal(err)
	}
	for _, entry := range entries {
		if !strings.HasSuffix(entry.Name(), ".service") {
			continue
		}
		raw, err := units.ReadFile("units/" + entry.Name())
		if err != nil {
			t.Fatal(err)
		}
		var accepted []string
		for _, line := range strings.Split(string(raw), "\n") {
			if strings.HasPrefix(line, "SuccessExitStatus=") {
				accepted = append(accepted, strings.Fields(strings.TrimPrefix(line, "SuccessExitStatus="))...)
			}
		}
		poll := entry.Name() == "brrdfeeder-release-poll.service" || entry.Name() == "brrdfeeder-host-update.service"
		if poll {
			if len(accepted) != 1 || accepted[0] != "75" {
				t.Fatal("poll must accept only dedicated defer status", entry.Name(), accepted)
			}
		} else if len(accepted) != 0 {
			t.Fatal("non-poll service masks errors", entry.Name(), accepted)
		}
	}
}
