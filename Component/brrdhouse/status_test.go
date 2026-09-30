// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"html"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

var proofNow = time.Date(2026, 9, 21, 12, 0, 0, 0, time.UTC)

func fixture(t *testing.T, path string, age time.Duration, interval uint64, hostile bool) {
	t.Helper()
	s := status{Schema: 1, WrittenAt: proofNow.Add(-age), Interval: interval}
	s.Heartbeat.ProductVersion = "fixture-v1"
	s.Heartbeat.Revision = "0123456789abcdef0123456789abcdef01234567"
	s.Heartbeat.Build = 1042
	s.Heartbeat.Digest = "sha256:fixture"
	s.Heartbeat.Radio = "up"
	s.Links.NATS = "connected"
	s.Links.Frames = 42
	b, _ := json.Marshal(s)
	var doc map[string]any
	_ = json.Unmarshal(b, &doc)
	str := func(normal string) string {
		if hostile {
			return attack
		}
		return normal
	}
	doc["inventory"] = map[string]any{
		"capture": []any{map[string]any{"interface": str("wlan1"), "driver": str("fixture-driver"), "monitor_mode": true, "current_channel": 6}},
		"rid_ble": map[string]any{"bd_addr": str("00:00:00:00:00:01"), "usb_id": str("1234:5678"), "observed_at": str("2026-09-21T11:59:59Z"), "state": "healthy", "current_rfkill": map[string]any{"soft_blocked": false, "hard_blocked": false, "observed_at": str("2026-09-21T11:59:59Z")}, "rfkill": map[string]any{"soft_blocked": false, "hard_blocked": false, "observed_at": str("2026-09-21T11:59:59Z")}},
		"gps":     map[string]any{"device": str("/dev/cybrrd_gps"), "state": "healthy", "fix_quality": 1, "sat_count": 8},
	}
	// Deliberately populate private/unknown fields at multiple depths. The reader
	// must not serialize raw status as HTML, JSON, debug output or SSE payloads.
	doc["email"] = "PRIVATE_EMAIL_SENTINEL"
	doc["sub"] = "PRIVATE_SUB_SENTINEL"
	doc["path"] = attack
	doc["gps_device"] = attack
	hb := doc["heartbeat"].(map[string]any)
	hb["node_id"] = "PRIVATE_NODE_SENTINEL"
	hb["nats_credentials"] = "PRIVATE_CREDENTIAL_SENTINEL"
	hb["current_position"] = map[string]any{"latitude": "PRIVATE_POSITION_SENTINEL"}
	if hostile {
		hb["product_version"] = attack
		hb["engine_version"] = attack
		hb["image_digest"] = attack
		links := doc["links"].(map[string]any)
		links["last_frame_observed"] = attack
		links["last_successful_publish"] = attack
	}
	b, err := json.Marshal(doc)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path+".next", b, 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(path+".next", path); err != nil {
		t.Fatal(err)
	}
}

const attack = "<script>alert('LAN')</script><img src=x onerror=alert(1)> ; $(touch /tmp/D43-SHOULD-NOT-EXIST) `id` & \" ' \n event: forged"

func testConsole(t *testing.T) (*console, string) {
	t.Helper()
	p := filepath.Join(t.TempDir(), "status.json")
	c, err := newConsole(p, "console.test:8080,127.0.0.1:8080,[::1]:8080")
	if err != nil {
		t.Fatal(err)
	}
	c.now = func() time.Time { return proofNow }
	return c, p
}

func render(c *console, route string) *httptest.ResponseRecorder {
	r := httptest.NewRequest("GET", "http://console.test:8080"+route, nil)
	w := httptest.NewRecorder()
	c.ServeHTTP(w, r)
	return w
}

