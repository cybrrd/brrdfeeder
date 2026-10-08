// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"
)

var oldDigest = "sha256:" + strings.Repeat("1", 64)
var newDigest = "sha256:" + strings.Repeat("2", 64)
var oldVersion = strings.Repeat("a", 40)
var newVersion = strings.Repeat("b", 40)

type fakeHost struct {
	u                                *Updater
	old, next                        Image
	active                           Image
	bad, stale, missing, rollbackBad bool
	pulls, restarts                  int
	anchors                          map[string]string
	events                           []string
	consoleActive                    Image
	consoleBad                       bool
	consoleRestarts                  int
	unitAuto, containerAuto          [2]uint64
	incarnation                      [2]int
}

func (h *fakeHost) Run(bin string, args ...string) ([]byte, error) {
	h.events = append(h.events, bin+" "+strings.Join(args, " "))
	data := func(v any) ([]byte, error) {
		if images, ok := v.([]Image); ok {
			var result []map[string]any
			for _, im := range images {
				revision, build := newVersion, "2"
				if im.Digest == oldDigest {
					revision, build = oldVersion, "1"
				}
				label := "com.macawi.brrdfeeder.build_seq"
				if contains(im.RepoDigests, consoleRepository+"@"+im.Digest) {
					label = "com.macawi.brrdhouse.build_seq"
				}
				result = append(result, map[string]any{"Id": im.ID, "Digest": im.Digest, "RepoDigests": im.RepoDigests, "Labels": map[string]string{"org.opencontainers.image.revision": revision, label: build}})
			}
			return json.Marshal(result)
		}
		return json.Marshal(v)
	}
	if bin == "runuser" {
		bin, args = args[6], args[7:]
		if bin == "systemctl" {
			if args[1] == "show" {
				return []byte(fmt.Sprintf("NRestarts=%d\nInvocationID=%032x\nActiveState=active\n", h.unitAuto[1], 100+h.incarnation[1])), nil
			}
			if args[1] == "reset-failed" {
				h.unitAuto[1] = 0
			}
			if args[1] == "restart" {
				h.consoleRestarts++
				h.incarnation[1]++
				h.containerAuto[1] = 0
				b, _ := os.ReadFile(h.u.cfg.ConsoleQuadlet)
				digest := oldDigest
				id := strings.Repeat("5", 64)
				if strings.Contains(string(b), newDigest) {
					digest = newDigest
					id = strings.Repeat("6", 64)
				}
				h.consoleActive = Image{ID: id, Digest: digest, RepoDigests: []string{consoleRepository + "@" + digest}}
			}
			return nil, nil
		}
		switch args[0] {
		case "info":
			return []byte(h.u.cfg.PrivateDir), nil
		case "inspect":
			id := h.consoleActive.ID
			if held, ok := h.anchors[args[len(args)-1]]; ok {
				id = held
			}
			return data([]map[string]any{{"Id": fmt.Sprintf("%064x", 200+h.incarnation[1]), "RestartCount": h.containerAuto[1], "Image": id, "State": map[string]bool{"Running": true}}})
		case "image":
			ref := args[2]
			digest := newDigest
			id := strings.Repeat("6", 64)
			if ref == strings.Repeat("5", 64) || ref == consoleRepository+"@"+oldDigest {
				digest = oldDigest
				id = strings.Repeat("5", 64)
			}
			return data([]Image{{ID: id, Digest: digest, RepoDigests: []string{consoleRepository + "@" + digest}}})
		}
	}
	if bin == "systemctl" {
		if args[0] == "show" {
			return []byte(fmt.Sprintf("NRestarts=%d\nInvocationID=%032x\nActiveState=active\n", h.unitAuto[0], 100+h.incarnation[0])), nil
		}
		if args[0] == "reset-failed" {
			h.unitAuto[0] = 0
		}
		if args[0] == "restart" {
			h.restarts++
			h.incarnation[0]++
			h.containerAuto[0] = 0
			b, _ := os.ReadFile(h.u.cfg.Quadlet)
			if strings.Contains(string(b), newDigest) {
				h.active = h.next
			} else {
				h.active = h.old
			}
			h.write()
		}
		return nil, nil
	}
	switch args[0] {
	case "info":
		return []byte(h.u.cfg.PrivateDir), nil
	case "inspect":
		if id, ok := h.anchors[args[len(args)-1]]; ok {
			return data([]map[string]any{{"Image": id, "State": map[string]bool{"Running": true}}})
		}
		return data([]map[string]any{{"Id": fmt.Sprintf("%064x", 200+h.incarnation[0]), "RestartCount": h.containerAuto[0], "Image": h.active.ID, "State": map[string]bool{"Running": true}}})
	case "image":
		ref := args[2]
		if ref == h.old.ID || ref == repository+"@"+oldDigest {
			return data([]Image{h.old})
		}
		return data([]Image{h.next})
	case "pull":
		h.pulls++
		return nil, nil
	case "create":
		name := args[2]
		h.anchors[name] = args[len(args)-1]
		return []byte(name), nil
	case "rm":
		delete(h.anchors, args[len(args)-1])
		return nil, nil
	case "run":
		if args[len(args)-1] == "hold" {
			h.anchors[args[3]] = args[len(args)-2]
			return []byte(args[3]), nil
		}
		return []byte("verify-blue: OK"), nil
	}
	return nil, fmt.Errorf("unexpected command: %s %v", bin, args)
}
func (h *fakeHost) write() {
	if h.missing && h.active.ID == h.next.ID {
		_ = os.Remove(filepath.Join(h.u.cfg.StateDir, "status.json"))
		return
	}
	s := EngineStatus{Schema: 1, Written: h.u.now(), Interval: 1}
	s.Heartbeat.Node = h.u.cfg.Node
	s.Heartbeat.Digest = h.active.Digest
	s.Heartbeat.Version = oldVersion
	s.Heartbeat.Build = 1
	s.Heartbeat.Radio = "up"
	trusted := true
	s.Heartbeat.Trusted = &trusted
	if h.active.ID == h.next.ID {
		s.Heartbeat.Version = newVersion
		s.Heartbeat.Build = 2
		if h.bad {
			s.Heartbeat.Radio = "error"
		}
		if h.stale {
			s.Written = s.Written.Add(-time.Hour)
		}
	} else if h.rollbackBad {
		s.Heartbeat.Radio = "error"
	}
	_ = atomicJSON(filepath.Join(h.u.cfg.StateDir, "status.json"), s, 0644)
}
func setup(t *testing.T) (*Updater, *fakeHost, ed25519.PrivateKey) {
	t.Helper()
	root := t.TempDir()
	c := defaults()
	c.Node = "brrdg3s1"
	c.StateDir = filepath.Join(root, "engine")
	c.PrivateDir = filepath.Join(root, "host")
	c.Quadlet = filepath.Join(root, "engine.container")
	c.StatusFile = filepath.Join(c.StateDir, "status.json")
	c.ConsoleQuadlet = filepath.Join(root, "console.container")
	c.ConsoleUID = 1001
	c.ConsoleBuild = 1
	if e := os.WriteFile(c.ConsoleQuadlet, []byte("[Container]\nImage="+consoleRepository+"@"+oldDigest+"\n"), 0644); e != nil {
		t.Fatal(e)
	}
	for _, p := range []string{c.StateDir, c.PrivateDir} {
		if e := os.Mkdir(p, 0700); e != nil {
			t.Fatal(e)
		}
	}
	if e := os.WriteFile(c.Quadlet, []byte("[Container]\nImage="+repository+"@"+oldDigest+"\n"), 0644); e != nil {
		t.Fatal(e)
	}
	u := newUpdater(c)
	now := time.Now().UTC().Truncate(time.Second)
	u.now = func() time.Time { return now }
	u.healthTimeout = 10 * time.Second
	h := &fakeHost{u: u, old: Image{ID: strings.Repeat("3", 64), Digest: oldDigest, RepoDigests: []string{repository + "@" + oldDigest}}, next: Image{ID: strings.Repeat("4", 64), Digest: newDigest, RepoDigests: []string{repository + "@" + newDigest}}, anchors: map[string]string{}}
	h.active = h.old
	h.consoleActive = Image{ID: strings.Repeat("5", 64), Digest: oldDigest, RepoDigests: []string{consoleRepository + "@" + oldDigest}}
	web := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if h.consoleBad && h.consoleActive.Digest == newDigest {
			http.Error(w, "offline", 503)
			return
		}
		s, e := u.status()
		if e != nil {
			fmt.Fprint(w, `data-engine-state="offline"`)
			return
		}
		fmt.Fprintf(w, `<section data-engine-state="healthy" data-engine-written-at="%s"><dd>%s</dd><dd>%s</dd></section>`, s.Written.Format(time.RFC3339Nano), s.Heartbeat.Digest, s.Heartbeat.Version)
	}))
	t.Cleanup(web.Close)
	u.cfg.ConsoleURL = web.URL
	u.run = h
	u.sleep = func(d time.Duration) { now = now.Add(d); h.write() }
	h.write()
	if e := u.load(); e != nil {
		t.Fatal(e)
	}
	pub, private, e := ed25519.GenerateKey(rand.Reader)
	if e != nil {
		t.Fatal(e)
	}
	u.keys = []ed25519.PublicKey{pub}
	return u, h, private
}
func manifest(u *Updater) Release {
	return Release{Schema: releaseSchema, Ring: u.cfg.Ring, Audience: audienceFor(u.cfg.Ring), Repository: repository, Digest: newDigest, Version: newVersion, BuildSeq: 2, ConsoleDigest: oldDigest, ConsoleVersion: oldVersion, ConsoleBuild: 1, Sequence: 10, Rollout: 100, Salt: "weekly-cohort", Published: u.now().Add(-8 * 24 * time.Hour).Format(time.RFC3339), Expires: u.now().Add(22 * 24 * time.Hour).Format(time.RFC3339)}
}
func sign(m Release, key ed25519.PrivateKey) []byte {
	m.Signature = hex.EncodeToString(ed25519.Sign(key, m.canonical()))
	b, _ := json.Marshal(m)
	return b
}
func evidence(t *testing.T, name string, v any) {
	t.Helper()
	if dir := os.Getenv("D44_EVIDENCE"); dir != "" {
		if e := os.MkdirAll(dir, 0755); e != nil {
			t.Fatal(e)
		}
		if e := atomicJSON(filepath.Join(dir, name), v, 0644); e != nil {
			t.Fatal(e)
		}
	}
}
func queue(t *testing.T, u *Updater, raw []byte) {
	t.Helper()
	if e := stage(u.pending(), raw); e != nil {
		t.Fatal(e)
	}
}
func outcome(t *testing.T, u *Updater) map[string]any {
	t.Helper()
	b, e := os.ReadFile(filepath.Join(u.cfg.StateDir, "update_outcome.json"))
	if e != nil {
		t.Fatal(e)
	}
	var v map[string]any
	if e = json.Unmarshal(b, &v); e != nil {
		t.Fatal(e)
	}
	return v
}

