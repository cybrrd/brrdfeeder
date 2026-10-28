// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"time"
)

func captureHostPoll(t *testing.T, poll func() error) (error, string) {
	t.Helper()
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	old := os.Stdout
	os.Stdout = w
	defer func() { os.Stdout = old; r.Close(); w.Close() }()
	err = poll()
	w.Close()
	os.Stdout = old
	out, readErr := io.ReadAll(r)
	if readErr != nil {
		t.Fatal(readErr)
	}
	return err, string(out)
}

func TestHostManifestAbsentIsNoRelease(t *testing.T) {
	u, h, _ := setup(t)
	u.cfg.Ring = "dev"
	compiled, _ := strconv.ParseUint(updaterBuild, 10, 64)
	prior := HostState{Build: compiled, Seen: Seen{7, strings.Repeat("b", 64)}, Attempted: map[string]bool{strings.Repeat("c", 64): true}}
	statePath := filepath.Join(u.cfg.PrivateDir, "host-state.json")
	if err := atomicJSON(statePath, prior, 0600); err != nil {
		t.Fatal(err)
	}
	wantPoll := u.now().Add(pollFloor + time.Duration(cohort(u.cfg.Node, "host-poll"))*3*time.Second)
	calls := 0
	u.client.Transport = receiptTransport(func(r *http.Request) (*http.Response, error) {
		calls++
		if r.URL.String() != "https://get.cybrrd.com/releases/v1/dev/updater.json" {
			t.Fatalf("unexpected request: %s", r.URL)
		}
		// The scheduling floor must already be durable before a response exists.
		var saved HostState
		raw, err := os.ReadFile(statePath)
		if err != nil || json.Unmarshal(raw, &saved) != nil || !saved.NextPoll.Equal(wantPoll) {
			t.Fatalf("next_poll not persisted before GET: %s, %v", raw, err)
		}
		return &http.Response{StatusCode: 404, Body: io.NopCloser(strings.NewReader("not a signed release"))}, nil
	})
	err, out := captureHostPoll(t, func() error { return u.execute("poll-updater") })
	if err != nil || out != "host-updater: no release published for ring dev; nothing to do\n" || calls != 1 {
		t.Fatalf("absent manifest was not one successful no-release outcome: err=%v output=%q calls=%d", err, out, calls)
	}
	raw, err := os.ReadFile(statePath)
	var saved HostState
	if err != nil || json.Unmarshal(raw, &saved) != nil || !saved.NextPoll.Equal(wantPoll) || saved.Build != prior.Build || saved.Seen != prior.Seen || !reflect.DeepEqual(saved.Attempted, prior.Attempted) {
		t.Fatalf("absence changed host trust/attempt state: %s, %v", raw, err)
	}
	err, out = captureHostPoll(t, func() error { return u.execute("poll-updater") })
	after, readErr := os.ReadFile(statePath)
	if err != nil || readErr != nil || out != "" || calls != 1 || string(after) != string(raw) || len(h.events) != 0 {
		t.Fatalf("floor no-op changed behavior: err=%v output=%q calls=%d", err, out, calls)
	}
}

func TestHostDownloadHTTPFailuresRemainErrors(t *testing.T) {
	for _, stage := range []string{"manifest", "executable"} {
		for _, status := range []int{404, 403, 429, 500, 503, 204, 206} {
			if stage == "manifest" && status == 404 {
				continue
			}
			t.Run(fmt.Sprintf("%s-%d", stage, status), func(t *testing.T) {
				u, h, key := setup(t)
				m := hostManifest(u)
				calls := 0
				u.client.Transport = receiptTransport(func(r *http.Request) (*http.Response, error) {
					calls++
					if stage == "executable" && r.URL.Path == "/releases/v1/general/updater.json" {
						return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader(string(hostSign(m, key))))}, nil
					}
					return &http.Response{StatusCode: status, Body: io.NopCloser(strings.NewReader("unavailable"))}, nil
				})
				dir := t.TempDir()
				err, out := captureHostPoll(t, func() error { return u.pollHostAt(filepath.Join(dir, "release"), filepath.Join(dir, "receipt")) })
				wantCalls := 1
				if stage == "executable" {
					wantCalls++
				}
				if err == nil || err.Error() != fmt.Sprintf("host-updater %s HTTP %d", stage, status) || out != "" || calls != wantCalls || len(h.events) != 0 {
					t.Fatalf("HTTP fault changed semantics: err=%v output=%q calls=%d", err, out, calls)
				}
				if files, err := os.ReadDir(dir); err != nil || len(files) != 0 {
					t.Fatal("download failure wrote executable/receipt", files, err)
				}
			})
		}
	}
}

