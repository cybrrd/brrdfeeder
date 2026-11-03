// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestPolicyAuthorityBeforeAnyMutation(t *testing.T) {
	for _, name := range []string{"signature", "expired", "ring", "cohort", "replay", "floor", "console-tuple", "stale", "clock", "gps", "missing-feed"} {
		t.Run(name, func(t *testing.T) {
			u, h, key := setupPolicy(t)
			m := samePolicyManifest(u)
			switch name {
			case "expired":
				m.Expires = u.now().Add(-time.Second).Format(time.RFC3339)
			case "ring":
				m.Ring = "dev"
				m.Audience = audienceFor("dev")
			case "cohort":
				m.Rollout = 0
			case "replay":
				u.state.Seen.Sequence = m.Sequence + 1
				_ = u.save()
			case "floor":
				m.BuildSeq = 0
			case "console-tuple":
				m.ConsoleVersion = newVersion
			case "gps":
				u.cfg.GPSRequired = true
			case "stale", "clock":
				var s EngineStatus
				raw, _ := os.ReadFile(u.cfg.StatusFile)
				_ = json.Unmarshal(raw, &s)
				if name == "stale" {
					s.Written = s.Written.Add(-time.Hour)
				} else {
					s.Heartbeat.Trusted = nil
				}
				_ = atomicJSON(u.cfg.StatusFile, s, 0644)
			}
			raw := sign(m, key)
			if name == "signature" {
				raw = bytes.Replace(raw, []byte(oldVersion), []byte(newVersion), 1)
			}
			if name == "missing-feed" {
				raw = []byte("unavailable")
			}
			policyFeed(u, raw)
			before, _ := os.ReadFile(u.cfg.Quadlet)
			_ = u.execute("poll")
			after, _ := os.ReadFile(u.cfg.Quadlet)
			if !bytes.Equal(before, after) || h.restarts != 0 || u.state.Active != nil {
				t.Fatal("unsigned/ineligible/unready policy mutation")
			}
			if _, err := os.Lstat(u.policyBudgetPath()); !os.IsNotExist(err) {
				t.Fatal("authority refusal consumed budget")
			}
		})
	}
}

func TestPolicyLatePreflightChanges(t *testing.T) {
	for _, name := range []string{"file", "metadata", "effective", "config", "invocation", "expiry", "fallback"} {
		t.Run(name, func(t *testing.T) {
			u, h, key := setupPolicy(t)
			m := samePolicyManifest(u)
			queue(t, u, sign(m, key))
			u.checkpoint = func(point string) {
				if point != "downloads" {
					return
				}
				switch name {
				case "file":
					_ = os.WriteFile(u.cfg.Quadlet, append(legacyPolicyQuadlet(), []byte("# operator edit\n")...), 0644)
				case "metadata":
					_ = os.Chmod(u.cfg.Quadlet, 0600)
				case "effective":
					h.effective.DropIns = "new.conf"
				case "config":
					_ = os.WriteFile(u.policyTest.config, []byte("node: edited\n"), 0600)
				case "invocation":
					h.incarnation[0]++
				case "expiry":
					u.sleep(30 * 24 * time.Hour)
				case "fallback":
					_ = os.WriteFile(u.policyTest.binary+".previous.sha256", []byte(strings.Repeat("0", 64)+"\n"), 0600)
				}
			}
			if err := u.execute("apply"); err == nil {
				t.Fatal("late prerequisite change accepted")
			}
			after, _ := os.ReadFile(u.cfg.Quadlet)
			if bytes.Contains(after, []byte("StartLimitIntervalSec=0\n")) || h.restarts != 0 || u.state.Active != nil {
				t.Fatal("late refusal mutated policy")
			}
		})
	}
}

func TestPolicyRecoveryIsBoundedAndVerifiesEffectiveState(t *testing.T) {
	u, h, key := setupPolicy(t)
	queue(t, u, sign(samePolicyManifest(u), key))
	u.checkpoint = func(point string) {
		if point == "engine-restart" {
			panic("power loss")
		}
	}
	func() { defer func() { _ = recover() }(); _ = u.execute("apply") }()
	u.checkpoint = nil
	if u.state.Active == nil || h.effective.IntervalUS != 0 {
		t.Fatal("fixture not switched")
	}
	h.failRestart = true
	if err := u.execute("recover"); err == nil {
		t.Fatal("failed start accepted")
	}
	if h.effective.IntervalUS != 300000000 || u.state.Active.Phase != "recovery_wait" || u.state.Active.EnginePolicy.RecoveryStarts != 1 {
		t.Fatal("same-image effective policy not restored before recovery start")
	}
	if err := u.execute("recover"); err == nil {
		t.Fatal("backoff not retained")
	}
	if u.state.Active.EnginePolicy.RecoveryStarts != 1 {
		t.Fatal("backoff consumed restart")
	}
	u.sleep(2 * time.Minute)
	_ = u.execute("recover")
	if u.state.Active.EnginePolicy.RecoveryStarts != 2 {
		t.Fatal("second bounded recovery attempt missing")
	}
	for i := 0; i < 3; i++ {
		u.sleep(20 * time.Minute)
		_ = u.execute("recover")
		if u.state.Active.EnginePolicy.RecoveryStarts != 2 {
			t.Fatal("timer creates unlimited restarts")
		}
	}
	h.failRestart = false
	if err := u.execute("authorize-policy-retry"); err != nil {
		t.Fatal(err)
	}
	if err := u.execute("recover"); err != nil {
		t.Fatal(err)
	}
	if u.state.Active != nil || h.effective.IntervalUS != 300000000 || h.consoleRestarts != 0 {
		t.Fatal("authorized recovery incomplete")
	}
}

