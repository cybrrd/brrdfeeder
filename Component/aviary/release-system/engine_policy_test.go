// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"crypto/ed25519"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

func legacyPolicyQuadlet() []byte {
	return []byte("# preserved packaging comment\n[Unit]\nDescription=BRRDfeeder\nAfter=network-online.target\nStartLimitIntervalSec=300\nStartLimitBurst=3\n\n[Container]\nImage=" + repository + "@" + oldDigest + "\nContainerName=brrdfeeder-engine\n\n[Service]\nRestart=always\nRestartSec=5\nMemoryMax=256M\n\n[Install]\nWantedBy=multi-user.target\n")
}

func TestPolicyTransformNarrow(t *testing.T) {
	original := legacyPolicyQuadlet()
	want := bytes.Replace(original, []byte("StartLimitIntervalSec=300\n"), []byte("StartLimitIntervalSec=0\n"), 1)
	got, changed, err := transformEnginePolicy(original)
	if err != nil || !changed || !bytes.Equal(got, want) {
		t.Fatalf("narrow transform: %v %s", err, got)
	}
	for _, current := range [][]byte{want, bytes.Replace(want, []byte("StartLimitBurst=3\n"), nil, 1)} {
		got, changed, err = transformEnginePolicy(current)
		if err != nil || changed || !bytes.Equal(got, current) {
			t.Fatal("current policy was edited", err)
		}
	}
	for name, raw := range map[string][]byte{
		"duplicate":     bytes.Replace(original, []byte("StartLimitBurst=3"), []byte("StartLimitBurst=3\nStartLimitBurst=3"), 1),
		"section":       append(append([]byte(nil), original...), []byte("[Unit]\n")...),
		"alias":         bytes.Replace(original, []byte("StartLimitIntervalSec"), []byte("StartLimitInterval"), 1),
		"service-alias": bytes.Replace(original, []byte("Restart=always"), []byte("StartLimitInterval=300\nRestart=always"), 1),
		"duration":      bytes.Replace(original, []byte("=300\n"), []byte("=5min\n"), 1),
		"interval":      bytes.Replace(original, []byte("=300\n"), []byte("=301\n"), 1),
		"burst":         bytes.Replace(original, []byte("Burst=3"), []byte("Burst=4"), 1),
		"restart":       bytes.Replace(original, []byte("always"), []byte("on-failure"), 1),
		"backoff":       bytes.Replace(original, []byte("RestartSec=5"), []byte("RestartSec=1"), 1),
		"continued":     bytes.Replace(original, []byte("Description=BRRDfeeder"), []byte("Description=BRRDfeeder\\"), 1),
		"whitespace":    bytes.Replace(original, []byte("StartLimitIntervalSec"), []byte(" StartLimitIntervalSec"), 1),
		"wrong-section": bytes.Replace(original, []byte("[Unit]"), []byte("[Service]"), 1),
		"no-burst":      bytes.Replace(original, []byte("StartLimitBurst=3\n"), nil, 1),
		"duplicate-pin": append(append([]byte(nil), original...), []byte("Image="+repository+"@"+oldDigest+"\n")...),
		"malformed":     append(append([]byte(nil), original...), []byte("not a directive\n")...),
		"crlf":          bytes.ReplaceAll(original, []byte("\n"), []byte("\r\n")),
		"nul":           append(append([]byte(nil), original...), 0),
		"overbound":     bytes.Repeat([]byte("#"), 65537),
	} {
		t.Run(name, func(t *testing.T) {
			if _, _, err := transformEnginePolicy(raw); err == nil {
				t.Fatal("custom/malformed policy accepted")
			}
		})
	}
}

type policyHost struct {
	*fakeHost
	effective                                                     effectivePolicy
	owners                                                        map[string]string
	unitActive                                                    bool
	pid                                                           uint64
	badTarget, noInvocation, failReload, failRestart, wrongReload bool
}