func TestHostDownloadTransportFailuresRemainErrors(t *testing.T) {
	for _, stage := range []string{"manifest", "executable"} {
		for _, fault := range []error{&net.DNSError{Err: "fixture DNS failure", Name: "fixture.invalid"}, errors.New("fixture TLS failure"), context.DeadlineExceeded} {
			t.Run(stage+"/"+fault.Error(), func(t *testing.T) {
				u, _, key := setup(t)
				m := hostManifest(u)
				u.client.Transport = receiptTransport(func(r *http.Request) (*http.Response, error) {
					if stage == "executable" && r.URL.Path == "/releases/v1/general/updater.json" {
						return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader(string(hostSign(m, key))))}, nil
					}
					return nil, fault
				})
				dir := t.TempDir()
				err, out := captureHostPoll(t, func() error { return u.pollHostAt(filepath.Join(dir, "release"), filepath.Join(dir, "receipt")) })
				if !errors.Is(err, fault) || out != "" {
					t.Fatalf("transport fault swallowed: %v, %q", err, out)
				}
			})
		}
	}
}

func TestHostManifest200StillRequiresValidRelease(t *testing.T) {
	for _, mode := range []string{"unsigned", "expired", "wrong-ring", "wrong-arch", "replay", "downgrade", "over-limit", "redirect"} {
		t.Run(mode, func(t *testing.T) {
			u, _, key := setup(t)
			m := hostManifest(u)
			switch mode {
			case "expired":
				m.Published, m.Expires = u.now().Add(-2*time.Hour).Format(time.RFC3339), u.now().Add(-time.Hour).Format(time.RFC3339)
			case "wrong-ring":
				m.Ring, m.Audience = "dev", audienceFor("dev")
			case "wrong-arch":
				m.Architecture = "arm64"
				if runtime.GOARCH == "arm64" {
					m.Architecture = "amd64"
				}
			case "replay":
				if err := atomicJSON(filepath.Join(u.cfg.PrivateDir, "host-state.json"), HostState{Seen: Seen{Sequence: 2}, Attempted: map[string]bool{}}, 0600); err != nil {
					t.Fatal(err)
				}
			case "downgrade":
				m.Build -= 2
			}
			raw := hostSign(m, key)
			if mode == "unsigned" {
				m.Signature = ""
				raw, _ = json.Marshal(m)
			}
			if mode == "over-limit" {
				raw = []byte(strings.Repeat("x", maxDocument+1))
			}
			calls := 0
			u.client.Transport = receiptTransport(func(*http.Request) (*http.Response, error) {
				calls++
				if mode == "redirect" {
					return &http.Response{StatusCode: 302, Header: http.Header{"Location": {"https://get.cybrrd.com/redirected"}}, Body: io.NopCloser(strings.NewReader(""))}, nil
				}
				return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader(string(raw)))}, nil
			})
			dir := t.TempDir()
			err, out := captureHostPoll(t, func() error { return u.pollHostAt(filepath.Join(dir, "release"), filepath.Join(dir, "receipt")) })
			if err == nil || out != "" || calls != 1 {
				t.Fatalf("manifest policy weakened: err=%v output=%q calls=%d", err, out, calls)
			}
		})
	}
}
