// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
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

// Persist the external fake Podman/systemd substrate separately from the real
// updater files. The updater process itself is SIGKILLed, not unwound by panic.
type bootFixture struct {
	PublicKey               []byte
	Config                  Config
	Engine, Console         Image
	Anchors                 map[string]string
	UnitAuto, ContainerAuto [2]uint64
	Incarnation             [2]int
	Now                     time.Time
}
type offlineRunner struct{ host *fakeHost }

func (r offlineRunner) Run(bin string, args ...string) ([]byte, error) {
	for _, a := range args {
		if a == "pull" {
			return nil, errors.New("WAN down: pull forbidden")
		}
	}
	return r.host.Run(bin, args...)
}

type offlineTransport struct{}

func (offlineTransport) RoundTrip(*http.Request) (*http.Response, error) {
	return nil, errors.New("WAN down: manifest fetch forbidden")
}

func TestBootWorker(t *testing.T) {
	dir := os.Getenv("D44_BOOT_FIXTURE")
	if dir == "" {
		t.Skip("subprocess only")
	}
	u, h, key := setup(t)
	file := filepath.Join(dir, "substrate.json")
	if os.Getenv("D44_BOOT_RESUME") == "1" {
		raw, err := os.ReadFile(file)
		if err != nil {
			t.Fatal(err)
		}
		var f bootFixture
		if err = json.Unmarshal(raw, &f); err != nil {
			t.Fatal(err)
		}
		// Keep only this process's loopback console listener. All durable pins,
		// journals and status are those left behind by the killed process.
		f.Config.ConsoleURL = u.cfg.ConsoleURL
		u.cfg = f.Config
		u.keys = []ed25519.PublicKey{f.PublicKey}
		h.active, h.consoleActive, h.anchors = f.Engine, f.Console, f.Anchors
		h.unitAuto, h.containerAuto, h.incarnation = f.UnitAuto, f.ContainerAuto, f.Incarnation
		now := f.Now.Add(time.Second)
		u.now = func() time.Time { return now }
		u.sleep = func(d time.Duration) { now = now.Add(d); h.write() }
		h.write()
		u.run = offlineRunner{h}
		u.client = &http.Client{Transport: offlineTransport{}}
		mode := os.Getenv("D44_BOOT_RESUME_MODE")
		if mode == "" {
			mode = "recover"
		}
		if err = u.execute(mode); err != nil {
			t.Fatal(err)
		}
		if u.state.Active != nil {
			t.Fatal("journal not settled")
		}
		point := os.Getenv("D44_BOOT_POINT")
		want := oldDigest
		if point == "staged" || point == "confirmed" || strings.HasPrefix(point, "commit-") {
			want = newDigest
		}
		if h.active.Digest != want || h.consoleActive.Digest != want {
			t.Fatalf("half switched pair: engine=%s console=%s want=%s", h.active.Digest, h.consoleActive.Digest, want)
		}
		for _, p := range []string{u.cfg.Quadlet, u.cfg.ConsoleQuadlet} {
			b, err := os.ReadFile(p)
			if err != nil || !strings.Contains(string(b), "@"+want+"\n") || strings.Count(string(b), "Image=") != 1 {
				t.Fatalf("non-atomic/wrong pin %s: %s %v", p, b, err)
			}
		}
		if h.pulls != 0 {
			t.Fatal("offline recovery pulled")
		}
		if err = u.execute("recover"); err != nil {
			t.Fatal("repeat recovery:", err)
		}
		t.Logf("offline boot recovered %s to %s without pull or remote fetch", point, want)
		return
	}
	point := os.Getenv("D44_BOOT_POINT")
	if strings.HasPrefix(point, "rollback-") {
		h.bad = true
	}
	u.checkpoint = func(at string) {
		if at != point {
			return
		}
		f := bootFixture{key.Public().(ed25519.PublicKey), u.cfg, h.active, h.consoleActive, h.anchors, h.unitAuto, h.containerAuto, h.incarnation, u.now()}
		if err := atomicJSON(file, f, 0600); err != nil {
			t.Fatal(err)
		}
		if err := syscall.Kill(os.Getpid(), syscall.SIGKILL); err != nil {
			t.Fatal(err)
		}
		select {} // SIGKILL must terminate us; never defer-clean the disk state.
	}
	m := manifest(u)
	m.ConsoleDigest, m.ConsoleVersion, m.ConsoleBuild = newDigest, newVersion, 2
	if err := stage(u.pending(), sign(m, key)); err != nil {
		t.Fatal(err)
	}
	u.point("staged")
	if err := u.execute("apply"); err != nil {
		t.Fatal(err)
	}
	t.Fatalf("kill boundary %s not reached", point)
}

func TestSIGKILLBootRecoveryOffline(t *testing.T) {
	for _, point := range []string{"downloads", "anchors", "prepared", "switching", "engine-pin", "engine-restart", "console-pin", "console-restart", "health", "confirmed", "commit-pending-removed", "commit-complete", "rollback-journal", "rollback-console-pin", "rollback-console-restart", "rollback-engine-pin", "rollback-engine-restart", "rollback-complete"} {
		t.Run(point, func(t *testing.T) {
			dir := t.TempDir()
			cmd := exec.Command(os.Args[0], "-test.run=^TestBootWorker$", "-test.v")
			cmd.Env = append(os.Environ(), "TMPDIR="+dir, "D44_BOOT_FIXTURE="+dir, "D44_BOOT_POINT="+point)
			out, err := cmd.CombinedOutput()
			var ee *exec.ExitError
			if !errors.As(err, &ee) || ee.Sys().(syscall.WaitStatus).Signal() != syscall.SIGKILL {
				t.Fatalf("worker not SIGKILLed: %v\n%s", err, out)
			}
			resume := exec.Command(os.Args[0], "-test.run=^TestBootWorker$", "-test.v")
			resume.Env = append(cmd.Env, "D44_BOOT_RESUME=1")
			out, err = resume.CombinedOutput()
			if err != nil {
				t.Fatalf("boot recovery failed: %v\n%s", err, out)
			}
			t.Log(strings.TrimSpace(string(out)))
		})
	}
}