func (h *policyHost) Run(bin string, args ...string) ([]byte, error) {
	targetBin, targetArgs := bin, args
	if bin == "runuser" {
		targetBin, targetArgs = args[6], args[7:]
	}
	if bin == "systemctl" && args[0] == "show" && contains(args, "--property=StartLimitIntervalUSec") {
		h.events = append(h.events, bin+" "+strings.Join(args, " "))
		active := "active"
		if !h.unitActive {
			active = "failed"
		}
		return []byte(fmt.Sprintf("StartLimitIntervalUSec=%dus\nStartLimitBurst=%d\nRestart=%s\nRestartUSec=%dus\nSourcePath=%s\nFragmentPath=%s\nDropInPaths=%s\nActiveState=%s\nMainPID=%d\n", h.effective.IntervalUS, h.effective.Burst, h.effective.Restart, h.effective.RestartUS, h.effective.Source, h.effective.Fragment, h.effective.DropIns, active, h.pid)), nil
	}
	if bin == "systemctl" && args[0] == "daemon-reload" {
		if h.failReload {
			return nil, errors.New("injected reload failure")
		}
		if !h.wrongReload {
			raw, _ := os.ReadFile(h.u.cfg.Quadlet)
			h.effective.IntervalUS = 300000000
			if bytes.Contains(raw, []byte("StartLimitIntervalSec=0\n")) {
				h.effective.IntervalUS = 0
			}
		}
	}
	if bin == "systemctl" && args[0] == "restart" && h.failRestart {
		return nil, errors.New("injected engine start failure")
	}
	if targetBin == "podman" && targetArgs[0] == "run" && targetArgs[len(targetArgs)-1] == "hold" {
		for i, arg := range targetArgs {
			if arg == "--label" {
				h.owners[targetArgs[3]] = strings.TrimPrefix(targetArgs[i+1], "io.cybrrd.policy-transaction=")
			}
		}
	}
	if targetBin == "podman" && targetArgs[0] == "ps" {
		h.events = append(h.events, bin+" "+strings.Join(args, " "))
		name := strings.TrimSuffix(strings.TrimPrefix(targetArgs[3], "name=^"), "$")
		if _, ok := h.anchors[name]; ok {
			return []byte(name + "\n"), nil
		}
		return nil, nil
	}
	prior := h.incarnation[0]
	raw, err := h.fakeHost.Run(bin, args...)
	if bin == "systemctl" && args[0] == "restart" {
		if h.noInvocation {
			h.incarnation[0] = prior
		}
		h.unitActive, h.pid = true, 42
		h.writePolicyStatus()
	}
	if targetBin == "podman" && targetArgs[0] == "inspect" {
		name := targetArgs[len(targetArgs)-1]
		if owner, ok := h.owners[name]; ok {
			var rows []map[string]any
			if json.Unmarshal(raw, &rows) == nil && len(rows) == 1 {
				rows[0]["Config"] = map[string]any{"Labels": map[string]string{"io.cybrrd.policy-transaction": owner}}
				return json.Marshal(rows)
			}
		}
	}
	return raw, err
}

func (h *policyHost) writePolicyStatus() {
	h.write()
	if h.badTarget && h.effective.IntervalUS == 0 {
		var status EngineStatus
		raw, _ := os.ReadFile(h.u.cfg.StatusFile)
		_ = json.Unmarshal(raw, &status)
		status.Heartbeat.Radio = "error"
		_ = atomicJSON(h.u.cfg.StatusFile, status, 0644)
	}
}

