// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Both component stores are real, isolated ROOTLESS Podman stores owned by the
// sandbox build identity. This is not a rootful engine or real systemd proof:
// runuser/store selection, systemctl and registry pull are explicit adapters.
type podmanHost struct {
	prefix       [2][]string
	u            *Updater
	calls        []string
	generation   [2]int
	binary, addr string
}

func (p *podmanHost) podman(console bool, args ...string) ([]byte, error) {
	argv := append(append([]string{}, p.prefix[componentIndex(console)]...), args...)
	b, e := exec.Command("podman", argv...).CombinedOutput()
	if e != nil {
		return b, fmt.Errorf("sandbox podman %v: %w: %s", args, e, b)
	}
	return b, nil
}
func (p *podmanHost) Run(bin string, args ...string) ([]byte, error) {
	p.calls = append(p.calls, bin+" "+strings.Join(args, " "))
	console := bin == "runuser"
	if console {
		bin, args = args[6], args[7:]
		if bin == "systemctl" {
			args = args[1:]
		}
	}
	if bin == "podman" {
		if args[0] == "pull" {
			return p.podman(console, "image", "inspect", args[1])
		}
		for i, a := range args {
			if a == "/usr/local/libexec/brrdfeeder-release:/hold:ro" {
				args[i] = p.binary + ":/hold:ro"
			}
		}
		return p.podman(console, args...)
	}
	if bin != "systemctl" {
		return nil, fmt.Errorf("unexpected %s", bin)
	}
	i := componentIndex(console)
	switch args[0] {
	case "show":
		return []byte(fmt.Sprintf("NRestarts=0\nInvocationID=%032x\nActiveState=active\n", 100+p.generation[i])), nil
	case "daemon-reload", "reset-failed":
		return nil, nil
	case "restart":
	default:
		return nil, fmt.Errorf("unexpected systemctl %v", args)
	}
	file, name := p.u.cfg.Quadlet, p.u.cfg.Container
	if console {
		file, name = p.u.cfg.ConsoleQuadlet, "brrdhouse"
	}
	raw, e := os.ReadFile(file)
	if e != nil {
		return nil, e
	}
	target, e := previousPin(raw)
	if console {
		target, e = consolePin(raw)
	}
	if e != nil {
		return nil, e
	}
	im, e := p.u.componentImage(console, target)
	if e != nil {
		return nil, e
	}
	if _, e = p.podman(console, "rm", "--ignore", "-f", name); e != nil {
		return nil, e
	}
	p.generation[i]++
	argv := []string{"run", "-d", "--name", name, "--userns=keep-id", "--cap-drop=all", "--security-opt=no-new-privileges", "--read-only"}
	if console {
		argv = append(argv, "--network=host", "-v", p.u.cfg.StateDir+":/state:ro", target, "--listen", p.addr, "--allowed-hosts", p.addr, "--status-file", "/state/status.json")
	} else {
		argv = append(argv, "--network=none", "-v", p.u.cfg.StateDir+":/state:rw", "-e", "D44_DIGEST="+im.Digest, target)
	}
	return p.podman(console, argv...)
}

func podmanFixtureRoot(t *testing.T) string {
	t.Helper()
	// Podman limits runroot to 50 bytes. t.TempDir embeds the test name and
	// inherits the runner's potentially long TMPDIR. MkdirTemp still provides
	// a unique, private (0700) root, but its explicit parent keeps paths short.
	root, err := os.MkdirTemp("/tmp", "p-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		// Test defers stop containers and reset their stores before this runs.
		if err := os.RemoveAll(root); err != nil {
			t.Errorf("remove isolated Podman fixture: %v", err)
		}
	})
	return root
}

func TestPodmanFixtureRootShortPrivateAndCleaned(t *testing.T) {
	longTmp := filepath.Join(t.TempDir(), strings.Repeat("long-runner-temp-", 8))
	if err := os.Mkdir(longTmp, 0700); err != nil {
		t.Fatal(err)
	}
	t.Setenv("TMPDIR", longTmp)
	var roots []string
	t.Run("isolated-stores", func(t *testing.T) {
		for attempt := 0; attempt < 2; attempt++ {
			root := podmanFixtureRoot(t)
			roots = append(roots, root)
			info, err := os.Stat(root)
			if err != nil {
				t.Fatal(err)
			}
			if info.Mode().Perm() != 0700 {
				t.Errorf("fixture root permissions: %o", info.Mode().Perm())
			}
			for component := 0; component < 2; component++ {
				runroot := filepath.Join(root, fmt.Sprint(component), "run")
				if len(runroot) > 50 {
					t.Errorf("Podman runroot is %d bytes (limit 50): %s", len(runroot), runroot)
				}
			}
		}
		if roots[0] == roots[1] {
			t.Error("fixture roots are not isolated")
		}
	})
	for _, root := range roots {
		if _, err := os.Stat(root); !os.IsNotExist(err) {
			t.Errorf("fixture root not removed: %s: %v", root, err)
		}
	}
}