func TestWrongKeyRefuses(t *testing.T) {
	u, h, _ := setup(t)
	_, wrong, _ := ed25519.GenerateKey(rand.Reader)
	queue(t, u, sign(manifest(u), wrong))
	if e := u.execute("apply"); e == nil || h.pulls != 0 {
		t.Fatal("wrong-key signature did not refuse before pull")
	}
}
func TestTamperedRefuses(t *testing.T) {
	u, h, key := setup(t)
	raw := sign(manifest(u), key)
	raw = []byte(strings.Replace(string(raw), "weekly-cohort", "tampered-cohort", 1))
	queue(t, u, raw)
	if e := u.execute("apply"); e == nil || h.pulls != 0 {
		t.Fatal("tampered signed manifest did not refuse")
	}
}
func TestTagTargetRefuses(t *testing.T) {
	u, _, key := setup(t)
	m := manifest(u)
	m.Digest = "latest"
	if _, e := verifyRelease(sign(m, key), u.keys, u.now()); e == nil {
		t.Fatal("signed tag-shaped target accepted")
	}
}
func TestCohortExcludes(t *testing.T) {
	u, h, key := setup(t)
	u.cfg.Node = "customer-001"
	h.write()
	m := manifest(u)
	m.Rollout = 48 // independent Python SHA-256 vector: this node's bucket is 48
	serve := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write(sign(m, key)) }))
	defer serve.Close()
	u.client = serve.Client()
	u.cfg.BaseURL = serve.URL
	if e := u.execute("poll"); e != nil {
		t.Fatal(e)
	}
	if _, e := os.Stat(u.pending()); !os.IsNotExist(e) || h.pulls != 0 {
		t.Fatal("out-of-cohort unit staged update")
	}
}