func setupPolicy(t *testing.T) (*Updater, *policyHost, ed25519.PrivateKey) {
	t.Helper()
	u, host, key := setup(t)
	root := filepath.Dir(u.cfg.Quadlet)
	if err := os.Chmod(root, 0700); err != nil {
		t.Fatal(err)
	}
	enabled := true
	p := &policyEnvironment{quadlet: u.cfg.Quadlet, config: root + "/config.yaml", binary: root + "/release", receipt: root + "/receipt", boundary: root, owner: uint32(os.Geteuid()), initiate: &enabled}
	p.executable = func() (string, error) { return p.binary, nil }
	u.policyTest = p
	current := []byte("#!/bin/sh\nprintf '" + strings.TrimSuffix(policyCapability, "\n") + "\\n'\n")
	previous := append(append([]byte(nil), current...), []byte("# prior bridge\n")...)
	for path, raw := range map[string][]byte{u.cfg.Quadlet: legacyPolicyQuadlet(), p.config: []byte("node: test-only\n"), p.binary: current, p.binary + ".previous": previous, p.receipt: []byte(contentHash(current) + "  " + p.binary + "\n"), p.binary + ".previous.sha256": []byte(contentHash(previous) + "\n")} {
		mode := os.FileMode(0600)
		if path == u.cfg.Quadlet {
			mode = 0644
		}
		if path == p.binary || path == p.binary+".previous" {
			mode = 0755
		}
		if err := os.WriteFile(path, raw, mode); err != nil {
			t.Fatal(err)
		}
	}
	h := &policyHost{fakeHost: host, owners: map[string]string{}, unitActive: true, pid: 42, effective: effectivePolicy{300000000, 3, "always", 5000000, u.cfg.Quadlet, "/run/systemd/generator/brrdfeeder-engine.service", ""}}
	u.run = h
	sleep := u.sleep
	u.sleep = func(d time.Duration) { sleep(d); h.writePolicyStatus() }
	return u, h, key
}

func samePolicyManifest(u *Updater) Release {
	m := manifest(u)
	m.Digest, m.Version, m.BuildSeq = oldDigest, oldVersion, 1
	return m
}

func policyFeed(u *Updater, raw []byte) {
	u.client.Transport = receiptTransport(func(*http.Request) (*http.Response, error) {
		return &http.Response{StatusCode: 200, Body: io.NopCloser(bytes.NewReader(raw))}, nil
	})
}

func TestPolicyPlannerMatrix(t *testing.T) {
	for _, engineChange := range []bool{false, true} {
		for _, consoleChange := range []bool{false, true} {
			for _, legacy := range []bool{false, true} {
				t.Run(fmt.Sprint(engineChange, consoleChange, legacy), func(t *testing.T) {
					m := Release{Digest: oldDigest, ConsoleDigest: oldDigest}
					if engineChange {
						m.Digest = newDigest
					}
					if consoleChange {
						m.ConsoleDigest = newDigest
					}
					quad := legacyPolicyQuadlet()
					if !legacy {
						quad = bytes.Replace(quad, []byte("=300\n"), []byte("=0\n"), 1)
					}
					p, err := planWorkload(quad, Image{RepoDigests: []string{repository + "@" + oldDigest}}, Image{RepoDigests: []string{consoleRepository + "@" + oldDigest}}, m, true)
					if err != nil || p.EngineImageChanged != engineChange || p.ConsoleImageChanged != consoleChange || p.EnginePolicyChanged != legacy || p.EngineRestartRequired != (legacy || engineChange) {
						t.Fatal(p, err)
					}
					if !bytes.Contains(p.Engine, []byte("StartLimitIntervalSec=0\n")) {
						t.Fatal("planned legacy policy")
					}
				})
			}
		}
	}
}