func assertState(t *testing.T, body, want string) {
	t.Helper()
	if !strings.Contains(body, `data-engine-state="`+want+`"`) {
		t.Fatalf("expected rendered %s, got:\n%s", want, body)
	}
	if want == "offline" && (strings.Contains(body, `class="engine healthy"`) || strings.Contains(body, "Capturing") || strings.Contains(body, "Fix available")) {
		t.Fatal("offline page leaked live status")
	}
	for _, sentinel := range []string{"PRIVATE_EMAIL_SENTINEL", "PRIVATE_SUB_SENTINEL", "PRIVATE_NODE_SENTINEL", "PRIVATE_CREDENTIAL_SENTINEL", "PRIVATE_POSITION_SENTINEL"} {
		if strings.Contains(body, sentinel) {
			t.Fatalf("private field leaked: %s", sentinel)
		}
	}
}

func evidence(t *testing.T, name, body string) {
	t.Helper()
	if dir := os.Getenv("D43_EVIDENCE"); dir != "" {
		if err := os.MkdirAll(dir, 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0644); err != nil {
			t.Fatal(err)
		}
	}
}

func TestRenderedProofs(t *testing.T) {
	for _, tc := range []struct {
		name             string
		age              time.Duration
		want             string
		hostile, deleted bool
	}{
		{"fresh", time.Second, "healthy", false, false},
		{"stale", 3 * time.Hour, "offline", false, false},
		{"deleted", time.Second, "offline", false, true},
		{"escaped", time.Second, "healthy", true, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			c, p := testConsole(t)
			fixture(t, p, tc.age, 30, tc.hostile)
			if tc.deleted {
				assertState(t, render(c, "/").Body.String(), "healthy")
				if err := os.Remove(p); err != nil {
					t.Fatal(err)
				}
			}
			w := render(c, "/")
			if w.Code != 200 {
				t.Fatal(w.Code)
			}
			body := w.Body.String()
			evidence(t, tc.name+".html", body)
			want := tc.want
			// Only the mutation demonstrator opts into expecting the wrong result.
			if tc.name == "stale" && os.Getenv("D43_EXPECT_MUTANT") == "1" {
				want = "healthy"
			}
			assertState(t, body, want)
			if tc.hostile {
				if strings.Contains(body, "<script>alert") || strings.Contains(body, "<img src=x") || strings.Contains(body, attack) {
					t.Fatal("active LAN markup leaked")
				}
				if n := strings.Count(body, html.EscapeString(attack)); n != 12 {
					t.Fatalf("expected all 12 displayed hostile strings escaped, got %d", n)
				}
				if _, err := os.Stat("/tmp/D43-SHOULD-NOT-EXIST"); !os.IsNotExist(err) {
					t.Fatal("shell marker exists")
				}
			}
			t.Logf("rendered %s: %s", tc.name, want)
		})
	}
}

func TestIntervalAndInvalidInput(t *testing.T) {
	for _, tc := range []struct {
		interval uint64
		age      time.Duration
		want     string
	}{
		{30, 90 * time.Second, "healthy"}, {30, 90*time.Second + time.Nanosecond, "offline"},
		{5, 16 * time.Second, "offline"}, {120, 300 * time.Second, "healthy"}, {120, 361 * time.Second, "offline"},
		{0, time.Second, "offline"}, {maxInterval + 1, time.Second, "offline"}, {30, -time.Second, "offline"},
	} {
		t.Run(fmt.Sprintf("%d-%s", tc.interval, tc.age), func(t *testing.T) {
			c, p := testConsole(t)
			fixture(t, p, tc.age, tc.interval, false)
			assertState(t, render(c, "/").Body.String(), tc.want)
		})
	}
	for _, payload := range []string{`{`, `null`, `{}`, `{"schema_version":2}`, `{"schema_version":1,"written_at":"bad"}`, strings.Repeat(" ", maxStatusBytes+1)} {
		c, p := testConsole(t)
		if err := os.WriteFile(p, []byte(payload), 0644); err != nil {
			t.Fatal(err)
		}
		assertState(t, render(c, "/").Body.String(), "offline")
	}
	c, p := testConsole(t)
	fixture(t, p, time.Second, 30, false)
	b, _ := os.ReadFile(p)
	b = []byte(strings.Replace(string(b), `"os_clock_trusted":null`, `"os_clock_trusted":false`, 1))
	if err := os.WriteFile(p, b, 0644); err != nil {
		t.Fatal(err)
	}
	assertState(t, render(c, "/").Body.String(), "offline")
}