func TestRealPodmanLifecycleAndPrune(t *testing.T) {
	if os.Getenv("D44_PODMAN") != "1" {
		t.Skip("worldport-only opt-in real container fixture")
	}
	u, _, key := setup(t)
	u.now = time.Now
	u.sleep = time.Sleep
	u.healthTimeout = 12 * time.Second
	u.cfg.Container = "d44-fixture-engine"
	root := podmanFixtureRoot(t)
	conf := filepath.Join(root, "containers.conf")
	// This isolated rootless fixture has no systemd user session. Select the
	// cgroup manager explicitly rather than depending on the developer's bus.
	if e := os.WriteFile(conf, []byte("[engine]\nlock_type=\"file\"\ncgroup_manager=\"cgroupfs\"\n"), 0600); e != nil {
		t.Fatal(e)
	}
	t.Setenv("CONTAINERS_CONF", conf)
	bin := os.Getenv("D44_FIXTURE_DIR")
	if bin == "" {
		t.Fatal("D44_FIXTURE_DIR required")
	}
	listener, e := net.Listen("tcp", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	addr := listener.Addr().String()
	listener.Close()
	p := &podmanHost{u: u, binary: filepath.Join(bin, "brrdfeeder-release"), addr: addr}
	for i := 0; i < 2; i++ {
		dir := filepath.Join(root, fmt.Sprint(i))
		p.prefix[i] = []string{"--root", filepath.Join(dir, "storage"), "--runroot", filepath.Join(dir, "run"), "--tmpdir", filepath.Join(dir, "tmp"), "--storage-driver=vfs"}
	}
	u.run = p
	u.cfg.ConsoleURL = "http://" + addr
	defer func() {
		for _, console := range []bool{false, true} {
			// Stop synchronously before resetting the disposable store; otherwise
			// conmon can still write its temporary files while the fixture cleans up.
			if _, e := p.podman(console, "stop", "--all", "--time", "2"); e != nil {
				t.Error("isolated containers shutdown", e)
			}
		}
		// Both stores share this fixture's rootless pause process. Stop both
		// components before resetting either store's namespace bookkeeping.
		for _, console := range []bool{false, true} {
			if _, e := p.podman(console, "system", "reset", "--force"); e != nil {
				t.Error("isolated store cleanup", e)
			}
		}
	}()
	for _, console := range []bool{false, true} {
		b, e := p.podman(console, "info", "--format", "{{.Host.Security.Rootless}}")
		if e != nil || strings.TrimSpace(string(b)) != "true" {
			t.Fatalf("not rootless: %v %s", e, b)
		}
	}
	context := filepath.Join(root, "build")
	if e := os.Mkdir(context, 0755); e != nil {
		t.Fatal(e)
	}
	for _, name := range []string{"fixture-engine", "brrdhouse"} {
		b, e := os.ReadFile(filepath.Join(bin, name))
		if e != nil {
			t.Fatal(e)
		}
		if e = os.WriteFile(filepath.Join(context, name), b, 0755); e != nil {
			t.Fatal(e)
		}
	}
	images := map[string]Image{}
	for _, tc := range []struct {
		name, version, seq, bad string
		console                 bool
	}{
		{"engine-old", oldVersion, "1", "false", false},
		{"engine-bad", newVersion, "2", "true", false},
		{"engine-good", newVersion, "2", "false", false},
		{"console-old", oldVersion, "1", "false", true},
		{"console-good", newVersion, "2", "false", true},
		{"console-bad", newVersion, "2", "true", true},
	} {
		recipe := "FROM scratch\nCOPY fixture-engine /fixture-engine\nENV D44_VERSION=" + tc.version + " D44_SEQ=" + tc.seq + " D44_BAD=" + tc.bad + "\nENTRYPOINT [\"/fixture-engine\"]\n"
		repo := repository
		if tc.console {
			repo = consoleRepository
			if tc.bad == "false" {
				recipe = "FROM scratch\nCOPY brrdhouse /brrdhouse\nLABEL fixture.build=" + tc.seq + "\nENTRYPOINT [\"/brrdhouse\"]\n"
			}
		}
		buildLabel := "com.macawi.brrdfeeder.build_seq"
		if tc.console {
			buildLabel = "com.macawi.brrdhouse.build_seq"
		}
		recipe += "LABEL org.opencontainers.image.revision=" + tc.version + " " + buildLabel + "=" + tc.seq + "\n"
		if e := os.WriteFile(filepath.Join(context, "Containerfile"), []byte(recipe), 0644); e != nil {
			t.Fatal(e)
		}
		tag := repo + ":d44-test-" + tc.name
		if _, e := p.podman(tc.console, "build", "--network=none", "--timestamp=0", "--format=oci", "-t", tag, context); e != nil {
			t.Fatal(e)
		}
		im, e := u.componentImage(tc.console, tag)
		if e != nil {
			t.Fatal(e)
		}
		images[tc.name] = im
	}
	old, co := images["engine-old"], images["console-old"]
	if e := atomicFile(u.cfg.Quadlet, []byte("[Container]\nImage="+repository+"@"+old.Digest+"\n"), 0644); e != nil {
		t.Fatal(e)
	}
	if e := atomicFile(u.cfg.ConsoleQuadlet, []byte("[Container]\nImage="+consoleRepository+"@"+co.Digest+"\n"), 0644); e != nil {
		t.Fatal(e)
	}
	if e := u.restart(); e != nil {
		t.Fatal(e)
	}
	if e := u.consoleRestart(); e != nil {
		t.Fatal(e)
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		s, e := u.status()
		if e == nil && s.Heartbeat.Digest == old.Digest {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("old engine did not serve", e)
		}
		time.Sleep(100 * time.Millisecond)
	}
	m := manifest(u)
	m.Digest = images["engine-bad"].Digest
	m.ConsoleDigest = co.Digest
	raw := sign(m, key)
	for i := 0; i < 2; i++ {
		queue(t, u, raw)
		if e := u.execute("apply"); e != nil {
			t.Fatal(e)
		}
	}
	if outcome(t, u)["kind"] != "quarantined" {
		t.Fatal("engine failure not quarantined")
	}
	before := len(p.calls)
	queue(t, u, raw)
	if e := u.execute("apply"); e != nil {
		t.Fatal(e)
	}
	for _, call := range p.calls[before:] {
		if strings.Contains(call, "podman pull ") {
			t.Fatal("quarantined image pulled")
		}
	}
	// Paired switch: new engine starts, bad console cannot serve; BOTH restore.
	m.Digest = images["engine-good"].Digest
	m.ConsoleDigest = images["console-bad"].Digest
	m.ConsoleBuild = 2
	m.ConsoleVersion = newVersion
	m.Sequence++
	queue(t, u, sign(m, key))
	if e := u.execute("apply"); e != nil {
		t.Fatal(e)
	}
	im, _ := u.current()
	ci, _ := u.consoleCurrent()
	if im.ID != old.ID || ci.ID != co.ID || outcome(t, u)["kind"] != "rolled_back" {
		t.Fatal("paired real containers did not roll back")
	}
	m.ConsoleDigest = images["console-good"].Digest
	m.Sequence++
	queue(t, u, sign(m, key))
	if e := u.execute("apply"); e != nil {
		t.Fatal(e)
	}
	if outcome(t, u)["kind"] != "applied" {
		t.Fatal("real healthy pair not confirmed")
	}
	for _, console := range []bool{false, true} {
		if _, e := p.podman(console, "system", "prune", "--all", "--force"); e != nil {
			t.Fatal(e)
		}
		previous, anchor := old, u.state.Anchor
		if console {
			previous, anchor = co, u.state.ConsoleAnchor
		}
		if _, e := u.componentImage(console, previous.ID); e != nil {
			t.Fatal("outgoing image pruned", e)
		}
		b, e := p.podman(console, "inspect", "--type", "container", anchor)
		if e != nil {
			t.Fatal(e)
		}
		var held []struct {
			State         struct{ Running bool }
			EffectiveCaps []string
		}
		if json.Unmarshal(b, &held) != nil || len(held) != 1 || !held[0].State.Running || len(held[0].EffectiveCaps) != 0 {
			t.Fatal("anchor protection/caps invalid")
		}
	}
	evidence(t, "real-podman.json", map[string]any{"test_only": true, "images": images, "outcome": outcome(t, u), "engine_quarantined_after": 2, "paired_console_failure_restored_both": true, "both_anchors_survived_prune": true, "host_calls": p.calls, "limitations": "real separate rootless Podman stores and actual console binary; synthetic engine status; systemd counters/restarts, runuser namespace dispatch, registry pulls and host-helper mount path adapted; not a native Pi or full installer proof"})
}
