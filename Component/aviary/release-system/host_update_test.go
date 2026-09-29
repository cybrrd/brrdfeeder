// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

func hostManifest(u *Updater) HostRelease {
	return HostRelease{Schema: hostSchema, UpdaterOnly: true, Ring: u.cfg.Ring, Audience: audienceFor(u.cfg.Ring), SHA256: strings.Repeat("a", 64), Architecture: runtime.GOARCH, Build: 2, Sequence: 1, Rollout: 100, Salt: "host-test", Published: u.now().Add(-time.Hour).Format(time.RFC3339), Expires: u.now().Add(time.Hour).Format(time.RFC3339)}
}
func hostSign(m HostRelease, key ed25519.PrivateKey) []byte {
	m.Signature = hex.EncodeToString(ed25519.Sign(key, m.canonical()))
	b, _ := json.Marshal(m)
	return b
}
func TestHostExplicitSignatureDomain(t *testing.T) {
	u, _, key := setup(t)
	m := hostManifest(u)
	if _, e := verifyHost(hostSign(m, key), u.keys, u.now()); e != nil {
		t.Fatal(e)
	}
	for _, name := range []string{"wrong-key", "tamper", "not-explicit", "package-domain", "package-as-host", "host-as-package", "tag"} {
		t.Run(name, func(t *testing.T) {
			x := m
			raw := hostSign(x, key)
			switch name {
			case "wrong-key":
				_, k, _ := ed25519.GenerateKey(rand.Reader)
				raw = hostSign(x, k)
			case "tamper":
				raw = []byte(strings.Replace(string(raw), "host-test", "evil-test", 1))
			case "not-explicit":
				x.UpdaterOnly = false
				raw = hostSign(x, key)
			case "package-domain":
				x.Schema = releaseSchema
				raw = hostSign(x, key)
			case "package-as-host":
				raw = sign(manifest(u), key)
			case "tag":
				x.SHA256 = "latest"
				raw = hostSign(x, key)
			case "host-as-package":
				if _, e := verifyRelease(raw, u.keys, u.now()); e == nil {
					t.Fatal("host accepted as package")
				}
				return
			}
			if _, e := verifyHost(raw, u.keys, u.now()); e == nil {
				t.Fatal("invalid host update accepted")
			}
		})
	}
}
func TestHostBinaryStartupFallback(t *testing.T) {
	for _, good := range []bool{true, false} {
		t.Run(map[bool]string{true: "starts", false: "refuses-start"}[good], func(t *testing.T) {
			u, _, _ := setup(t)
			dir := t.TempDir()
			binary := filepath.Join(dir, "release")
			receipt := filepath.Join(dir, "receipt")
			old := []byte("last known good executable")
			next := []byte("new signed executable")
			if e := atomicFile(binary, old, 0755); e != nil {
				t.Fatal(e)
			}
			probe := func(p string) error {
				b, e := os.ReadFile(p)
				if e != nil || string(b) != string(next) {
					t.Fatal("tested other bytes")
				}
				if !good {
					return errors.New("cannot execute")
				}
				return nil
			}
			e := u.switchHost(binary, receipt, next, probe)
			if (e == nil) != good {
				t.Fatalf("unexpected outcome %v", e)
			}
			b, _ := os.ReadFile(binary)
			want := old
			if good {
				want = next
			}
			if string(b) != string(want) {
				t.Fatal("wrong executable after gate")
			}
			prev, _ := os.ReadFile(binary + ".previous")
			if string(prev) != string(old) {
				t.Fatal("previous not retained")
			}
			if _, e := os.Stat(filepath.Join(u.cfg.PrivateDir, "host-transaction.json")); !os.IsNotExist(e) {
				t.Fatal("transaction not settled")
			}
			if u.state.RunningSequence != 0 {
				t.Fatal("host updated package currency")
			}
		})
	}
}
func TestHostCannotSharePackageTransaction(t *testing.T) {
	u, _, _ := setup(t)
	u.state.Active = &Transaction{}
	dir := t.TempDir()
	binary := filepath.Join(dir, "release")
	if e := atomicFile(binary, []byte("previous"), 0755); e != nil {
		t.Fatal(e)
	}
	if e := u.switchHost(binary, filepath.Join(dir, "receipt"), []byte("candidate"), func(string) error { return nil }); e == nil {
		t.Fatal("host shared package transaction")
	}
	b, _ := os.ReadFile(binary)
	if string(b) != "previous" {
		t.Fatal("package transaction's host code changed")
	}
}