func TestHostAndReadOnlyBoundary(t *testing.T) {
	c, p := testConsole(t)
	fixture(t, p, time.Second, 30, false)
	for _, route := range []string{"/", "/status", "/events", "/web/htmx.min.js", "/source.tar.gz", "/LICENSE", "/unknown"} {
		for _, host := range []string{"evil.example:8080", "console.test.evil:8080", "console.test:80", "198.51.100.1:8080", "console.test:8080@evil", "console.test.:8080", ""} {
			r := httptest.NewRequest("GET", "http://console.test:8080"+route, nil)
			r.Host = host
			r.Header.Set("X-Forwarded-Host", "console.test:8080")
			w := httptest.NewRecorder()
			c.ServeHTTP(w, r)
			if w.Code != 421 {
				t.Fatalf("%s host %q: %d", route, host, w.Code)
			}
		}
	}
	for _, host := range []string{"console.test:8080", "CONSOLE.TEST:8080", "127.0.0.1:8080", "[::1]:8080"} {
		r := httptest.NewRequest("GET", "http://console.test:8080/", nil)
		r.Host = host
		w := httptest.NewRecorder()
		c.ServeHTTP(w, r)
		if w.Code != 200 {
			t.Fatalf("allowed %s: %d", host, w.Code)
		}
	}
	for _, method := range []string{"POST", "PUT", "PATCH", "DELETE", "OPTIONS"} {
		r := httptest.NewRequest(method, "http://console.test:8080/", nil)
		w := httptest.NewRecorder()
		c.ServeHTTP(w, r)
		if w.Code != 405 {
			t.Fatal(method, w.Code)
		}
	}
	for _, header := range []string{"Origin", "Sec-Fetch-Site"} {
		r := httptest.NewRequest("GET", "http://console.test:8080/", nil)
		if header == "Origin" {
			r.Header.Set(header, "https://evil.example")
		} else {
			r.Header.Set(header, "cross-site")
		}
		w := httptest.NewRecorder()
		c.ServeHTTP(w, r)
		if w.Code != 403 {
			t.Fatal(header, w.Code)
		}
	}
	w := render(c, "/")
	if w.Header().Get("Cache-Control") != "no-store" || !strings.Contains(w.Header().Get("Content-Security-Policy"), "frame-ancestors 'none'") {
		t.Fatal("missing browser protections")
	}
}

func TestSSETransitions(t *testing.T) {
	c, p := testConsole(t)
	fixture(t, p, time.Second, 30, false)
	var advance atomic.Int64
	c.now = func() time.Time { return proofNow.Add(time.Duration(advance.Load())) }
	c.period = 10 * time.Millisecond
	server := httptest.NewServer(c)
	defer server.Close()
	req, _ := http.NewRequest("GET", server.URL+"/events", nil)
	req.Host = "console.test:8080"
	client := &http.Client{Timeout: 5 * time.Second}
	res, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	if res.Header.Get("Content-Type") != "text/event-stream" {
		t.Fatal(res.Header)
	}
	scanner := bufio.NewScanner(res.Body)
	event := func(want string) {
		t.Helper()
		for i := 0; i < 10; i++ {
			var b strings.Builder
			for scanner.Scan() {
				line := scanner.Text()
				if line == "" {
					break
				}
				if strings.HasPrefix(line, "data: ") {
					b.WriteString(strings.TrimPrefix(line, "data: "))
					b.WriteByte('\n')
				}
			}
			body := b.String()
			if strings.Contains(body, `data-engine-state="`+want+`"`) {
				assertState(t, body, want)
				evidence(t, "sse-"+want+".html", body)
				return
			}
			if scanner.Err() != nil {
				t.Fatal(scanner.Err())
			}
		}
		t.Fatalf("SSE never reached %s", want)
	}
	event("healthy")
	advance.Store(int64(91 * time.Second))
	event("offline")
	fixture(t, p, -91*time.Second, 30, false)
	event("healthy")
	if err := os.Remove(p); err != nil {
		t.Fatal(err)
	}
	event("offline")
}