func TestKilledStageAppliesNextOfflineTick(t *testing.T) {
	dir := t.TempDir()
	c := exec.Command(os.Args[0], "-test.run=^TestBootWorker$")
	c.Env = append(os.Environ(), "TMPDIR="+dir, "D44_BOOT_FIXTURE="+dir, "D44_BOOT_POINT=staged")
	b, e := c.CombinedOutput()
	var ee *exec.ExitError
	if !errors.As(e, &ee) || ee.Sys().(syscall.WaitStatus).Signal() != syscall.SIGKILL {
		t.Fatalf("not killed: %v %s", e, b)
	}
	c = exec.Command(os.Args[0], "-test.run=^TestBootWorker$", "-test.v")
	c.Env = append(os.Environ(), "TMPDIR="+dir, "D44_BOOT_FIXTURE="+dir, "D44_BOOT_POINT=staged", "D44_BOOT_RESUME=1", "D44_BOOT_RESUME_MODE=poll")
	if b, e = c.CombinedOutput(); e != nil {
		t.Fatalf("staged recovery: %v %s", e, b)
	}
}

func TestHostKillWorker(t *testing.T) {
	dir := os.Getenv("D44_HOST_FIXTURE")
	if dir == "" {
		t.Skip("subprocess only")
	}
	u := newUpdater(defaults())
	u.cfg.PrivateDir = dir
	binary := filepath.Join(dir, "release")
	receipt := filepath.Join(dir, "receipt")
	old := []byte("#!/bin/sh\nprintf 'brrdfeeder-release self-test v1\\n'\n")
	if e := atomicFile(binary, old, 0755); e != nil {
		t.Fatal(e)
	}
	u.checkpoint = func(point string) {
		if point == os.Getenv("D44_HOST_POINT") {
			if e := syscall.Kill(os.Getpid(), syscall.SIGKILL); e != nil {
				t.Fatal(e)
			}
			select {}
		}
	}
	if e := u.switchHost(binary, receipt, append(old, []byte("# signed candidate\n")...), hostSelfTest); e != nil {
		t.Fatal(e)
	}
	t.Fatal("host kill boundary not reached")
}

func TestHostTickDefersAfterPackageRecovery(t *testing.T) {
	dir := t.TempDir()
	c := exec.Command(os.Args[0], "-test.run=^TestBootWorker$")
	c.Env = append(os.Environ(), "TMPDIR="+dir, "D44_BOOT_FIXTURE="+dir, "D44_BOOT_POINT=engine-pin")
	b, e := c.CombinedOutput()
	var ee *exec.ExitError
	if !errors.As(e, &ee) || ee.Sys().(syscall.WaitStatus).Signal() != syscall.SIGKILL {
		t.Fatalf("not killed: %v %s", e, b)
	}
	c = exec.Command(os.Args[0], "-test.run=^TestBootWorker$", "-test.v")
	c.Env = append(os.Environ(), "TMPDIR="+dir, "D44_BOOT_FIXTURE="+dir, "D44_BOOT_POINT=engine-pin", "D44_BOOT_RESUME=1", "D44_BOOT_RESUME_MODE=poll-updater")
	if b, e = c.CombinedOutput(); e != nil {
		t.Fatalf("host tick did not defer: %v %s", e, b)
	}
}

func TestHostSIGKILLFallback(t *testing.T) {
	for _, point := range []string{"host-prepared", "host-switched", "host-started"} {
		t.Run(point, func(t *testing.T) {
			dir := t.TempDir()
			cmd := exec.Command(os.Args[0], "-test.run=^TestHostKillWorker$")
			cmd.Env = append(os.Environ(), "D44_HOST_FIXTURE="+dir, "D44_HOST_POINT="+point)
			out, e := cmd.CombinedOutput()
			var ee *exec.ExitError
			if !errors.As(e, &ee) || ee.Sys().(syscall.WaitStatus).Signal() != syscall.SIGKILL {
				t.Fatalf("host not SIGKILLed: %v %s", e, out)
			}
			// Use the actual Python supervisor to recover the actual executable
			// files. The test reader accepts caller-owned /tmp; production's root
			// ownership reader has separate refusal tests.
			resume := exec.Command("python3", "-c", `import importlib.util,sys
from pathlib import Path
spec=importlib.util.spec_from_file_location('launcher','launcher.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
p=Path(sys.argv[1])
m.recover(p/'release',p,p/'receipt',read=lambda p,o,limit=m.LIMIT:p.read_bytes())
assert (p/'release').read_bytes()==(p/'release.previous').read_bytes()
assert not (p/'host-transaction.json').exists()
assert m.starts(p/'release')
`, dir)
			if out, e = resume.CombinedOutput(); e != nil {
				t.Fatalf("host fallback: %v %s", e, out)
			}
			t.Logf("%s: killed process; retained executable restored offline", point)
		})
	}
}
