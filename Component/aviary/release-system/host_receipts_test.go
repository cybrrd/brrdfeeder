// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

type receiptTransport func(*http.Request) (*http.Response, error)

func (f receiptTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestUnknownStoreRefusesBeforePull(t *testing.T) {
	u, h, key := setup(t)
	queue(t, u, sign(manifest(u), key))
	u.run = gateRunner{Runner: h, after: func(bin string, args []string, b []byte, e error) ([]byte, error) {
		if strings.Contains(strings.Join(args, " "), "info --format") {
			return []byte("/does-not-exist/brrdfeeder-store"), nil
		}
		return b, e
	}}
	err := u.execute("apply")
	if err == nil || h.pulls != 0 || h.restarts != 0 || len(h.anchors) != 0 {
		t.Fatal("unmeasurable store did not defer before pull", err, h.events)
	}
}

func TestAttemptReceiptSeparatesHTTPFailureFromValidCheck(t *testing.T) {
	u, _, _ := setup(t)
	u.client = &http.Client{Transport: receiptTransport(func(*http.Request) (*http.Response, error) {
		return &http.Response{StatusCode: 503, Body: io.NopCloser(strings.NewReader("secret-token"))}, nil
	})}
	if err := u.execute("poll"); err == nil {
		t.Fatal("503 accepted")
	}
	for _, path := range []string{filepath.Join(u.cfg.PrivateDir, "attempt.json"), filepath.Join(u.cfg.StateDir, "release_attempt.json")} {
		b, err := os.ReadFile(path)
		if err != nil {
			t.Fatal("missing failed-attempt receipt", err)
		}
		var v map[string]any
		if json.Unmarshal(b, &v) != nil || v["http_status"] != float64(503) || v["error_code"] != "http" || v["network_attempted"] != true || v["helper_build"] == nil || len(b) > 4096 || strings.Contains(string(b), "secret-token") {
			t.Fatal("unsafe/incomplete receipt", string(b))
		}
	}
	if !u.state.LastReleaseCheck.IsZero() || u.state.Seen.Sequence != 0 {
		t.Fatal("failed fetch advanced valid check")
	}
}

func TestHTTPCompletionRechecksClockBeforeValidCheck(t *testing.T) {
	u, h, key := setup(t)
	raw := sign(manifest(u), key)
	u.client = &http.Client{Transport: receiptTransport(func(*http.Request) (*http.Response, error) {
		s, err := u.status()
		if err != nil {
			t.Fatal(err)
		}
		trusted := false
		s.Heartbeat.Trusted = &trusted
		if err = atomicJSON(u.cfg.StatusFile, s, 0644); err != nil {
			t.Fatal(err)
		}
		return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader(string(raw)))}, nil
	})}
	if err := u.execute("poll"); err == nil {
		t.Fatal("lost clock trust accepted")
	}
	if !u.state.LastReleaseCheck.IsZero() || u.state.Seen.Sequence != 0 || h.pulls != 0 {
		t.Fatal("untrusted completion recorded valid check")
	}
}

func TestCommandOutputBoundCancelsWhileStreaming(t *testing.T) {
	if os.Getenv("BRRD_TEST_OUTPUT_CHILD") == "1" {
		for i := 0; i < 300; i++ {
			fmt.Print(strings.Repeat("x", 8192))
			time.Sleep(10 * time.Millisecond)
		}
		os.Exit(0)
	}
	t.Setenv("BRRD_TEST_OUTPUT_CHILD", "1")
	started := time.Now()
	b, err := (commands{}).Run(os.Args[0], "-test.run=^TestCommandOutputBoundCancelsWhileStreaming$")
	if err == nil || len(b) != 0 || time.Since(started) > 2500*time.Millisecond {
		t.Fatal("output was not bounded during collection", err, len(b), time.Since(started))
	}
}

