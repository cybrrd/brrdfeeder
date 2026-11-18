// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"crypto/ed25519"
	"encoding/json"
	"errors"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// Only test executables have this adapter. Probes use the real CLI and recovery
// uses the production state machine with a persisted synthetic systemd/Podman
// substrate. This is process/launcher coverage, not native host qualification.
func TestMain(m *testing.M) {
	if os.Getenv("POLICY_PROCESS_FIXTURE") != "" && len(os.Args) == 2 {
		switch os.Args[1] {
		case "self-test", "build-seq", "recovery-capabilities":
			if err := cli(os.Args[1:]); err != nil {
				os.Exit(1)
			}
			os.Exit(0)
		case "recover":
			os.Args = []string{os.Args[0], "-test.run=^TestPolicyProcessWorker$", "-test.v"}
			os.Setenv("POLICY_PROCESS_RESUME", "1")
		}
	}
	os.Exit(m.Run())
}

type policyProcessFixture struct {
	Boot      bootFixture
	Effective effectivePolicy
	Owners    map[string]string
}

type policyOfflineRunner struct{ host *policyHost }

func (r policyOfflineRunner) Run(bin string, args ...string) ([]byte, error) {
	if contains(args, "pull") {
		return nil, errors.New("offline recovery pull forbidden")
	}
	return r.host.Run(bin, args...)
}

func TestPolicyProcessWorker(t *testing.T) {
	dir := os.Getenv("POLICY_PROCESS_FIXTURE")
	if dir == "" {
		t.Skip("subprocess only")
	}
	u, h, key := setupPolicy(t)
	file := filepath.Join(dir, "substrate.json")
	point := os.Getenv("POLICY_PROCESS_POINT")
	if os.Getenv("POLICY_PROCESS_RESUME") == "1" {
		raw, err := os.ReadFile(file)
		if err != nil {
			t.Fatal(err)
		}
		var f policyProcessFixture
		if err = json.Unmarshal(raw, &f); err != nil {
			t.Fatal(err)
		}
		f.Boot.Config.ConsoleURL = u.cfg.ConsoleURL
		u.cfg = f.Boot.Config
		u.keys = []ed25519.PublicKey{f.Boot.PublicKey}
		h.active, h.consoleActive, h.anchors = f.Boot.Engine, f.Boot.Console, f.Boot.Anchors
		h.unitAuto, h.containerAuto, h.incarnation = f.Boot.UnitAuto, f.Boot.ContainerAuto, f.Boot.Incarnation
		h.effective, h.owners = f.Effective, f.Owners
		root := filepath.Dir(u.cfg.Quadlet)
		disabled := false
		u.policyTest = &policyEnvironment{quadlet: u.cfg.Quadlet, config: root + "/config.yaml", binary: root + "/release", receipt: root + "/receipt", boundary: root, owner: uint32(os.Geteuid()), initiate: &disabled}
		now := f.Boot.Now.Add(2 * time.Minute)
		u.now = func() time.Time { return now }
		u.sleep = func(d time.Duration) { now = now.Add(d); h.writePolicyStatus() }
		h.writePolicyStatus()
		u.run = policyOfflineRunner{h}
		u.client = &http.Client{Transport: offlineTransport{}}
		if err = u.execute("recover"); err != nil {
			t.Fatal(err)
		}
		if err = u.execute("recover"); err != nil {
			t.Fatal("repeat:", err)
		}
		committed := point == "confirmed" || point == "policy-receipted" || point == "policy-hold-removed" || strings.HasPrefix(point, "commit-")
		want := legacyPolicyQuadlet()
		interval := uint64(300000000)
		if committed {
			want = bytes.Replace(want, []byte("=300\n"), []byte("=0\n"), 1)
			interval = 0
		}
		got, _ := os.ReadFile(u.cfg.Quadlet)
		if !bytes.Equal(got, want) || h.effective.IntervalUS != interval || u.state.Active != nil || h.consoleRestarts != 0 || h.pulls != 0 {
			t.Fatalf("incorrect recovery at %s: active=%v restarts=%d pulls=%d", point, u.state.Active, h.consoleRestarts, h.pulls)
		}
		b, err := u.readPolicyBudget()
		if err != nil || !b.Used || b.Hold != "" {
			t.Fatal("hold/budget:", b, err)
		}
		t.Log("offline bridge recovery settled", point)
		return
	}
	queue(t, u, sign(samePolicyManifest(u), key))
	u.checkpoint = func(at string) {
		if at != point {
			return
		}
		f := policyProcessFixture{Boot: bootFixture{key.Public().(ed25519.PublicKey), u.cfg, h.active, h.consoleActive, h.anchors, h.unitAuto, h.containerAuto, h.incarnation, u.now()}, Effective: h.effective, Owners: h.owners}
		if err := atomicJSON(file, f, 0600); err != nil {
			t.Fatal(err)
		}
		if err := syscall.Kill(os.Getpid(), syscall.SIGKILL); err != nil {
			t.Fatal(err)
		}
		select {}
	}
	if err := u.execute("apply"); err != nil {
		t.Fatal(err)
	}
	t.Fatal("kill boundary not reached")
}