func TestOfflineWeekConverges(t *testing.T) {
	u, h, key := setup(t)
	m := manifest(u)
	// Publisher calls a TLS signer fixture; no private key enters its API/files.
	signer := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var submitted Release
		if e := json.NewDecoder(r.Body).Decode(&submitted); e != nil {
			t.Error(e)
		}
		w.Write(sign(submitted, key))
	}))
	defer signer.Close()
	out := filepath.Join(t.TempDir(), "release.json")
	if e := publishWith([]string{"--signer-url", signer.URL + "/sign/release", "--out", out, "--ring", m.Ring, "--digest", m.Digest, "--version", m.Version, "--build-seq", "2", "--console-digest", m.ConsoleDigest, "--console-version", m.ConsoleVersion, "--console-build-seq", "1", "--sequence", "10", "--salt", m.Salt, "--published-at", m.Published, "--expires-at", m.Expires}, signer.Client(), u.keys); e != nil {
		t.Fatal(e)
	}
	raw, _ := os.ReadFile(out)
	requests := 0
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests++
		if r.URL.Path != "/general/release.json" {
			t.Errorf("wrong ring URL: %s", r.URL.Path)
		}
		if r.Header.Get("Authorization") != "" {
			t.Error("release fetch has credentials")
		}
		w.Write(raw)
	}))
	defer server.Close()
	u.client = server.Client()
	u.cfg.BaseURL = server.URL
	u.state.NextPoll = u.now().Add(-7 * 24 * time.Hour)
	if e := u.save(); e != nil {
		t.Fatal(e)
	}
	if e := u.execute("poll"); e != nil {
		t.Fatal(e)
	}
	if h.active.ID != h.next.ID || outcome(t, u)["kind"] != "applied" {
		t.Fatal("offline unit did not converge")
	}
	if h.anchors[u.state.Anchor] != h.old.ID {
		t.Fatal("outgoing image not protected against prune")
	}
	if u.state.AppliedBuild != 2 {
		t.Fatal("watermark not committed")
	}
	if e := u.execute("poll"); e != nil {
		t.Fatal(e)
	}
	if requests != 1 {
		t.Fatal("poll floor bypassed")
	}
	u.sleep(21 * time.Minute)
	if e := u.execute("poll"); e != nil {
		t.Fatal(e)
	}
	if _, e := os.Stat(u.pending()); !os.IsNotExist(e) {
		t.Fatal("converged unit restaged update")
	}
	running, e := u.status()
	if e != nil {
		t.Fatal(e)
	}
	evidence(t, "offline-week.json", map[string]any{"test_only": true, "ephemeral_signing_public_key": hex.EncodeToString(key.Public().(ed25519.PublicKey)), "offline_days": 7, "blue_announcements_received": 0, "release": json.RawMessage(raw), "host_calls": h.events, "outcome": outcome(t, u), "running_status": running, "poll_http_requests": requests, "applied_build": u.state.AppliedBuild})
}
func TestUnhealthyRollsBack(t *testing.T) {
	u, h, key := setup(t)
	h.bad = true
	queue(t, u, sign(manifest(u), key))
	if e := u.execute("apply"); e != nil {
		t.Fatal(e)
	}
	if h.active.ID != h.old.ID || outcome(t, u)["kind"] != "rolled_back" || u.state.AppliedBuild != 0 {
		t.Fatal("unhealthy new generation did not roll back without advancing watermark")
	}
	evidence(t, "rollback.json", map[string]any{"calls": h.events, "outcome": outcome(t, u)})
}
func TestAttemptCapQuarantines(t *testing.T) {
	u, h, key := setup(t)
	h.bad = true
	raw := sign(manifest(u), key)
	for i := 0; i < 2; i++ {
		queue(t, u, raw)
		if e := u.execute("apply"); e != nil {
			t.Fatal(e)
		}
	}
	if !u.state.Quarantined[packageTarget(manifest(u))] || outcome(t, u)["kind"] != "quarantined" {
		t.Fatal("attempt cap did not quarantine and emit durable Red input")
	}
	queue(t, u, raw)
	if e := u.execute("apply"); e != nil {
		t.Fatal(e)
	}
	if h.pulls != 2 {
		t.Fatal("quarantined digest attempted again")
	}
	evidence(t, "quarantine.json", outcome(t, u))
	evidence(t, "attempts.json", map[string]any{"pulls": h.pulls, "restarts": h.restarts, "state": u.state, "calls": h.events})
}
func TestStaleMissingAndFailedRollback(t *testing.T) {
	for _, mode := range []string{"stale", "missing", "rollback-failed"} {
		t.Run(mode, func(t *testing.T) {
			u, h, key := setup(t)
			h.stale = mode == "stale"
			h.missing = mode == "missing"
			h.bad = mode == "rollback-failed"
			h.rollbackBad = h.bad
			queue(t, u, sign(manifest(u), key))
			e := u.execute("apply")
			if mode == "rollback-failed" {
				if e == nil || outcome(t, u)["kind"] != "rollback_failed" {
					t.Fatal("rollback failure not quarantined")
				}
				n := h.restarts
				_ = u.execute("poll")
				if h.restarts != n {
					t.Fatal("rollback failure retried without bound")
				}
			} else if e != nil || h.active.ID != h.old.ID {
				t.Fatal("invalid health did not roll back", e)
			}
		})
	}
}
func TestRingReplayAndMailbox(t *testing.T) {
	u, _, key := setup(t)
	for _, ring := range []string{"dev", "staging", "general", "stable", "rc"} {
		m := manifest(u)
		if m.eligible("customer", ring) != (ring == "general") {
			t.Fatal("ring promotion", ring)
		}
	}
	m := manifest(u)
	m.Sequence = 1
	u.state.Seen = Seen{2, "other"}
	if _, e := u.releaseRequest(sign(m, key)); e == nil {
		t.Fatal("replay accepted")
	}
	if cohort("brrdg3s1", "weekly-cohort") != 0 || cohort("customer-001", "weekly-cohort") != 48 || cohort("customer-002", "weekly-cohort") != 35 {
		t.Fatal("cohort golden vectors drifted")
	}
	raw := sign(m, key)
	queue(t, u, raw)
	if e := stage(u.pending(), []byte("newer")); !os.IsExist(e) {
		t.Fatal("mailbox overwrites accepted draft", e)
	}
	if e := u.clearPending("different-hash"); e != nil {
		t.Fatal(e)
	}
	if _, e := os.Stat(u.pending()); e != nil {
		t.Fatal("wrong draft cleared")
	}
	m.Ring = "dev"
	m.Audience = audienceFor("dev")
	m.Rollout = 50
	if _, e := verifyRelease(sign(m, key), u.keys, u.now()); e == nil {
		t.Fatal("percentage outside general accepted")
	}
}