func TestPolicySignedFlowMatrix(t *testing.T) {
	for _, mode := range []string{"same-poll", "same-staged", "engine", "console", "pair"} {
		t.Run(mode, func(t *testing.T) {
			u, h, key := setupPolicy(t)
			m := samePolicyManifest(u)
			if mode == "engine" || mode == "pair" {
				m.Digest, m.Version, m.BuildSeq = newDigest, newVersion, 2
			}
			if mode == "console" || mode == "pair" {
				m.ConsoleDigest, m.ConsoleVersion, m.ConsoleBuild = newDigest, newVersion, 2
			}
			u.state.RunningSequence = m.Sequence
			u.state.Anchor = "historical-anchor"
			h.anchors[u.state.Anchor] = h.old.ID
			if err := u.save(); err != nil {
				t.Fatal(err)
			}
			consoleBefore, _ := os.ReadFile(u.cfg.ConsoleQuadlet)
			if mode == "same-staged" {
				queue(t, u, sign(m, key))
			} else {
				policyFeed(u, sign(m, key))
			}
			if err := u.execute("poll"); err != nil {
				t.Fatal(err)
			}
			if h.restarts != 1 || h.effective.IntervalUS != 0 || u.state.Active != nil || u.state.RunningSequence != m.Sequence || u.state.AppliedBuild != m.BuildSeq {
				t.Fatal("migration did not commit exactly once", h.restarts, u.state)
			}
			if h.active.Digest != m.Digest || h.consoleActive.Digest != m.ConsoleDigest {
				t.Fatal("wrong package")
			}
			wantPulls := 0
			if m.Digest == newDigest {
				wantPulls++
			}
			if m.ConsoleDigest == newDigest {
				wantPulls++
			}
			if h.pulls != wantPulls {
				t.Fatal("unnecessary pull", h.pulls)
			}
			if m.ConsoleDigest == oldDigest {
				after, _ := os.ReadFile(u.cfg.ConsoleQuadlet)
				if !bytes.Equal(consoleBefore, after) || h.consoleRestarts != 0 {
					t.Fatal("unchanged console disturbed")
				}
			}
			if m.Digest == oldDigest && u.state.Anchor != "historical-anchor" {
				t.Fatal("policy hold replaced historical anchor")
			}
			b, err := u.readPolicyBudget()
			if err != nil || !b.Used || b.Outcome != "migrated" {
				t.Fatal(b, err)
			}
			if m.Digest == oldDigest && b.Hold != "" {
				t.Fatal("temporary hold leaked")
			}
			state, _ := os.ReadFile(u.statePath())
			if bytes.Contains(state, []byte("engine_policy")) {
				t.Fatal("postcommit state incompatible with old reader")
			}
			// Repeating the genuine same release verifies actual file/effective state.
			u.sleep(21 * time.Minute)
			policyFeed(u, sign(m, key))
			if err := u.execute("poll"); err != nil {
				t.Fatal(err)
			}
			if h.restarts != 1 || h.pulls != wantPulls {
				t.Fatal("committed no-op disturbed workloads")
			}
		})
	}
}

func TestPolicyReadinessRefusesBeforeStaging(t *testing.T) {
	for _, name := range []string{"no-previous", "previous-hash", "current-hash", "executing-hash", "capability", "host-active", "drop-in", "source", "inactive", "no-pid", "custom", "symlink", "hardlink", "mode", "parent", "bad-health", "budget", "resources"} {
		t.Run(name, func(t *testing.T) {
			u, h, key := setupPolicy(t)
			m := samePolicyManifest(u)
			p := u.policyTest
			switch name {
			case "no-previous":
				_ = os.Remove(p.binary + ".previous")
			case "previous-hash":
				_ = os.WriteFile(p.binary+".previous.sha256", []byte(strings.Repeat("0", 64)+"\n"), 0600)
			case "current-hash":
				_ = os.WriteFile(p.receipt, []byte(strings.Repeat("0", 64)+"  "+p.binary+"\n"), 0600)
			case "executing-hash":
				p.executable = func() (string, error) { return p.binary + ".previous", nil }
			case "capability":
				p.probe = func(string) (string, error) { return selfTestReply, nil }
			case "host-active":
				_ = os.WriteFile(filepath.Join(u.cfg.PrivateDir, "host-transaction.json"), []byte("{}"), 0600)
			case "drop-in":
				h.effective.DropIns = "/etc/systemd/system/brrdfeeder-engine.service.d/custom.conf"
			case "source":
				h.effective.Source = "/other.container"
			case "inactive":
				h.unitActive = false
			case "no-pid":
				h.pid = 0
			case "custom":
				_ = os.WriteFile(u.cfg.Quadlet, bytes.Replace(legacyPolicyQuadlet(), []byte("=300\n"), []byte("=99\n"), 1), 0644)
			case "symlink":
				_ = os.Rename(u.cfg.Quadlet, u.cfg.Quadlet+".real")
				_ = os.Symlink(u.cfg.Quadlet+".real", u.cfg.Quadlet)
			case "hardlink":
				_ = os.Link(u.cfg.Quadlet, u.cfg.Quadlet+".link")
			case "mode":
				_ = os.Chmod(u.cfg.Quadlet, 0664)
			case "parent":
				_ = os.Chmod(p.boundary, 0777)
			case "bad-health":
				h.rollbackBad = true
				h.write()
			case "budget":
				_ = u.writePolicyBudget(policyBudget{Schema: 1, Rule: policyRule, Used: true, ID: strings.Repeat("a", 32)})
			case "resources":
				u.space = func(string) (uint64, uint64, error) { return 0, 0, nil }
			}
			before, _ := os.ReadFile(u.cfg.Quadlet)
			policyFeed(u, sign(m, key))
			if err := u.execute("poll"); err == nil {
				t.Fatal("invalid migration staged")
			}
			if _, err := os.Lstat(u.pending()); !os.IsNotExist(err) {
				t.Fatal("prerequisite failure blocked helper upgrade with new queue")
			}
			after, _ := os.ReadFile(u.cfg.Quadlet)
			if !bytes.Equal(before, after) || h.restarts != 0 || h.pulls != 0 || u.state.Active != nil {
				t.Fatal("preflight mutated workloads")
			}
			if name != "budget" {
				if _, err := os.Lstat(u.policyBudgetPath()); !os.IsNotExist(err) {
					t.Fatal("preflight consumed mutation budget")
				}
			}
		})
	}
}

