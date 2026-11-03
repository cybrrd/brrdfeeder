// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"strings"
	"syscall"
	"testing"
)

func TestHelperTargetIdentity(t *testing.T) {
	if policyInitiationEnabled {
		t.Fatal("bridge build must not initiate migration")
	}
	if updaterBuild != "6" || localHelperBuild != 6 {
		t.Fatal("helper 6 compiled/local identity mismatch")
	}
	b, e := os.ReadFile("build-host.sh")
	if e != nil {
		t.Fatal(e)
	}
	if !strings.Contains(string(b), "-X main.updaterBuild=6'") {
		t.Fatal("official artifact must link build 6")
	}
	b, e = os.ReadFile("local_upgrade.go")
	if e != nil {
		t.Fatal(e)
	}
	if regexp.MustCompile("json\\.RawMessage\\(\\s*[`\"]4[`\"]\\s*\\)").Match(b) {
		t.Fatal("stale literal build-4 floor write in local upgrade source")
	}
}

// Files only: directory activity and temporary staging are not installed-file writes.
func installedMetadata(t *testing.T, root string) map[string]string {
	t.Helper()
	result := treeBytes(t, root)
	err := filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if info.IsDir() {
			return nil
		}
		s := info.Sys().(*syscall.Stat_t)
		b, err := json.Marshal([]any{s.Uid, s.Gid, s.Ino, s.Mtim, s.Ctim})
		if err != nil {
			return err
		}
		result[path] += string(b)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return result
}

func TestHelperTargetFloorAndIdempotence(t *testing.T) {
	for _, prior := range []string{"", `{"build_seq":2}`, ` {"build_seq":4,"extension":{"keep":true}} `, `{"build_seq":5}`} {
		t.Run(prior, func(t *testing.T) {
			f := newLocalFixture(t)
			floor := f.p.private + "/host-state.json"
			if prior == "" {
				if e := os.Remove(floor); e != nil {
					t.Fatal(e)
				}
			} else if e := os.WriteFile(floor, []byte(prior), 0600); e != nil {
				t.Fatal(e)
			}
			if e := f.run(func(string) {}); e != nil {
				t.Fatal(e)
			}
			raw, e := os.ReadFile(floor)
			if e != nil {
				t.Fatal(e)
			}
			var state map[string]json.RawMessage
			if e := json.Unmarshal(raw, &state); e != nil {
				t.Fatal(e)
			}
			if string(state["build_seq"]) != "6" {
				t.Fatal("expected literal helper floor 6")
			}
			if strings.Contains(prior, "extension") && string(state["extension"]) != `{"keep":true}` {
				t.Fatal("extra field lost")
			}
			before := installedMetadata(t, f.p.boundary)
			if e := f.run(func(string) {}); e != nil {
				t.Fatal(e)
			}
			if !reflect.DeepEqual(before, installedMetadata(t, f.p.boundary)) {
				t.Fatal("idempotent invocation changed installed metadata or bytes")
			}
			if e := os.WriteFile(floor, []byte(`{"build_seq":7}`), 0600); e != nil {
				t.Fatal(e)
			}
			before = installedMetadata(t, f.p.boundary)
			if e := f.run(func(string) {}); e == nil || !strings.Contains(e.Error(), "downgrade") {
				t.Fatalf("same hash bypassed higher floor: %v", e)
			}
			if !reflect.DeepEqual(before, installedMetadata(t, f.p.boundary)) {
				t.Fatal("higher floor refusal changed state")
			}
		})
	}
}

func TestHelperTargetWrongCandidateBuild(t *testing.T) {
	for _, build := range []string{"4\n", "5\n", "7\n", "6", "06\n"} {
		t.Run(strings.TrimSpace(build), func(t *testing.T) {
			f := newLocalFixture(t)
			before := treeBytes(t, f.p.boundary)
			probe := func(path, verb string) (string, error) {
				if path != f.p.binary && verb == "build-seq" {
					return build, nil
				}
				return f.probe(path, verb)
			}
			e := upgradeLocal(f.p, f.candidate, f.hash, probe, func(string) {})
			if e == nil || !strings.Contains(e.Error(), "exactly 6") {
				t.Fatalf("wrong candidate build accepted: %q %v", build, e)
			}
			after := treeBytes(t, f.p.boundary)
			for _, p := range []string{f.p.binary, f.p.receipt, f.p.launch, f.p.private + "/host-state.json"} {
				if before[p] != after[p] {
					t.Fatal("candidate rejection changed installed state")
				}
			}
		})
	}
}