func TestPolicyRetryAndReceiptCannotAuthorizeDrift(t *testing.T) {
	u, h, key := setupPolicy(t)
	m := samePolicyManifest(u)
	h.badTarget = true
	queue(t, u, sign(m, key))
	if err := u.execute("apply"); err != nil {
		t.Fatal(err)
	}
	baseline := h.restarts
	if err := u.execute("authorize-policy-retry"); err != nil {
		t.Fatal(err)
	}
	if h.restarts != baseline {
		t.Fatal("retry authorization mutated workload")
	}
	h.badTarget = false
	queue(t, u, sign(m, key))
	if err := u.execute("apply"); err != nil {
		t.Fatal(err)
	}
	// A receipt from a successful attempt must not mask an operator's later drift.
	_ = os.WriteFile(u.cfg.Quadlet, legacyPolicyQuadlet(), 0644)
	h.effective.IntervalUS = 300000000
	u.sleep(20 * time.Minute)
	policyFeed(u, sign(m, key))
	baseline = h.restarts
	if err := u.execute("poll"); err == nil || !strings.Contains(err.Error(), "retry_authorization") {
		t.Fatal("receipt incorrectly treated as current", err)
	}
	if h.restarts != baseline {
		t.Fatal("drift bypassed budget")
	}
}

func TestPolicyWriteFaults(t *testing.T) {
	for _, afterWrite := range []bool{false, true} {
		for _, point := range []string{"reservation", "prepared", "marker", "switching", "quadlet", "confirmed", "receipt", "state-clear", "outcome"} {
			t.Run(fmt.Sprint(point, afterWrite), func(t *testing.T) {
				u, h, key := setupPolicy(t)
				queue(t, u, sign(samePolicyManifest(u), key))
				fired := false
				u.write = func(path string, raw []byte, mode os.FileMode) error {
					hit := false
					switch point {
					case "reservation":
						hit = path == u.policyBudgetPath() && bytes.Contains(raw, []byte(`"outcome":"reserved"`))
					case "prepared", "switching", "confirmed":
						hit = path == u.statePath() && bytes.Contains(raw, []byte(`"phase":"`+point+`"`))
					case "marker":
						hit = path == u.marker()
					case "quadlet":
						hit = path == u.cfg.Quadlet && bytes.Contains(raw, []byte("StartLimitIntervalSec=0"))
					case "receipt":
						hit = path == u.policyBudgetPath() && bytes.Contains(raw, []byte(`"outcome":"migrated"`))
					case "state-clear":
						hit = path == u.statePath() && !bytes.Contains(raw, []byte(`"active"`)) && u.state.RunningSequence > 0
					case "outcome":
						hit = path == filepath.Join(u.cfg.StateDir, "engine_policy_outcome.json")
					}
					if hit && !fired {
						fired = true
						if afterWrite {
							if err := atomicFile(path, raw, mode); err != nil {
								return err
							}
						}
						return errors.New("injected I/O boundary failure")
					}
					return atomicFile(path, raw, mode)
				}
				err := u.execute("apply")
				if !fired {
					t.Fatal("write fault not reached")
				}
				if err == nil {
					t.Fatal("durability failure reported success")
				}
				u.write = nil
				if err := u.execute("recover"); err != nil {
					t.Fatal("local recovery after I/O failure:", err)
				}
				raw, _ := os.ReadFile(u.cfg.Quadlet)
				legacy := bytes.Equal(raw, legacyPolicyQuadlet())
				current := bytes.Contains(raw, []byte("StartLimitIntervalSec=0\n"))
				if !legacy && !current {
					t.Fatal("unknown postfault bytes")
				}
				if (legacy && h.effective.IntervalUS != 300000000) || (current && h.effective.IntervalUS != 0) || u.state.Active != nil || h.consoleRestarts != 0 {
					t.Fatal("incoherent fault recovery")
				}
			})
		}
	}
}

func TestPolicyCombinedOrphanHoldsAndOwnership(t *testing.T) {
	for _, point := range []string{"policy-reserved", "policy-held", "policy-console-held", "prepared", "console-pin", "console-restart"} {
		t.Run(point, func(t *testing.T) {
			u, h, key := setupPolicy(t)
			m := manifest(u)
			m.ConsoleDigest, m.ConsoleVersion, m.ConsoleBuild = newDigest, newVersion, 2
			queue(t, u, sign(m, key))
			u.checkpoint = func(at string) {
				if at == point {
					panic("power loss")
				}
			}
			func() { defer func() { _ = recover() }(); _ = u.execute("apply") }()
			u.checkpoint = nil
			if err := u.execute("recover"); err != nil {
				t.Fatal(err)
			}
			if len(h.anchors) != 0 || u.state.Active != nil || h.active.Digest != oldDigest || h.consoleActive.Digest != oldDigest {
				t.Fatal("paired hold/pin recovery incomplete", h.anchors)
			}
		})
	}
	// A label mismatch is not an invitation to delete an unrelated container.
	u, h, key := setupPolicy(t)
	queue(t, u, sign(samePolicyManifest(u), key))
	u.checkpoint = func(at string) {
		if at == "policy-held" {
			panic("power loss")
		}
	}
	func() { defer func() { _ = recover() }(); _ = u.execute("apply") }()
	u.checkpoint = nil
	for name := range h.owners {
		h.owners[name] = "somebody-else"
	}
	if err := u.execute("recover"); err == nil || len(h.anchors) != 1 {
		t.Fatal("unowned hold removed", err)
	}
}