func TestResourceReserveAtBothDownloadBoundaries(t *testing.T) {
	for _, afterPull := range []bool{false, true} {
		for _, full := range []string{"bytes", "inodes"} {
			t.Run(fmt.Sprint(afterPull, full), func(t *testing.T) {
				u, h, key := setup(t)
				m := manifest(u)
				queue(t, u, sign(m, key))
				u.space = func(string) (uint64, uint64, error) {
					if !afterPull || h.pulls > 0 {
						if full == "bytes" {
							return metadataReserve - 1, inodeReserve, nil
						}
						return metadataReserve, inodeReserve - 1, nil
					}
					return metadataReserve, inodeReserve, nil
				}
				if err := u.execute("apply"); err == nil {
					t.Fatal("reserve ignored")
				}
				if (!afterPull && h.pulls != 0) || h.restarts != 0 || h.consoleRestarts != 0 || len(h.anchors) != 0 || u.state.Active != nil || u.state.Attempts[packageTarget(m)] != 0 {
					t.Fatal("storage refusal disturbed old pair", h.events)
				}
			})
		}
	}
}

func TestWriteENOSPCKeepsFloorsAndDoesNotQuarantine(t *testing.T) {
	for _, location := range []string{"stage", "journal", "engine-pin", "console-pin", "rollback-pin"} {
		t.Run(location, func(t *testing.T) {
			u, h, key := setup(t)
			m := manifest(u)
			m.ConsoleDigest, m.ConsoleVersion, m.ConsoleBuild = newDigest, newVersion, 2
			raw := sign(m, key)
			mode := "apply"
			injected := false
			if location == "stage" {
				mode = "poll"
				u.client = &http.Client{Transport: receiptTransport(func(*http.Request) (*http.Response, error) {
					return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader(string(raw)))}, nil
				})}
				u.stageFile = func(string, []byte) error { injected = true; return syscall.ENOSPC }
			} else {
				queue(t, u, raw)
				u.write = func(path string, b []byte, perm os.FileMode) error {
					fail := location == "journal" && path == u.statePath()
					fail = fail || location == "engine-pin" && path == u.cfg.Quadlet
					fail = fail || (location == "console-pin" || location == "rollback-pin") && path == u.cfg.ConsoleQuadlet
					if fail && (!injected || location == "rollback-pin") {
						injected = true
						return syscall.ENOSPC
					}
					return atomicFile(path, b, perm)
				}
			}
			if err := u.execute(mode); !errors.Is(err, syscall.ENOSPC) || !injected {
				t.Fatal("fault not exercised/reported", err)
			}
			if u.state.AppliedBuild != 0 || u.state.AppliedConsoleBuild != 0 || u.state.Attempts[packageTarget(m)] != 0 || u.state.Quarantined[packageTarget(m)] {
				t.Fatal("storage error counted as bad image or committed")
			}
			if h.active.Digest != oldDigest || h.consoleActive.Digest != oldDigest {
				t.Fatal("old running pair not restored")
			}
			if location == "rollback-pin" && u.state.Active == nil {
				t.Fatal("lost retry journal while rollback writes unavailable")
			}
			u.write = nil
			if err := u.execute("recover"); err != nil {
				t.Fatal("later local recovery", err)
			}
			if u.state.Active != nil {
				t.Fatal("recovery journal not cleared")
			}
		})
	}
}

func TestUncachedInterruptedPullLeavesOldPair(t *testing.T) {
	u, h, key := setup(t)
	m := manifest(u)
	queue(t, u, sign(m, key))
	u.run = gateRunner{Runner: h, after: func(bin string, args []string, b []byte, err error) ([]byte, error) {
		if bin == "podman" && (args[0] == "pull" || args[0] == "image" && args[len(args)-1] == repository+"@"+newDigest) {
			return nil, syscall.ENOSPC
		}
		return b, err
	}}
	if err := u.execute("apply"); !errors.Is(err, syscall.ENOSPC) {
		t.Fatal(err)
	}
	if h.restarts != 0 || h.consoleRestarts != 0 || len(h.anchors) != 0 || u.state.Active != nil || u.state.Attempts[packageTarget(m)] != 0 {
		t.Fatal("failed pull changed old workloads", h.events)
	}
	if _, err := os.Stat(u.pending()); err != nil {
		t.Fatal("retryable stage lost", err)
	}
}

