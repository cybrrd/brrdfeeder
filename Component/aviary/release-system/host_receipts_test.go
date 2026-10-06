// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

type receiptTransport func(*http.Request) (*http.Response, error)
func (f receiptTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestUnknownStoreRefusesBeforePull(t *testing.T) {
	u, h, key := setup(t)
	queue(t, u, sign(manifest(u), key))
	u.run = gateRunner{Runner: h, after: func(bin string, args []string, b []byte, e error) ([]byte, error) {
		if strings.Contains(strings.Join(args, " "), "info --format") { return []byte("/does-not-exist/brrdfeeder-store"), nil }
		return b, e
	}}
	err := u.execute("apply")
	if err == nil || h.pulls != 0 || h.restarts != 0 || len(h.anchors) != 0 { t.Fatal("unmeasurable store did not defer before pull", err, h.events) }
}

func TestAttemptReceiptSeparatesHTTPFailureFromValidCheck(t *testing.T) {
	u, _, _ := setup(t)
	u.client = &http.Client{Transport: receiptTransport(func(*http.Request) (*http.Response, error) {
		return &http.Response{StatusCode: 503, Body: io.NopCloser(strings.NewReader("secret-token"))}, nil
	})}
	if err := u.execute("poll"); err == nil { t.Fatal("503 accepted") }
	for _, path := range []string{filepath.Join(u.cfg.PrivateDir, "attempt.json"), filepath.Join(u.cfg.StateDir, "release_attempt.json")} {
		b, err := os.ReadFile(path)
		if err != nil { t.Fatal("missing failed-attempt receipt", err) }
		var v map[string]any
		if json.Unmarshal(b, &v) != nil || v["http_status"] != float64(503) || v["error_code"] != "http" || v["network_attempted"] != true || v["helper_build"] == nil || len(b) > 4096 || strings.Contains(string(b), "secret-token") { t.Fatal("unsafe/incomplete receipt", string(b)) }
	}
	if !u.state.LastReleaseCheck.IsZero() || u.state.Seen.Sequence != 0 { t.Fatal("failed fetch advanced valid check") }
}

func TestHTTPCompletionRechecksClockBeforeValidCheck(t *testing.T) {
	u, h, key := setup(t)
	raw := sign(manifest(u), key)
	u.client = &http.Client{Transport: receiptTransport(func(*http.Request) (*http.Response, error) {
		s, err := u.status(); if err != nil { t.Fatal(err) }
		trusted := false; s.Heartbeat.Trusted = &trusted
		if err = atomicJSON(u.cfg.StatusFile, s, 0644); err != nil { t.Fatal(err) }
		return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader(string(raw)))}, nil
	})}
	if err := u.execute("poll"); err == nil { t.Fatal("lost clock trust accepted") }
	if !u.state.LastReleaseCheck.IsZero() || u.state.Seen.Sequence != 0 || h.pulls != 0 { t.Fatal("untrusted completion recorded valid check") }
}

func TestCommandOutputBoundCancelsWhileStreaming(t *testing.T) {
	if os.Getenv("BRRD_TEST_OUTPUT_CHILD") == "1" {
		for i := 0; i < 300; i++ { fmt.Print(strings.Repeat("x", 8192)); time.Sleep(10*time.Millisecond) }
		os.Exit(0)
	}
	t.Setenv("BRRD_TEST_OUTPUT_CHILD", "1")
	started := time.Now()
	b, err := (commands{}).Run(os.Args[0], "-test.run=^TestCommandOutputBoundCancelsWhileStreaming$")
	if err == nil || len(b) != 0 || time.Since(started) > 2500*time.Millisecond { t.Fatal("output was not bounded during collection", err, len(b), time.Since(started)) }
}