func TestPolicySIGKILLAndLauncherFallback(t *testing.T) {
	points := []string{"policy-reserved", "policy-held", "prepared", "switching", "engine-pin", "policy-reloaded", "policy-readback", "policy-counter-reset", "engine-restart", "health", "confirmed", "policy-receipted", "policy-hold-removed", "commit-pending-removed", "commit-complete"}
	for _, point := range points {
		t.Run(point, func(t *testing.T) {
			dir := t.TempDir()
			env := append(os.Environ(), "TMPDIR="+dir, "POLICY_PROCESS_FIXTURE="+dir, "POLICY_PROCESS_POINT="+point)
			kill := exec.Command(os.Args[0], "-test.run=^TestPolicyProcessWorker$")
			kill.Env = env
			raw, err := kill.CombinedOutput()
			var ee *exec.ExitError
			if !errors.As(err, &ee) || ee.Sys().(syscall.WaitStatus).Signal() != syscall.SIGKILL {
				t.Fatalf("not SIGKILL: %v %s", err, raw)
			}
			binary := filepath.Join(dir, "release")
			previous, err := os.ReadFile(os.Args[0])
			if err != nil {
				t.Fatal(err)
			}
			// Retained fallback is an actual compiled Go executable. Corrupt current
			// must fail self-test; the unmodified launcher restores and invokes previous.
			for name, data := range map[string][]byte{binary: []byte("broken ELF\n"), binary + ".previous": previous, binary + ".previous.sha256": []byte(contentHash(previous) + "\n"), filepath.Join(dir, "host-state.json"): []byte(`{"schema":1,"highest_build":7}`)} {
				if err = os.WriteFile(name, data, 0700); err != nil {
					t.Fatal(err)
				}
			}
			py := `import importlib.util,pathlib,sys,subprocess,os,stat
root=pathlib.Path(sys.argv[1]);binary=root/'release';floor=(root/'host-state.json').read_bytes()
s=importlib.util.spec_from_file_location('launcher','launcher.py');m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
def read(p,owner,limit=m.LIMIT):
 p=pathlib.Path(p)
 if p.parent!=root: raise RuntimeError('fixture path escape')
 st=p.lstat()
 if not stat.S_ISREG(st.st_mode) or st.st_uid!=owner or st.st_mode&0o022 or st.st_size>limit: raise RuntimeError('unsafe fixture')
 return p.read_bytes()
m.recover(binary,root,root/'receipt',os.geteuid(),read)
assert binary.read_bytes()==(root/'release.previous').read_bytes()
assert (root/'host-state.json').read_bytes()==floor
sys.exit(subprocess.call([str(binary),'recover']))`
			resume := exec.Command("python3", "-B", "-c", py, dir)
			resume.Env = env
			raw, err = resume.CombinedOutput()
			if err != nil {
				t.Fatalf("launcher bridge recovery: %v\n%s", err, raw)
			}
			t.Log(strings.TrimSpace(string(raw)))
		})
	}
}

func TestPolicyExportStrictReaderFixtures(t *testing.T) {
	dir := os.Getenv("POLICY_STRICT_FIXTURES")
	if dir == "" {
		t.Skip("explicit compatibility proof only")
	}
	u, _, key := setupPolicy(t)
	if err := u.save(); err != nil {
		t.Fatal(err)
	}
	write := func(name string) {
		raw, err := os.ReadFile(u.statePath())
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(filepath.Join(dir, name), raw, 0600); err != nil {
			t.Fatal(err)
		}
	}
	write("untouched.json")
	u.checkpoint = func(at string) {
		if at == "prepared" {
			write("active.json")
		}
	}
	queue(t, u, sign(samePolicyManifest(u), key))
	if err := u.execute("apply"); err != nil {
		t.Fatal(err)
	}
	write("committed.json")
}
