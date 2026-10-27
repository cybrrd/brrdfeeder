// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"crypto/ed25519"
	"encoding/base32"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"syscall"
	"testing"
)

func testNKey(raw []byte) string {
	var crc uint16
	for _, v := range raw {
		crc ^= uint16(v) << 8
		for i := 0; i < 8; i++ {
			if crc&0x8000 != 0 {
				crc = (crc << 1) ^ 0x1021
			} else {
				crc <<= 1
			}
		}
	}
	raw = append(raw, byte(crc), byte(crc>>8))
	return base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(raw)
}
func testCreds(node string, salt byte) []byte {
	return testCredsIssuer(node, salt, 17)
}
func testCredsIssuer(node string, salt, issuerSalt byte) []byte {
	issuer := ed25519.NewKeyFromSeed(bytes.Repeat([]byte{issuerSalt}, 32))
	seed := bytes.Repeat([]byte{salt}, 32)
	key := ed25519.NewKeyFromSeed(seed)
	account := testNKey(append([]byte{0}, issuer[32:]...))
	subject := testNKey(append([]byte{160}, key[32:]...))
	payload, _ := json.Marshal(map[string]any{"name": node, "sub": subject, "iss": account, "nats": map[string]any{"type": "user", "version": 2, "issuer_account": account}, "exp": 1})
	token := base64.RawURLEncoding.EncodeToString([]byte(`{"typ":"JWT","alg":"ed25519-nkey"}`)) + "." + base64.RawURLEncoding.EncodeToString(payload)
	token += "." + base64.RawURLEncoding.EncodeToString(ed25519.Sign(issuer, []byte(token)))
	return []byte("-----BEGIN NATS USER JWT-----\n" + token + "\n------END NATS USER JWT------\n-----BEGIN USER NKEY SEED-----\n" + testNKey(append([]byte{149, 0}, seed...)) + "\n------END USER NKEY SEED------\n")
}

type localFixture struct {
	p              localPaths
	candidate, old []byte
	hash           string
	oldBuild       string
	failInstalled  bool
	calls          []string
}