func TestPolicyRefusalPreservesExistingPending(t *testing.T) {
	u, h, key := setupPolicy(t)
	raw := sign(samePolicyManifest(u), key)
	queue(t, u, raw)
	_ = os.Remove(u.policyTest.binary + ".previous")
	if err := u.execute("poll"); err == nil {
		t.Fatal("missing bridge accepted")
	}
	after, _ := os.ReadFile(u.pending())
	if !bytes.Equal(raw, after) || h.restarts != 0 {
		t.Fatal("pending work deleted or executed")
	}
}

func TestPolicyFaultRecoveryMatrix(t *testing.T) {
	points := []string{"policy-reserved", "policy-held", "anchors", "prepared", "switching", "engine-pin", "policy-reloaded", "policy-readback", "policy-counter-reset", "engine-restart", "health", "policy-receipted", "policy-hold-removed", "confirmed", "commit-pending-removed", "commit-complete"}
	for _, point := range points {
		t.Run(point, func(t *testing.T) {
			u, h, key := setupPolicy(t)
			queue(t, u, sign(samePolicyManifest(u), key))
			before, _ := os.ReadFile(u.cfg.Quadlet)
			triggered := false
			u.checkpoint = func(at string) {
				if at == point {
					triggered = true
					panic("power loss")
				}
			}
			func() {
				defer func() {
					if got := recover(); got != nil && got != "power loss" {
						panic(got)
					}
				}()
				if err := u.execute("apply"); err != nil {
					t.Fatal(err)
				}
			}()
			if !triggered {
				t.Fatal("fault not exercised")
			}
			u.checkpoint = nil
			disabled := false
			u.policyTest.initiate = &disabled
			if err := u.execute("recover"); err != nil {
				t.Fatal("bridge recovery:", err)
			}
			committed := point == "policy-receipted" || point == "policy-hold-removed" || point == "confirmed" || point == "commit-pending-removed" || point == "commit-complete"
			after, _ := os.ReadFile(u.cfg.Quadlet)
			if committed {
				if !bytes.Contains(after, []byte("StartLimitIntervalSec=0\n")) || h.effective.IntervalUS != 0 {
					t.Fatal("confirmed migration reverted")
				}
			} else if !bytes.Equal(before, after) || h.effective.IntervalUS != 300000000 {
				t.Fatal("original policy not recovered")
			}
			if u.state.Active != nil || h.consoleRestarts != 0 {
				t.Fatal("journal retained or policy-only console restarted")
			}
			b, err := u.readPolicyBudget()
			if err != nil || !b.Used || b.Hold != "" {
				t.Fatal("budget/hold not recovered", b, err)
			}
		})
	}
}