func TestFailedPollFloorSurvivesRestartAndBackwardClock(t *testing.T) {
	u, h, _ := setup(t)
	calls := 0
	u.client = &http.Client{Transport: receiptTransport(func(*http.Request) (*http.Response, error) { calls++; return nil, errors.New("unavailable") })}
	if err := u.execute("poll"); err == nil {
		t.Fatal("failure missing")
	}
	floor := u.state.NextPoll
	u.state = State{} // next execute reloads durable floor
	u.sleep(-time.Minute)
	h.write()
	if err := u.execute("poll"); err != nil {
		t.Fatal(err)
	}
	if calls != 1 || !u.state.NextPoll.Equal(floor) || !u.state.LastReleaseCheck.IsZero() {
		t.Fatal("restart/clock bypassed throttle")
	}
}

func TestBothActualStoresAndOwnedCommandBoundary(t *testing.T) {
	u, h, key := setup(t)
	m := manifest(u)
	m.ConsoleDigest, m.ConsoleVersion, m.ConsoleBuild = newDigest, newVersion, 2
	queue(t, u, sign(m, key))
	measured := map[string]int{}
	u.space = func(path string) (uint64, uint64, error) { measured[path]++; return metadataReserve, inodeReserve, nil }
	u.run = gateRunner{Runner: h, after: func(bin string, args []string, b []byte, err error) ([]byte, error) {
		if strings.Contains(strings.Join(args, " "), "info --format") {
			if bin == "runuser" {
				return []byte("/fixture/rootless-store"), nil
			}
			return []byte("/fixture/rootful-store"), nil
		}
		return b, err
	}}
	if err := u.execute("apply"); err != nil {
		t.Fatal(err)
	}
	for _, p := range []string{u.cfg.PrivateDir, u.cfg.StateDir, filepath.Dir(u.cfg.Quadlet), filepath.Dir(u.cfg.ConsoleQuadlet), "/fixture/rootful-store", "/fixture/rootless-store"} {
		if measured[p] < 2 {
			t.Fatal("filesystem not measured at both boundaries", p, measured)
		}
	}
	for _, event := range h.events {
		command := event
		if strings.HasPrefix(command, "runuser ") {
			prefix := "runuser -u brrdhouse -- env XDG_RUNTIME_DIR=/run/user/1001 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1001/bus "
			if !strings.HasPrefix(command, prefix) {
				t.Fatal("wrong console account", command)
			}
			command = strings.TrimPrefix(command, prefix)
			command = strings.Replace(command, "systemctl --user ", "systemctl ", 1)
		}
		allowed := false
		for _, prefix := range []string{"podman inspect --type container ", "podman image inspect ", "podman info --format {{.Store.GraphRoot}}", "podman pull ", "podman run -d --name brrdfeeder-rollback-", "podman run -d --name brrdhouse-rollback-", "systemctl show brrdfeeder-engine.service ", "systemctl show brrdhouse.service "} {
			allowed = allowed || strings.HasPrefix(command, prefix)
		}
		for _, exact := range []string{"systemctl daemon-reload", "systemctl reset-failed brrdfeeder-engine.service", "systemctl reset-failed brrdhouse.service", "systemctl restart brrdfeeder-engine.service", "systemctl restart brrdhouse.service"} {
			allowed = allowed || command == exact
		}
		if !allowed {
			t.Fatal("command escaped owned workloads", command)
		}
	}
	b, err := os.ReadFile(filepath.Join(u.cfg.StateDir, "release_attempt.json"))
	if err != nil {
		t.Fatal(err)
	}
	var a Attempt
	if json.Unmarshal(b, &a) != nil || a.ConsoleDigest != newDigest || a.ConsoleRevision != newVersion || a.ConsoleBuild != 2 || a.ErrorCode != "" || a.Outcome != "applied" {
		t.Fatal("verified console receipt incomplete", string(b))
	}
}