func TestPinnedTrustAndEffectorPayloadContract(t *testing.T) {
	b, e := os.ReadFile("../engine/src/blue_policy.rs")
	if e != nil {
		t.Fatal(e)
	}
	part := strings.Split(strings.Split(string(b), "pub const KEYRING:")[1], "/// The verification outcome")[0]
	matches := regexp.MustCompile(`0x([0-9a-f]{2})`).FindAllStringSubmatch(part, -1)
	var encoded strings.Builder
	for _, m := range matches {
		encoded.WriteString(m[1])
	}
	if len(pinnedKeys) != 1 || encoded.String() != hex.EncodeToString(pinnedKeys[0]) {
		t.Fatal("Go release keyring diverges from Blue compile-pinned keyring")
	}
	for _, name := range []string{"brrdfeeder-updater.sh", "brrdfeeder-updater.path", "brrdfeeder-updater.service"} {
		source, e := os.ReadFile("../deploy/host-updater/" + name)
		if e != nil {
			t.Fatal(e)
		}
		embedded, e := units.ReadFile("units/" + name)
		if e != nil || string(source) != string(embedded) {
			t.Fatal("installer embedded effector diverges from canonical source", name)
		}
	}
}
func TestStatusBoundary(t *testing.T) {
	u, h, _ := setup(t)
	u.sleep(3 * time.Second)
	s, _ := u.status()
	s.Written = u.now().Add(-3 * time.Second)
	if e := atomicJSON(filepath.Join(u.cfg.StateDir, "status.json"), s, 0644); e != nil {
		t.Fatal(e)
	}
	if _, e := u.status(); e != nil {
		t.Fatal("exact 3x boundary", e)
	}
	s.Written = s.Written.Add(-time.Nanosecond)
	_ = atomicJSON(filepath.Join(u.cfg.StateDir, "status.json"), s, 0644)
	if _, e := u.status(); e == nil {
		t.Fatal("stale accepted")
	}
	h.write()
}
func TestBlueCannotUpdate(t *testing.T) {
	u, h, _ := setup(t)
	queue(t, u, []byte(`{"schema":"cybrrd.blue.policy.v1","cadence":"immediate"}`))
	if e := u.execute("apply"); e == nil || h.pulls != 0 || h.restarts != 0 {
		t.Fatal("Blue accepted by pull-only effector")
	}
}
func TestLargeCheckpointSurvivesJournalReload(t *testing.T) {
	u, _, _ := setup(t)
	quad := []byte(strings.Repeat("# bounded checkpoint\n", 3000))
	u.state.Active = &Transaction{Quadlet: quad, Phase: "prepared"}
	if e := u.save(); e != nil {
		t.Fatal(e)
	}
	if e := u.load(); e != nil || u.state.Active == nil || string(u.state.Active.Quadlet) != string(quad) {
		t.Fatal("valid large Quadlet checkpoint cannot recover", e)
	}
}
func TestInterruptedSwitchRecovers(t *testing.T) {
	u, h, key := setup(t)
	raw := sign(manifest(u), key)
	r, _ := u.releaseRequest(raw)
	quad, _ := os.ReadFile(u.cfg.Quadlet)
	u.state.Active = &Transaction{Request: r, Previous: repository + "@" + oldDigest, PreviousID: h.old.ID, PreviousDigest: oldDigest, PreviousBuild: 1, PreviousVersion: oldVersion, Quadlet: quad, Anchor: "brrdfeeder-rollback-test", Phase: "switching", EngineChanged: true, Console: &Checkpoint{Previous: h.consoleActive}}
	u.state.Attempts[packageTarget(*r.Release)] = 1
	h.anchors[u.state.Active.Anchor] = h.old.ID
	h.active = h.next
	updated, _ := repin(quad, r.Target)
	_ = atomicFile(u.cfg.Quadlet, updated, 0644)
	queue(t, u, raw)
	_ = u.save()
	_ = atomicJSON(u.marker(), map[string]bool{"active": true}, 0644)
	if e := u.execute("apply"); e != nil {
		t.Fatal(e)
	}
	if h.active.ID != h.old.ID || u.state.Active != nil {
		t.Fatal("interrupted transition not restored")
	}
	evidence(t, "interruption.json", map[string]any{"calls": h.events, "outcome": outcome(t, u)})
}
func TestDuplicateAndUnknownJSON(t *testing.T) {
	u, _, key := setup(t)
	raw := sign(manifest(u), key)
	for _, b := range [][]byte{append(raw, []byte(` {}`)...), []byte(strings.Replace(string(raw), `"ring":"general"`, `"ring":"dev","ring":"general"`, 1)), []byte(strings.Replace(string(raw), `"schema":`, `"extra":true,"schema":`, 1))} {
		if _, e := verifyRelease(b, u.keys, u.now()); e == nil {
			t.Fatal("ambiguous/unknown input accepted")
		}
	}
}