func TestPolicyFailureBudgetAndExactRollback(t *testing.T) {
	for _, mode := range []string{"health", "invocation", "console"} {
		t.Run(mode, func(t *testing.T) {
			u, h, key := setupPolicy(t)
			m := samePolicyManifest(u)
			before, _ := os.ReadFile(u.cfg.Quadlet)
			if mode == "health" {
				h.badTarget = true
			}
			if mode == "invocation" {
				h.noInvocation = true
			}
			if mode == "console" {
				m.ConsoleDigest, m.ConsoleVersion, m.ConsoleBuild = newDigest, newVersion, 2
				h.consoleBad = true
			}
			queue(t, u, sign(m, key))
			if err := u.execute("apply"); err != nil {
				t.Fatal(err)
			}
			after, _ := os.ReadFile(u.cfg.Quadlet)
			if !bytes.Equal(before, after) || h.effective.IntervalUS != 300000000 || u.state.Active != nil {
				t.Fatal("rollback incomplete")
			}
			if mode != "console" && (len(u.state.Attempts) != 0 || len(u.state.Quarantined) != 0 || h.consoleRestarts != 0) {
				t.Fatal("policy-only failure quarantined pair or restarted console")
			}
			if mode == "console" && (u.state.Attempts[packageTarget(m)] != 1 || h.consoleActive.Digest != oldDigest) {
				t.Fatal("paired accounting/rollback missing")
			}
			restarts := h.restarts
			m.Sequence++
			m.Salt = "new-metadata"
			queue(t, u, sign(m, key))
			if err := u.execute("apply"); err == nil {
				t.Fatal("metadata replenished policy budget")
			}
			if h.restarts != restarts {
				t.Fatal("failed migration looped")
			}
		})
	}
}

func TestPolicyConflictAndCorruptJournal(t *testing.T) {
	for _, mode := range []string{"file", "drop-in", "config", "backup", "schema"} {
		t.Run(mode, func(t *testing.T) {
			u, h, key := setupPolicy(t)
			queue(t, u, sign(samePolicyManifest(u), key))
			u.checkpoint = func(at string) {
				if at == "engine-pin" {
					panic("power loss")
				}
			}
			func() { defer func() { _ = recover() }(); _ = u.execute("apply") }()
			u.checkpoint = nil
			if u.state.Active == nil {
				t.Fatal("test never reached active policy journal")
			}
			switch mode {
			case "file":
				_ = os.WriteFile(u.cfg.Quadlet, []byte("operator's new bytes\n"), 0644)
			case "drop-in":
				h.effective.DropIns = "operator.conf"
			case "config":
				_ = os.WriteFile(u.policyTest.config, []byte("operator config\n"), 0600)
			case "backup":
				u.state.Active.Quadlet[0] = '!'
				_ = u.save()
			case "schema":
				u.state.Active.EnginePolicy.Schema = 99
				_ = u.save()
			}
			before, _ := os.ReadFile(u.cfg.Quadlet)
			restarts := h.restarts
			if err := u.execute("recover"); err == nil {
				t.Fatal("conflicting/corrupt recovery accepted")
			}
			after, _ := os.ReadFile(u.cfg.Quadlet)
			if !bytes.Equal(before, after) || h.restarts != restarts {
				t.Fatal("conflict overwritten")
			}
		})
	}
}

func TestPolicyBridgeDoesNotInitiate(t *testing.T) {
	u, h, key := setupPolicy(t)
	disabled := false
	u.policyTest.initiate = &disabled
	m := samePolicyManifest(u)
	u.state.RunningSequence = m.Sequence
	_ = u.save()
	policyFeed(u, sign(m, key))
	if err := u.execute("poll"); err != nil {
		t.Fatal(err)
	}
	if h.restarts != 0 || h.effective.IntervalUS != 300000000 {
		t.Fatal("bridge initiated policy migration")
	}
	if _, err := os.Lstat(u.policyBudgetPath()); !os.IsNotExist(err) {
		t.Fatal("bridge reserved migration")
	}
}

func TestPolicyStateCompatibility(t *testing.T) {
	u, _, _ := setupPolicy(t)
	if err := u.save(); err != nil {
		t.Fatal(err)
	}
	raw, _ := os.ReadFile(u.statePath())
	var old map[string]json.RawMessage
	if json.Unmarshal(raw, &old) != nil {
		t.Fatal("invalid old-shaped state")
	}
	if _, exists := old["engine_policy"]; exists {
		t.Fatal("top-level extension breaks old reader")
	}
	before := u.state
	if err := u.load(); err != nil || !reflect.DeepEqual(before, u.state) {
		t.Fatal("bridge lost legacy state", err)
	}
}