func newLocalFixture(t *testing.T) *localFixture {
	t.Helper()
	root := t.TempDir()
	if e := os.Chmod(root, 0700); e != nil {
		t.Fatal(e)
	}
	uid := uint32(os.Geteuid())
	p := localPaths{binary: root + "/release", receipt: root + "/receipt", config: root + "/config.yaml", bootstrap: root + "/bootstrap.creds", active: root + "/active.creds", updater: root + "/updater.json", private: root + "/private", launch: root + "/launch", owner: uid, service: uid, boundary: root}
	if e := os.Mkdir(p.private, 0700); e != nil {
		t.Fatal(e)
	}
	old := []byte("#!/bin/sh\nprintf 'brrdfeeder-release self-test v1\\n'\n")
	candidate := make([]byte, 128)
	copy(candidate, []byte("\x7fELF\x02\x01"))
	machine := uint16(183)
	if runtime.GOARCH == "amd64" {
		machine = 62
	}
	binary.LittleEndian.PutUint16(candidate[18:20], machine)
	f := &localFixture{p: p, candidate: candidate, old: old, hash: contentHash(candidate), oldBuild: "2\n"}
	files := map[string][]byte{p.binary: old, p.receipt: []byte(contentHash(old) + "  " + p.binary + "\n"), p.config: []byte("node:\n  id: node-one\n"), p.bootstrap: testCreds("node-one", 22), p.active: testCreds("node-one", 23), p.launch: []byte("old launcher"), p.private + "/host-state.json": []byte(`{"build_seq":2,"seen":{"sequence":27,"hash":"keep"},"attempted":{"retained":true},"next_poll":"2026-10-22T00:00:00Z"}`), p.private + "/state.json": []byte(`{"applied_build":8000,"applied_console_build":9000,"seen":{"sequence":345,"hash":"keep"},"running_sequence":300,"attempts":{},"quarantined":{}}`), root + "/engine.container": []byte("Image=engine@sha256:keep\n"), root + "/console.container": []byte("Image=console@sha256:keep\n"), root + "/policy_state.json": []byte(`{"last_applied_build_seq":7900,"last_applied_unix_ms":123456}`)}
	cfg, _ := json.Marshal(Config{Node: "node-one", PrivateDir: p.private})
	files[p.updater] = cfg
	for path, b := range files {
		if e := os.WriteFile(path, b, 0600); e != nil {
			t.Fatal(e)
		}
	}
	if e := os.Chmod(p.binary, 0755); e != nil {
		t.Fatal(e)
	}
	return f
}
func (f *localFixture) probe(path, verb string) (string, error) {
	f.calls = append(f.calls, verb)
	if verb != "self-test" && verb != "build-seq" {
		return "", errors.New("forbidden process/identity call")
	}
	b, e := os.ReadFile(path)
	if e != nil {
		return "", e
	}
	old := bytes.Equal(b, f.old)
	if f.failInstalled && path == f.p.binary && !old {
		return "", errors.New("injected loader failure")
	}
	if verb == "self-test" {
		return selfTestReply, nil
	}
	if old {
		return f.oldBuild, nil
	}
	return "4\n", nil
}
func treeBytes(t *testing.T, root string) map[string]string {
	t.Helper()
	out := map[string]string{}
	e := filepath.Walk(root, func(path string, info os.FileInfo, e error) error {
		if e != nil {
			return e
		}
		if !info.IsDir() {
			if info.Mode()&os.ModeSymlink != 0 {
				target, e := os.Readlink(path)
				if e != nil {
					return e
				}
				out[path] = info.Mode().String() + ":link:" + target
				return nil
			}
			b, e := os.ReadFile(path)
			if e != nil {
				return e
			}
			out[path] = info.Mode().String() + ":" + string(b)
		}
		return nil
	})
	if e != nil {
		t.Fatal(e)
	}
	return out
}
func (f *localFixture) run(point func(string)) error {
	return upgradeLocal(f.p, f.candidate, f.hash, f.probe, point)
}
func TestLocalUpgradeRefusals(t *testing.T) {
	cases := []struct {
		name, want string
		change     func(*localFixture)
	}{
		{"missing_bootstrap", "bootstrap", func(f *localFixture) { os.Remove(f.p.bootstrap) }},
		// Synthetic bundles with an empty signed node isolate the config-ID guard.
		{"missing_node", "node ID", func(f *localFixture) {
			os.WriteFile(f.p.config, []byte("node: {}\n"), 0600)
			os.WriteFile(f.p.updater, []byte(`{"node_id":"","private_dir":"`+f.p.private+`"}`), 0600)
			os.WriteFile(f.p.bootstrap, testCreds("", 22), 0600)
			os.WriteFile(f.p.active, testCreds("", 23), 0600)
		}},
		{"corrupt_receipt", "receipt", func(f *localFixture) {
			os.WriteFile(f.p.receipt, []byte(strings.Repeat("0", 64)+"  "+f.p.binary+"\n"), 0600)
		}},
		{"downgrade", "downgrade", func(f *localFixture) { f.oldBuild = "5\n" }},
		{"equal_build_different_hash", "equal-build", func(f *localFixture) { f.oldBuild = "4\n" }},
		{"wrong_arch", "architecture", func(f *localFixture) { f.candidate[18] = 0; f.hash = contentHash(f.candidate) }},
		{"missing_active", "active", func(f *localFixture) { os.Remove(f.p.active) }},
		{"bad_active_signature", "credentials", func(f *localFixture) { b, _ := os.ReadFile(f.p.active); b[40] ^= 1; os.WriteFile(f.p.active, b, 0600) }},
		{"candidate_hash", "SHA256", func(f *localFixture) { f.hash = strings.Repeat("0", 64) }},
		{"persisted_floor", "downgrade", func(f *localFixture) { os.WriteFile(f.p.private+"/host-state.json", []byte(`{"build_seq":8}`), 0600) }},
		{"pending_work", "pending", func(f *localFixture) { os.WriteFile(f.p.private+"/pending_update.json", []byte(`{}`), 0600) }},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			f := newLocalFixture(t)
			c.change(f)
			before := treeBytes(t, f.p.boundary)
			e := f.run(func(string) {})
			after := treeBytes(t, f.p.boundary)
			if !reflect.DeepEqual(before, after) {
				t.Error("REFUSAL MUTATED INSTALLED STATE")
			}
			if e == nil || !strings.Contains(e.Error(), c.want) {
				t.Errorf("want refusal %q, got %v", c.want, e)
			}
		})
	}
}
func assertProtected(t *testing.T, f *localFixture, before map[string]string) {
	t.Helper()
	after := treeBytes(t, f.p.boundary)
	for _, path := range []string{f.p.config, f.p.bootstrap, f.p.active, f.p.updater, f.p.private + "/state.json", f.p.boundary + "/engine.container", f.p.boundary + "/console.container", f.p.boundary + "/policy_state.json"} {
		if before[path] != after[path] {
			t.Errorf("protected bytes changed: %s", filepath.Base(path))
		}
	}
}
func TestLocalUpgradeSuccess(t *testing.T) {
	f := newLocalFixture(t)
	before := treeBytes(t, f.p.boundary)
	if e := f.run(func(string) {}); e != nil {
		t.Fatal(e)
	}
	assertProtected(t, f, before)
	b, _ := os.ReadFile(f.p.binary)
	if !bytes.Equal(b, f.candidate) {
		t.Fatal("candidate not installed")
	}
	b, _ = os.ReadFile(f.p.receipt)
	if string(b) != f.hash+"  "+f.p.binary+"\n" {
		t.Fatal("receipt")
	}
	var state map[string]any
	b, _ = os.ReadFile(f.p.private + "/host-state.json")
	json.Unmarshal(b, &state)
	if state["build_seq"] != float64(4) || state["seen"].(map[string]any)["sequence"] != float64(27) || state["attempted"].(map[string]any)["retained"] != true {
		t.Fatal("host floor/sequence preservation")
	}
	if _, e := os.Stat(f.p.private + "/host-transaction.json"); !os.IsNotExist(e) {
		t.Fatal("journal remains")
	}
	// Second invocation is an exact-hash no-op, preserving all state bytes.
	before = treeBytes(t, f.p.boundary)
	if e := f.run(func(string) {}); e != nil {
		t.Fatal(e)
	}
	if !reflect.DeepEqual(before, treeBytes(t, f.p.boundary)) {
		t.Fatal("idempotent upgrade wrote state")
	}
}
func TestLocalUpgradeFailedSelfTestRollsBack(t *testing.T) {
	f := newLocalFixture(t)
	before := treeBytes(t, f.p.boundary)
	f.failInstalled = true
	if e := f.run(func(string) {}); e == nil || !strings.Contains(e.Error(), "candidate self-test") {
		t.Fatalf("wanted failed candidate self-test, got %v", e)
	}
	assertProtected(t, f, before)
	after := treeBytes(t, f.p.boundary)
	for _, path := range []string{f.p.binary, f.p.receipt, f.p.private + "/host-state.json"} {
		if before[path] != after[path] {
			t.Fatal("rollback bytes differ")
		}
	}
}
func TestLocalUpgradeCrashRecovery(t *testing.T) {
	for _, phase := range []string{"prepared", "switched", "receipted", "floored"} {
		t.Run(phase, func(t *testing.T) {
			f := newLocalFixture(t)
			before := treeBytes(t, f.p.boundary)
			func() {
				defer func() {
					if recover() == nil {
						t.Fatal("interruption not reached")
					}
				}()
				_ = f.run(func(s string) {
					if s == phase {
						panic("simulated process death")
					}
				})
			}()
			// Use the actual newly installed independent launcher, not the Go rollback.
			code := `import importlib.machinery,pathlib,sys
m=importlib.machinery.SourceFileLoader('recovery',sys.argv[1]).load_module()
m.recover(pathlib.Path(sys.argv[2]),pathlib.Path(sys.argv[3]),pathlib.Path(sys.argv[4]),read=lambda p,o,limit=m.LIMIT:p.read_bytes())`
			out, e := exec.Command("python3", "-I", "-c", code, f.p.launch, f.p.binary, f.p.private, f.p.receipt).CombinedOutput()
			if e != nil {
				t.Fatalf("launcher recovery: %v %s", e, out)
			}
			assertProtected(t, f, before)
			after := treeBytes(t, f.p.boundary)
			for _, path := range []string{f.p.binary, f.p.receipt, f.p.private + "/host-state.json"} {
				if before[path] != after[path] {
					t.Fatal("crash recovery bytes differ: " + path)
				}
			}
		})
	}
}
func TestLocalUpgradeLocks(t *testing.T) {
	for _, name := range []string{"launch.lock", "lock"} {
		t.Run(name, func(t *testing.T) {
			f := newLocalFixture(t)
			l, e := os.OpenFile(f.p.private+"/"+name, os.O_CREATE|os.O_RDWR, 0600)
			if e != nil {
				t.Fatal(e)
			}
			defer l.Close()
			if e = syscall.Flock(int(l.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); e != nil {
				t.Fatal(e)
			}
			before := treeBytes(t, f.p.boundary)
			if e = f.run(func(string) {}); e == nil || !strings.Contains(e.Error(), "busy") {
				t.Fatalf("lock bypass: %v", e)
			}
			assertProtected(t, f, before)
			b, _ := os.ReadFile(f.p.binary)
			if !bytes.Equal(b, f.old) {
				t.Fatal("busy helper replaced")
			}
		})
	}
}

func TestLocalUpgradeAbsentHostFloorRecovery(t *testing.T) {
	f := newLocalFixture(t)
	if e := os.Remove(f.p.private + "/host-state.json"); e != nil {
		t.Fatal(e)
	}
	func() {
		defer func() {
			if recover() == nil {
				t.Fatal("interruption not reached")
			}
		}()
		_ = f.run(func(s string) {
			if s == "floored" {
				panic("interrupted")
			}
		})
	}()
	code := `import importlib.machinery,pathlib,sys
m=importlib.machinery.SourceFileLoader('recovery',sys.argv[1]).load_module()
m.recover(pathlib.Path(sys.argv[2]),pathlib.Path(sys.argv[3]),pathlib.Path(sys.argv[4]),read=lambda p,o,limit=m.LIMIT:p.read_bytes())`
	if out, e := exec.Command("python3", "-I", "-B", "-c", code, f.p.launch, f.p.binary, f.p.private, f.p.receipt).CombinedOutput(); e != nil {
		t.Fatalf("%v %s", e, out)
	}
	if _, e := os.Stat(f.p.private + "/host-state.json"); !os.IsNotExist(e) {
		t.Fatal("absent prior floor not restored")
	}
	if b, _ := os.ReadFile(f.p.binary); !bytes.Equal(b, f.old) {
		t.Fatal("old helper not restored")
	}
}

func TestLocalUpgradeCandidateBuildAndPretest(t *testing.T) {
	for _, verb := range []string{"self-test", "build-seq"} {
		t.Run(verb, func(t *testing.T) {
			f := newLocalFixture(t)
			before := treeBytes(t, f.p.boundary)
			probe := func(path, operation string) (string, error) {
				if path != f.p.binary && operation == verb {
					return "wrong\n", nil
				}
				return f.probe(path, operation)
			}
			e := upgradeLocal(f.p, f.candidate, f.hash, probe, func(string) {})
			if e == nil || !strings.Contains(e.Error(), "candidate") {
				t.Fatalf("wrong candidate accepted: %v", e)
			}
			assertProtected(t, f, before)
			after := treeBytes(t, f.p.boundary)
			for _, p := range []string{f.p.binary, f.p.receipt, f.p.launch, f.p.private + "/host-state.json"} {
				if before[p] != after[p] {
					t.Fatal("candidate prereq changed installed state")
				}
			}
		})
	}
}
