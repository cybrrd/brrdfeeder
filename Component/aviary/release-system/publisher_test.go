// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestHostPublisherIsPreparationOnly(t *testing.T) {
	path := filepath.Join(t.TempDir(), "updater.unsigned.json")
	args := []string{"--updater-only", "--ring", "dev", "--sha256", strings.Repeat("a", 64), "--build-seq", "2", "--sequence", "1", "--salt", "friday", "--out", path}
	if e := publishHost(args); e != nil {
		t.Fatal(e)
	}
	b, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	var m HostRelease
	if e = strictJSON(b, &m); e != nil || m.Signature != "" {
		t.Fatal("not unsigned metadata", e)
	}
	if _, e = verifyHost(b, pinnedKeys, time.Now()); e == nil {
		t.Fatal("unsigned host metadata accepted")
	}
	if e = publishHost(append(args, "--key", "/must-never-be-read")); e == nil {
		t.Fatal("private-key flag accepted")
	}
	if e = publishWith([]string{"--key", "/must-never-be-read"}, nil, pinnedKeys); e == nil {
		t.Fatal("package private-key flag accepted")
	}
}

func TestReleaseCanonicalWireBytes(t *testing.T) {
	m := Release{Schema: releaseSchema, Ring: "dev", Audience: "configured-ring:dev",
		Repository: repository, Digest: "sha256:" + strings.Repeat("a", 64),
		Version: strings.Repeat("b", 40), BuildSeq: 17,
		ConsoleDigest:  "sha256:" + strings.Repeat("c", 64),
		ConsoleVersion: strings.Repeat("d", 40), ConsoleBuild: 18,
		Sequence: 23, Rollout: 100, Salt: "synthetic",
		Published: "2026-01-01T00:00:00Z", Expires: "2026-01-02T00:00:00Z"}
	want := `["cybrrd.release.v1","dev","configured-ring:dev","ghcr.io/cybrrd/brrdfeeder","sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",17,"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","dddddddddddddddddddddddddddddddddddddddd",18,23,100,"synthetic","2026-01-01T00:00:00Z","2026-01-02T00:00:00Z"]`
	if !bytes.Equal(m.canonical(), []byte(want)) {
		t.Fatal("release wire field order, types or signature domain drift")
	}
}
func TestRemotePublisherRefusesUnsafeReplies(t *testing.T) {
	for _, kind := range []string{"good", "wrong-key", "changed", "unsigned", "redirect", "oversize", "refused"} {
		t.Run(kind, func(t *testing.T) {
			u, _, key := setup(t)
			m := manifest(u)
			s := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.URL.Path != "/sign/release" {
					t.Error("wrong endpoint")
				}
				var v Release
				if json.NewDecoder(r.Body).Decode(&v) != nil {
					t.Error("invalid request")
				}
				if v.Signature != "" {
					t.Error("publisher sent a signature")
				}
				k := key
				switch kind {
				case "wrong-key":
					_, k, _ = ed25519.GenerateKey(rand.Reader)
				case "changed":
					v.Salt = "changed"
				case "redirect":
					http.Redirect(w, r, "https://example.invalid", 307)
					return
				case "oversize":
					w.Write([]byte(strings.Repeat("x", maxDocument+1)))
					return
				case "refused":
					http.Error(w, "cosign refused", 403)
					return
				}
				if kind != "unsigned" {
					v.Signature = hex.EncodeToString(ed25519.Sign(k, v.canonical()))
				}
				json.NewEncoder(w).Encode(v)
			}))
			defer s.Close()
			client := s.Client()
			client.CheckRedirect = func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
			_, e := signReleaseRemote(m, s.URL+"/sign/release", "", client, u.keys)
			if (e == nil) != (kind == "good") {
				t.Fatal(kind, e)
			}
			if _, e = signReleaseRemote(m, strings.Replace(s.URL, "https:", "http:", 1)+"/sign/release", "", client, u.keys); e == nil {
				t.Fatal("plaintext signer accepted")
			}
		})
	}
}
