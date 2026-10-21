// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"net"
	"net/http"
	"os"
	"os/exec"
	"testing"
	"time"
)

// Overlay into release-system for local proof; never part of the host binary.
// Uses the real console process and real consoleHealthy, with synthetic engine
// status and fake host restart observations. This is not native rollback proof.
func TestDrillConsoleProcess(t *testing.T) {
	binary := os.Getenv("DRILL_CONSOLE_BINARY")
	if binary == "" {
		t.Fatal("DRILL_CONSOLE_BINARY required")
	}
	want := os.Getenv("DRILL_EXPECT_HEALTHY") == "true"
	u, _, _ := setup(t)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	addr := listener.Addr().String()
	listener.Close()
	cmd := exec.Command(binary, "-listen", addr, "-allowed-hosts", addr, "-status-file", u.cfg.StatusFile)
	cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
	// Proving the unhealthy release behavior requires the release-injected
	// ENV (see main.go / build.sh); proving healthy requires its absence —
	// exactly like fixture and release builds respectively.
	if !want {
		cmd.Env = append(os.Environ(), "BRRDHOUSE_DRILL_UNHEALTHY=1")
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cmd.Process.Signal(os.Interrupt); cmd.Wait() })
	u.cfg.ConsoleURL = "http://" + addr
	client := &http.Client{Timeout: time.Second}
	ready := false
	for i := 0; i < 100; i++ {
		res, err := client.Get(u.cfg.ConsoleURL + "/")
		if err == nil {
			res.Body.Close()
			ready = res.StatusCode == 200
			if ready {
				break
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	if !ready {
		t.Fatal("console root did not become ready")
	}
	res, err := client.Get(u.cfg.ConsoleURL + "/status")
	if err != nil {
		t.Fatal(err)
	}
	res.Body.Close()
	wantCode := 503
	if want {
		wantCode = 200
	}
	if res.StatusCode != wantCode {
		t.Fatalf("production /status = %d, want %d", res.StatusCode, wantCode)
	}
	start := u.now()
	// Existing fixture advances simulated time, refreshing status on every tick.
	u.healthTimeout = 180 * time.Second
	got := u.consoleHealthy(oldDigest, oldVersion, start.Add(-time.Second))
	if got != want {
		t.Fatalf("actual updater consoleHealthy = %v, want %v", got, want)
	}
	t.Logf("HTTP %d; consoleHealthy=%v; simulated health elapsed=%s", res.StatusCode, got, u.now().Sub(start))
}