func TestOwnerRepairs(t *testing.T) {
	c, p := testConsole(t)
	fixture(t, p, time.Second, 30, false)
	b, _ := os.ReadFile(p)
	body := strings.NewReplacer(`"radio_status":"up"`, `"radio_status":"recovering"`, `"nats_state":"connected"`, `"nats_state":"disconnected"`, `"soft_blocked":false`, `"soft_blocked":true`, `"fix_quality":1`, `"fix_quality":0`, `"frames_last_hour":42`, `"frames_last_hour":0`).Replace(string(b))
	if err := os.WriteFile(p, []byte(body), 0644); err != nil {
		t.Fatal(err)
	}
	got := render(c, "/").Body.String()
	assertState(t, got, "healthy")
	for _, want := range []string{"Wi-Fi capture is recovering", "Bluetooth is currently blocked", "GPS has no confirmed fix", "not connected to the messaging service", "No radio activity"} {
		if !strings.Contains(got, want) {
			t.Fatal("missing repair:", want)
		}
	}
	evidence(t, "repairs.html", got)
}

// G33 regression: WHATWG SSE terminates lines on CR as well as LF, so a status
// value carrying a bare CR can split events and inject protocol lines unless CR
// is neutralized at both the projection and the writer. This test goes red on
// code that writes CR into the stream and stays green only when every emitted
// frame line is an "event: "/"data: " line under the spec's splitter.
func TestSSECarriageReturnCannotBreakFraming(t *testing.T) {
	c, p := testConsole(t)
	fixture(t, p, time.Second, 30, false)
	b, err := os.ReadFile(p)
	if err != nil {
		t.Fatal(err)
	}
	var doc map[string]any
	if err := json.Unmarshal(b, &doc); err != nil {
		t.Fatal(err)
	}
	doc["inventory"].(map[string]any)["rid_ble"].(map[string]any)["bd_addr"] =
		"AA:BB\r\revent: forged\ndata: injected\rretry: 1"
	b, err = json.Marshal(doc)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(p, b, 0644); err != nil {
		t.Fatal(err)
	}
	c.period = 10 * time.Millisecond
	server := httptest.NewServer(c)
	defer server.Close()
	req, _ := http.NewRequest("GET", server.URL+"/events", nil)
	req.Host = "console.test:8080"
	client := &http.Client{Timeout: time.Second}
	res, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	var stream []byte
	tmp := make([]byte, 8192)
	for len(stream) < 1<<16 {
		n, err := res.Body.Read(tmp)
		stream = append(stream, tmp[:n]...)
		if err != nil {
			break
		}
	}
	body := string(stream)
	if !strings.Contains(body, "\n\n") {
		t.Fatal("no complete SSE frame captured")
	}
	if strings.Contains(body, "\r") {
		t.Fatal("bare CR reached the SSE stream; it can terminate events per the WHATWG spec")
	}
	// Split on the spec's terminators (CRLF, LF, CR). Every non-empty protocol
	// line the server emits must be an event name or a data line — a value must
	// never be able to forge "event:", "data:", "retry:" or any other field.
	var forged []string
	cur := strings.Builder{}
	flush := func() {
		line := cur.String()
		cur.Reset()
		if line == "" || strings.HasPrefix(line, "event: ") || strings.HasPrefix(line, "data: ") {
			return
		}
		forged = append(forged, line)
	}
	for i := 0; i < len(body); i++ {
		switch body[i] {
		case '\r':
			flush()
			if i+1 < len(body) && body[i+1] == '\n' {
				i++
			}
		case '\n':
			flush()
		default:
			cur.WriteByte(body[i])
		}
	}
	flush()
	if len(forged) > 0 {
		t.Fatalf("non-protocol lines reachable in SSE stream: %q", forged)
	}
}

// G33: the projection strips CR/NUL (SSE framing hazards) while preserving the
// newline and tab that legitimate displayed payloads use.
func TestSanitizeKeepsDisplayedText(t *testing.T) {
	got := sanitize("line1\nline2\tend\rCR\x00NUL")
	want := "line1\nline2\tendCRNUL"
	if got != want {
		t.Fatalf("sanitize changed displayed text: %q", got)
	}
}