func TestPolicySameImageQuarantineIsIndependent(t *testing.T) {
	u, h, key := setupPolicy(t)
	m := samePolicyManifest(u)
	u.state.Quarantined[packageTarget(m)] = true
	u.state.Attempts[packageTarget(m)] = 3
	if err := u.save(); err != nil {
		t.Fatal(err)
	}
	policyFeed(u, sign(m, key))
	if err := u.execute("poll"); err != nil {
		t.Fatal(err)
	}
	if h.restarts != 1 || h.pulls != 0 || !u.state.Quarantined[packageTarget(m)] || u.state.Attempts[packageTarget(m)] != 3 {
		t.Fatal("policy repair coupled to image quarantine")
	}
}

func TestPolicySameImageFullPairHistory(t *testing.T) {
	u, h, key := setupPolicy(t)
	for i := 0; i < 32; i++ {
		u.state.Attempts[fmt.Sprint(i)] = 1
	}
	if err := u.save(); err != nil {
		t.Fatal(err)
	}
	queue(t, u, sign(samePolicyManifest(u), key))
	if err := u.execute("apply"); err != nil {
		t.Fatal(err)
	}
	if h.restarts != 1 || len(u.state.Attempts) != 32 {
		t.Fatal("policy repair coupled to full image failure history")
	}
}

func TestPolicyOversizedJournalBeforeReservation(t *testing.T) {
	u, h, key := setupPolicy(t)
	// Both individually valid bounded inputs, but the original pair plus policy
	// postimage exceed the encoded journal ceiling.
	for _, path := range []string{u.cfg.Quadlet, u.cfg.ConsoleQuadlet} {
		raw, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		raw = append(raw, []byte("#"+strings.Repeat("x", 65534-len(raw))+"\n")...)
		if err = os.WriteFile(path, raw, 0644); err != nil {
			t.Fatal(err)
		}
	}
	queue(t, u, sign(samePolicyManifest(u), key))
	err := u.execute("apply")
	if err == nil || !strings.Contains(err.Error(), "policy_refused_journal_size") {
		t.Fatal(err)
	}
	if _, err = os.Lstat(u.policyBudgetPath()); !os.IsNotExist(err) {
		t.Fatal("oversize consumed budget", err)
	}
	if h.restarts != 0 || len(h.owners) != 0 || u.state.Active != nil {
		t.Fatal("oversize mutated workloads or holds")
	}
}

func TestPolicyHelperOnlyStatusIsReadOnly(t *testing.T) {
	for _, mode := range []string{"bridge", "ready", "current", "fallback", "custom", "effective"} {
		t.Run(mode, func(t *testing.T) {
			u, h, _ := setupPolicy(t)
			want := "policy_pending_next_eligible_poll"
			switch mode {
			case "bridge":
				disabled := false
				u.policyTest.initiate = &disabled
				want = "policy_pending_migration_helper"
			case "current":
				raw, _, _ := transformEnginePolicy(legacyPolicyQuadlet())
				_ = os.WriteFile(u.cfg.Quadlet, raw, 0644)
				h.effective.IntervalUS = 0
				want = "policy_current"
			case "fallback":
				u.policyTest.probe = func(string) (string, error) { return selfTestReply, nil }
				want = "policy_pending_recovery_helper"
			case "custom":
				_ = os.WriteFile(u.cfg.Quadlet, []byte("[Unit]\nStartLimitIntervalSec=99\n"), 0644)
				want = "policy_refused_source"
			case "effective":
				h.effective.DropIns = "custom.conf"
				want = "policy_refused_effective"
			}
			before := treeBytes(t, u.policyTest.boundary)
			if got := u.localPolicyStatus(); got != want {
				t.Fatal(got, want)
			}
			if !reflect.DeepEqual(before, treeBytes(t, u.policyTest.boundary)) || h.restarts != 0 || h.consoleRestarts != 0 || h.pulls != 0 {
				t.Fatal("helper status mutated state")
			}
			for _, e := range h.events {
				if strings.Contains(e, "daemon-reload") || strings.Contains(e, "reset-failed") {
					t.Fatal("helper status mutated systemd", e)
				}
			}
		})
	}
}