func TestHelperTargetFreshInstallerLayout(t *testing.T) {
	// Exact template from released 0.9.0 9faae55f. Synthetic credentials remain
	// temporary-root evidence, never real enrollment/native proof.
	f := newLocalFixture(t)
	config, e := os.ReadFile("tests/fixtures/v0.9.0-config.yaml")
	if e != nil {
		t.Fatal(e)
	}
	config = bytes.Replace(config, []byte(`id: "unenrolled"`), []byte(`id: "node-one"`), 1)
	if e := os.WriteFile(f.p.config, config, 0600); e != nil {
		t.Fatal(e)
	}
	if e := os.Remove(f.p.private + "/host-state.json"); e != nil {
		t.Fatal(e)
	}
	before := treeBytes(t, f.p.boundary)
	if e := f.run(func(string) {}); e != nil {
		t.Fatal(e)
	}
	assertProtected(t, f, before)
	if b, _ := os.ReadFile(f.p.binary); !bytes.Equal(b, f.candidate) {
		t.Fatal("fresh layout did not install helper 6")
	}
}

func TestHelperTargetAbsentFloorPosttestRollback(t *testing.T) {
	f := newLocalFixture(t)
	floor := f.p.private + "/host-state.json"
	if e := os.Remove(floor); e != nil {
		t.Fatal(e)
	}
	f.failInstalled = true
	if e := f.run(func(string) {}); e == nil {
		t.Fatal("posttest failure accepted")
	}
	if _, e := os.Lstat(floor); !os.IsNotExist(e) {
		t.Fatal("rollback fabricated absent floor")
	}
	if b, _ := os.ReadFile(f.p.binary); !bytes.Equal(b, f.old) {
		t.Fatal("posttest did not restore old helper")
	}
}

func TestHelperTargetAdditionalRefusals(t *testing.T) {
	for _, name := range []string{"empty_candidate", "non_elf", "malformed_floor", "null_floor", "missing_floor_field", "negative_floor", "host_transaction", "workload_transaction"} {
		t.Run(name, func(t *testing.T) {
			f := newLocalFixture(t)
			write := func(path, body string) {
				t.Helper()
				if e := os.WriteFile(path, []byte(body), 0600); e != nil {
					t.Fatal(e)
				}
			}
			switch name {
			case "empty_candidate":
				f.candidate = nil
				f.hash = contentHash(f.candidate)
			case "non_elf":
				f.candidate[0] = 0
				f.hash = contentHash(f.candidate)
			case "malformed_floor":
				write(f.p.private+"/host-state.json", `{`)
			case "null_floor":
				write(f.p.private+"/host-state.json", `null`)
			case "missing_floor_field":
				write(f.p.private+"/host-state.json", `{}`)
			case "negative_floor":
				write(f.p.private+"/host-state.json", `{"build_seq":-1}`)
			case "host_transaction":
				write(f.p.private+"/host-transaction.json", `{}`)
			case "workload_transaction":
				write(f.p.private+"/state.json", `{"active":{}}`)
			}
			before := installedMetadata(t, f.p.boundary)
			if e := f.run(func(string) {}); e == nil {
				t.Fatal("invalid prerequisite accepted")
			}
			if !reflect.DeepEqual(before, installedMetadata(t, f.p.boundary)) {
				t.Fatal("refusal mutated installed state")
			}
		})
	}
}

func TestHelperTargetCompiledHostFloor(t *testing.T) {
	for _, build := range []uint64{5, 6} {
		t.Run(fmt.Sprint(build), func(t *testing.T) {
			u, h, key := setup(t)
			m := hostManifest(u)
			m.Build = build
			raw := hostSign(m, key)
			calls := 0
			u.client.Transport = receiptTransport(func(*http.Request) (*http.Response, error) {
				calls++
				return &http.Response{StatusCode: 200, Body: io.NopCloser(bytes.NewReader(raw))}, nil
			})
			dir := t.TempDir()
			binary := filepath.Join(dir, "release")
			receipt := filepath.Join(dir, "receipt")
			if e := os.WriteFile(binary, []byte("installed bytes"), 0755); e != nil {
				t.Fatal(e)
			}
			before := installedMetadata(t, dir)
			e := u.pollHostAt(binary, receipt)
			if (e != nil) != (build == 5) {
				t.Fatalf("wrong floor behavior: %v", e)
			}
			if calls != 1 || !reflect.DeepEqual(before, installedMetadata(t, dir)) || len(h.events) != 0 {
				t.Fatal("lower/equal signed helper replaced state or fetched executable")
			}
			var state HostState
			b, err := os.ReadFile(filepath.Join(u.cfg.PrivateDir, "host-state.json"))
			if err != nil || json.Unmarshal(b, &state) != nil || state.Build != 6 {
				t.Fatal("absent host state lost compiled floor 6")
			}
		})
	}
}