func TestIndependentHostSourceContract(t *testing.T) {
	b, e := os.ReadFile("../engine/Containerfile")
	if e != nil {
		t.Fatal(e)
	}
	if strings.Contains(string(b), "release-system") || strings.Contains(string(b), "brrdfeeder-release") {
		t.Fatal("updater still depends on engine image")
	}
	for _, name := range []string{"brrdfeeder-release-poll.service", "brrdfeeder-release-recover.service", "brrdfeeder-host-update.service"} {
		b, e = units.ReadFile("units/" + name)
		if e != nil || !strings.Contains(string(b), "ExecStart=/usr/local/libexec/brrdfeeder-release-launch ") {
			t.Fatalf("%s bypasses stable host supervisor", name)
		}
	}
	b, _ = units.ReadFile("units/brrdfeeder-release-recover.service")
	if strings.Contains(string(b), "network-online") || strings.Contains(string(b), "podman run") {
		t.Fatal("boot recovery depends on network/container")
	}
}

func TestSignedHostPollDownloadsAndFallsBack(t *testing.T) {
	for _, mode := range []string{"good", "broken", "wrong-build", "tampered-bytes", "excluded"} {
		t.Run(mode, func(t *testing.T) {
			u, h, key := setup(t)
			dir := t.TempDir()
			binary := filepath.Join(dir, "release")
			receipt := filepath.Join(dir, "receipt")
			old := []byte("previous installed updater")
			if e := atomicFile(binary, old, 0755); e != nil {
				t.Fatal(e)
			}
			candidate := []byte("#!/bin/sh\ncase \"$1\" in self-test) printf 'brrdfeeder-release self-test v1\\n';; build-seq) printf '2\\n';; *) exit 2;; esac\n")
			if mode == "broken" {
				candidate = []byte("cannot execute")
			}
			if mode == "wrong-build" {
				candidate = []byte(strings.Replace(string(candidate), "printf '2", "printf '9", 1))
			}
			m := hostManifest(u)
			m.SHA256 = contentHash(candidate)
			if mode == "excluded" {
				m.Rollout = 0
			}
			manifestHits, artifactHits := 0, 0
			web := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.URL.Path == "/general/updater.json" {
					manifestHits++
					w.Write(hostSign(m, key))
					return
				}
				if r.URL.Path != "/updater/"+m.SHA256+"/linux-"+runtime.GOARCH+"/brrdfeeder-release" {
					t.Error("unexpected artifact path", r.URL.Path)
					http.NotFound(w, r)
					return
				}
				artifactHits++
				if mode == "tampered-bytes" {
					// Still starts and reports the right build: only the signed
					// artifact hash, not a redundant loader failure, can catch it.
					w.Write(append(candidate, []byte("# substituted bytes\n")...))
				} else {
					w.Write(candidate)
				}
			}))
			defer web.Close()
			u.client = web.Client()
			u.cfg.BaseURL = web.URL
			e := u.pollHostAt(binary, receipt)
			if (e == nil) != (mode == "good" || mode == "excluded") {
				t.Fatalf("unexpected %s outcome %v", mode, e)
			}
			got, _ := os.ReadFile(binary)
			want := old
			if mode == "good" {
				want = candidate
			}
			if string(got) != string(want) {
				t.Fatal("wrong retained/running bytes")
			}
			if len(h.events) != 0 {
				t.Fatal("host release touched workload containers")
			}
			if e = u.pollHostAt(binary, receipt); e != nil {
				t.Fatal(e)
			}
			if manifestHits != 1 {
				t.Fatal("host poll floor bypassed")
			}
			if mode == "excluded" && artifactHits != 0 {
				t.Fatal("excluded host downloaded")
			}
			if mode == "broken" || mode == "wrong-build" {
				u.sleep(21 * time.Minute)
				if e = u.pollHostAt(binary, receipt); e != nil {
					t.Fatal(e)
				}
				if artifactHits != 1 {
					t.Fatal("failed host bytes retried")
				}
			}
		})
	}
}
