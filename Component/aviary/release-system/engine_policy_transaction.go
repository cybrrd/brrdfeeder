// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

type policyBudget struct {
	ConsoleHold  string    `json:"console_hold,omitempty"`
	ConsoleImage string    `json:"console_image,omitempty"`
	Schema       int       `json:"schema"`
	Rule         string    `json:"rule"`
	Used         bool      `json:"used"`
	ID           string    `json:"transaction_id"`
	Hold         string    `json:"hold,omitempty"`
	Image        string    `json:"image,omitempty"`
	Outcome      string    `json:"outcome"`
	Updated      time.Time `json:"updated_at"`
}

func (u *Updater) policyBudgetPath() string {
	return filepath.Join(u.cfg.PrivateDir, "engine-policy-budget.json")
}

func (u *Updater) readPolicyBudget() (policyBudget, error) {
	b := policyBudget{Schema: 1, Rule: policyRule}
	if _, err := os.Lstat(u.policyBudgetPath()); os.IsNotExist(err) {
		return b, nil
	}
	p := u.policyEnvironment()
	reader := localPaths{owner: p.owner, boundary: p.boundary}
	raw, err := reader.read(u.policyBudgetPath(), p.owner, 0077, 4096)
	if os.IsNotExist(err) {
		return b, nil
	}
	if err != nil {
		return b, err
	}
	if strictJSON(raw, &b) != nil || b.Schema != 1 || b.Rule != policyRule || (b.Used && len(b.ID) != 32) || (b.Hold != "" && (b.Hold != "brrdfeeder-policy-"+b.ID || !imageIDRE.MatchString(b.Image))) || (b.ConsoleHold != "" && (b.ConsoleHold != "brrdhouse-policy-"+b.ID || !imageIDRE.MatchString(b.ConsoleImage))) {
		return b, errors.New("policy_refused_budget")
	}
	if b.ID != "" {
		if _, err := hex.DecodeString(b.ID); err != nil || len(b.ID) != 32 {
			return b, errors.New("policy_refused_budget")
		}
	}
	return b, nil
}

func (u *Updater) writePolicyBudget(b policyBudget) error {
	b.Updated = u.now().UTC()
	raw, err := json.Marshal(b)
	if err != nil || len(raw) > 4096 {
		return errors.New("policy receipt exceeds bound")
	}
	return u.writeFile(u.policyBudgetPath(), raw, 0600)
}

// Bounded public projection: no unit/config bytes, executable paths, error text,
// environment, or credentials. A current observation is not a migration receipt.
func (u *Updater) policyOutcome(code string) error {
	projection := map[string]any{"schema": 1, "rule": policyRule, "outcome": code, "observed_at": u.now().UTC()}
	if effective, err := u.observePolicy(); err == nil {
		projection["effective"] = map[string]any{"interval_us": effective.IntervalUS, "burst": effective.Burst, "restart": effective.Restart, "restart_us": effective.RestartUS}
	}
	raw, err := json.Marshal(projection)
	if err != nil {
		return err
	}
	u.state.Outcome = code
	if err = u.save(); err != nil {
		return err
	}
	return u.writeFile(filepath.Join(u.cfg.StateDir, "engine_policy_outcome.json"), raw, 0644)
}

func (u *Updater) policyRefusal(err error) error {
	code := strings.SplitN(err.Error(), ":", 2)[0]
	if !strings.HasPrefix(code, "policy_") {
		code = "policy_pending_prerequisite"
	}
	return errors.Join(err, u.policyOutcome(code))
}

type workloadPlan struct {
	EngineImageChanged, ConsoleImageChanged, EnginePolicyChanged, EngineRestartRequired bool
	Engine                                                                              []byte
}

func planWorkload(quad []byte, engine, console Image, m Release, reconcile bool) (workloadPlan, error) {
	p := workloadPlan{EngineImageChanged: !contains(engine.RepoDigests, repository+"@"+m.Digest), ConsoleImageChanged: !contains(console.RepoDigests, consoleRepository+"@"+m.ConsoleDigest), Engine: append([]byte(nil), quad...)}
	var err error
	if reconcile {
		p.Engine, p.EnginePolicyChanged, err = transformEnginePolicy(p.Engine)
	}
	if err == nil && p.EngineImageChanged {
		p.Engine, err = repin(p.Engine, repository+"@"+m.Digest)
	}
	p.EngineRestartRequired = p.EngineImageChanged || p.EnginePolicyChanged
	return p, err
}

func (u *Updater) planEnginePolicy(quad []byte, req Request, engine, console Image) (*enginePolicyTransaction, error) {
	if !u.policyEnabled() {
		return nil, nil
	}
	plan, err := planWorkload(quad, engine, console, *req.Release, true)
	if err != nil {
		return nil, err
	}
	raw, meta, err := u.readPolicyFile(u.cfg.Quadlet, 65536, 0644)
	if err != nil {
		return nil, err
	}
	if !bytes.Equal(raw, quad) {
		return nil, errors.New("policy_conflict: source changed")
	}
	effective, err := u.observePolicyActive(true)
	if err != nil {
		return nil, err
	}
	if !plan.EnginePolicyChanged {
		if effective.IntervalUS != 0 {
			return nil, errors.New("policy_refused_effective: current file is not effective")
		}
		return nil, nil
	}
	if effective.IntervalUS != 300000000 || effective.Burst != 3 {
		return nil, errors.New("policy_refused_effective: legacy file is not effective")
	}
	if err = u.policyRecoveryReady(); err != nil {
		return nil, err
	}
	b, err := u.readPolicyBudget()
	if err != nil {
		return nil, err
	}
	if b.Used {
		return nil, errors.New("policy_pending_retry_authorization")
	}
	if b.Hold != "" || b.ConsoleHold != "" {
		return nil, errors.New("policy_pending_hold_cleanup")
	}
	if err = u.resourceCheck(); err != nil {
		return nil, err
	}
	if err = u.beginWatch(); err != nil {
		return nil, errors.New("policy_pending_prerequisite: baseline inactive")
	}
	baseline := *u.watch
	s, err := u.status()
	if err != nil || baseline[0].Image != engine.ID || baseline[1].Image != console.ID || s.Heartbeat.Trusted == nil || !*s.Heartbeat.Trusted || !u.healthy(engine.Digest, s.Heartbeat.Version, s.Heartbeat.Build, u.now()) || !u.consoleHealthy(engine.Digest, s.Heartbeat.Version, s.Written) {
		return nil, errors.New("policy_pending_prerequisite: healthy stable baseline required")
	}
	configHash, err := u.policyConfigHash()
	if err != nil {
		return nil, err
	}
	id, err := policyID()
	if err != nil {
		return nil, err
	}
	return &enginePolicyTransaction{Schema: 1, Rule: policyRule, ID: id, Before: meta, After: plan.Engine, AfterHash: contentHash(plan.Engine), Effective: effective, ConfigHash: configHash, Engine: baseline[0], Console: baseline[1], TemporaryHold: !plan.EngineImageChanged}, nil
}

func (u *Updater) checkPolicyInputs(tx *Transaction) error {
	p := tx.EnginePolicy
	if p == nil {
		return nil
	}
	raw, meta, err := u.readPolicyFile(u.cfg.Quadlet, 65536, 0644)
	if err != nil || meta != p.Before || !bytes.Equal(raw, tx.Quadlet) {
		return errors.New("policy_conflict: engine file changed before mutation")
	}
	effective, err := u.observePolicyActive(true)
	if err != nil || effective != p.Effective {
		return errors.New("policy_conflict: effective policy changed before mutation")
	}
	config, err := u.policyConfigHash()
	if err != nil || config != p.ConfigHash {
		return errors.New("policy_conflict: config changed before mutation")
	}
	for i, console := range []bool{false, true} {
		sample, err := u.restartSample(console)
		if err != nil || sample != []restartSample{p.Engine, p.Console}[i] {
			return errors.New("policy_conflict: running baseline changed before mutation")
		}
	}
	return u.policyRecoveryReady()
}

func (u *Updater) reservePolicy(tx *Transaction) error {
	// Reject a deterministic oversized journal before consuming an attempt or
	// creating holds. save() independently enforces the same bound on every write.
	prospective := u.state
	prospective.Active = tx
	raw, err := json.Marshal(prospective)
	if err != nil || len(raw) > 256*1024 {
		return errors.New("policy_refused_journal_size")
	}
	p := tx.EnginePolicy
	if err := u.checkPolicyInputs(tx); err != nil {
		return err
	}
	budget, err := u.readPolicyBudget()
	if err != nil {
		return err
	}
	if budget.Used || budget.Hold != "" || budget.ConsoleHold != "" {
		return errors.New("policy_pending_retry_authorization")
	}
	// This reservation survives crashes even before the active journal. It
	// authorizes no unit mutation, but identifies any orphaned temporary hold.
	budget.Used, budget.ID, budget.Hold, budget.Image, budget.Outcome = true, p.ID, tx.Anchor, tx.PreviousID, "reserved"
	if tx.Console.Changed {
		budget.ConsoleHold, budget.ConsoleImage = tx.Console.Anchor, tx.Console.Previous.ID
	}
	if err = u.writePolicyBudget(budget); err != nil {
		return err
	}
	u.point("policy-reserved")
	if err := u.createPolicyHold(false, tx.Anchor, tx.PreviousID, p.ID); err != nil {
		return err
	}
	u.point("policy-held")
	if tx.Console.Changed {
		if err := u.createPolicyHold(true, tx.Console.Anchor, tx.Console.Previous.ID, p.ID); err != nil {
			return err
		}
		u.point("policy-console-held")
	}
	return nil
}

func (u *Updater) createPolicyHold(console bool, name, image, id string) error {
	args := []string{"run", "-d", "--name", name, "--label", "io.cybrrd.policy-transaction=" + id, "--network=none", "--user=65532:65532", "--cap-drop=all", "--security-opt=no-new-privileges", "--read-only", "--read-only-tmpfs=false", "--restart=always", "-v", hostBinary + ":/hold:ro", "--entrypoint=/hold", image, "hold"}
	if _, err := u.componentRun(console, "podman", args...); err != nil {
		return err
	}
	return u.checkPolicyHold(console, name, image, id, true)
}

func (u *Updater) checkPolicyHold(console bool, name, image, id string, running bool) error {
	raw, err := u.componentRun(console, "podman", "inspect", "--type", "container", name)
	var rows []struct {
		Image  string
		Config struct{ Labels map[string]string }
		State  struct{ Running bool }
	}
	if err != nil || json.Unmarshal(raw, &rows) != nil || len(rows) != 1 || strings.TrimPrefix(rows[0].Image, "sha256:") != image || (running && !rows[0].State.Running) || rows[0].Config.Labels["io.cybrrd.policy-transaction"] != id {
		return errors.New("policy_conflict: hold identity unavailable")
	}
	return nil
}

func (u *Updater) checkPolicyHolds(tx *Transaction) error {
	if err := u.checkPolicyHold(false, tx.Anchor, tx.PreviousID, tx.EnginePolicy.ID, true); err != nil {
		return err
	}
	if tx.Console.Changed {
		return u.checkPolicyHold(true, tx.Console.Anchor, tx.Console.Previous.ID, tx.EnginePolicy.ID, true)
	}
	return nil
}

func (u *Updater) cleanupPolicyHold(b *policyBudget, console bool) error {
	name, image := b.Hold, b.Image
	if console {
		name, image = b.ConsoleHold, b.ConsoleImage
	}
	if name == "" {
		return nil
	}
	// Listing distinguishes an already-removed hold from failed inspection.
	raw, err := u.componentRun(console, "podman", "ps", "--all", "--filter", "name=^"+name+"$", "--format", "{{.Names}}")
	if err != nil {
		return err
	}
	if strings.TrimSpace(string(raw)) != "" {
		if strings.TrimSpace(string(raw)) != name {
			return errors.New("policy_conflict: hold name mismatch")
		}
		if err = u.checkPolicyHold(console, name, image, b.ID, false); err != nil {
			return err
		}
		if _, err = u.componentRun(console, "podman", "rm", "--ignore", "-f", name); err != nil {
			return err
		}
	}
	u.point("policy-hold-removed")
	if console {
		b.ConsoleHold, b.ConsoleImage = "", ""
	} else {
		b.Hold, b.Image = "", ""
	}
	return u.writePolicyBudget(*b)
}

func (u *Updater) cleanupOrphanPolicyHold() error {
	b, err := u.readPolicyBudget()
	if err != nil {
		return err
	}
	for _, console := range []bool{false, true} {
		name, image, anchor := b.Hold, b.Image, u.state.Anchor
		if console {
			name, image, anchor = b.ConsoleHold, b.ConsoleImage, u.state.ConsoleAnchor
		}
		if name == "" {
			continue
		}
		// Adopted package anchors are historical, never temporary cleanup targets.
		if name == anchor {
			if console {
				b.ConsoleHold, b.ConsoleImage = "", ""
			} else {
				b.Hold, b.Image = "", ""
			}
			if err = u.writePolicyBudget(b); err != nil {
				return err
			}
			continue
		}
		var im Image
		if console {
			im, err = u.consoleCurrent()
		} else {
			im, err = u.current()
		}
		if err != nil || im.ID != image {
			return errors.New("policy_pending_hold_cleanup: original image not running")
		}
		if err = u.cleanupPolicyHold(&b, console); err != nil {
			return err
		}
	}
	return nil
}

func (u *Updater) policyReload(want effectivePolicy) error {
	if _, err := u.run.Run("systemctl", "daemon-reload"); err != nil {
		return err
	}
	u.point("policy-reloaded")
	actual, err := u.observePolicy()
	if err != nil || actual != want {
		return errors.New("policy effective readback mismatch")
	}
	u.point("policy-readback")
	return nil
}

func (u *Updater) restartPolicyEngine() error {
	if _, err := u.run.Run("systemctl", "reset-failed", u.cfg.EngineUnit); err != nil {
		return err
	}
	u.point("policy-counter-reset")
	_, err := u.run.Run("systemctl", "restart", u.cfg.EngineUnit)
	return err
}

func (u *Updater) policyTarget(tx *Transaction) effectivePolicy {
	want := tx.EnginePolicy.Effective
	want.IntervalUS = 0
	return want
}

func (u *Updater) policyPostcheck(tx *Transaction) error {
	p := tx.EnginePolicy
	raw, meta, err := u.readPolicyFile(u.cfg.Quadlet, 65536, 0644)
	if err != nil || meta.UID != p.Before.UID || meta.GID != p.Before.GID || meta.Mode != p.Before.Mode || !bytes.Equal(raw, p.After) {
		return errors.New("policy_conflict: postimage changed")
	}
	actual, err := u.observePolicyActive(true)
	if err != nil || actual != u.policyTarget(tx) {
		return errors.New("policy effective target mismatch")
	}
	config, err := u.policyConfigHash()
	if err != nil || config != p.ConfigHash {
		return errors.New("policy_conflict: config changed during transaction")
	}
	return nil
}

func (u *Updater) commitEnginePolicy(tx *Transaction) error {
	if err := u.validatePolicyTransaction(tx); err != nil {
		return err
	}
	if err := u.policyPostcheck(tx); err != nil {
		return err
	}
	s, err := u.status()
	if err != nil || s.Heartbeat.Digest != tx.Request.Release.Digest || s.Heartbeat.Version != tx.Request.Version || s.Heartbeat.Build != tx.Request.Build || s.Heartbeat.Radio != "up" || s.Heartbeat.Trusted == nil || !*s.Heartbeat.Trusted {
		return errors.New("policy commit awaiting verified running engine")
	}
	b, err := u.readPolicyBudget()
	if err != nil || !b.Used || b.ID != tx.EnginePolicy.ID {
		return errors.New("policy commit budget mismatch")
	}
	b.Outcome = "migrated"
	if err = u.writePolicyBudget(b); err != nil {
		return err
	}
	u.point("policy-receipted")
	if tx.EnginePolicy.TemporaryHold {
		im, err := u.current()
		if err != nil || im.ID != tx.PreviousID {
			return errors.New("policy cleanup awaiting verified current image")
		}
		if err = u.cleanupPolicyHold(&b, false); err != nil {
			return err
		}
	}
	return u.policyOutcome("policy_migrated")
}

func (u *Updater) policyRecoveryWait(tx *Transaction, cause error) error {
	tx.Phase = "recovery_wait"
	tx.EnginePolicy.NextRecovery = u.now().Add(time.Minute)
	if err := u.save(); err != nil {
		return err
	}
	return errors.Join(fmt.Errorf("policy_recovery_wait: %w", cause), u.policyOutcome("policy_recovery_wait"))
}

func (u *Updater) recoverEnginePolicy(boot bool) error {
	tx := u.state.Active
	if err := u.validatePolicyTransaction(tx); err != nil {
		return err
	}
	if tx.Phase == "confirmed" {
		return u.commit()
	}
	p := tx.EnginePolicy
	budget, err := u.readPolicyBudget()
	if err != nil || !budget.Used || budget.ID != p.ID {
		return errors.New("policy recovery budget mismatch")
	}
	raw, meta, err := u.readPolicyFile(u.cfg.Quadlet, 65536, 0644)
	if err != nil || meta.UID != p.Before.UID || meta.GID != p.Before.GID || meta.Mode != p.Before.Mode || (contentHash(raw) != p.Before.Hash && contentHash(raw) != p.AfterHash) {
		return u.policyRefusal(errors.New("policy_conflict: preserve third-party engine edit"))
	}
	config, err := u.policyConfigHash()
	if err != nil || config != p.ConfigHash {
		return u.policyRefusal(errors.New("policy_conflict: preserve changed config"))
	}
	currentPolicy, err := u.observePolicy()
	if err != nil {
		return u.policyRefusal(errors.New("policy_conflict: unit source or overrides changed"))
	}
	if currentPolicy != p.Effective && currentPolicy != u.policyTarget(tx) {
		return u.policyRefusal(errors.New("policy_conflict: unknown effective policy"))
	}
	combined := tx.EngineChanged || tx.Console.Changed
	cq, err := readBounded(u.cfg.ConsoleQuadlet, 65536)
	if err != nil {
		return err
	}
	consoleAfter, err := repin(tx.Console.Quadlet, consoleRepository+"@"+tx.Request.Release.ConsoleDigest)
	if err != nil || (!bytes.Equal(cq, tx.Console.Quadlet) && (!combined || !bytes.Equal(cq, consoleAfter))) {
		return u.policyRefusal(errors.New("policy_conflict: preserve third-party console edit"))
	}
	priorPhase := tx.Phase
	tx.Phase = "rollback"
	if err = u.save(); err != nil {
		return err
	}
	u.point("rollback-journal")
	budget.Outcome = "failed"
	if err = u.writePolicyBudget(budget); err != nil {
		return err
	}
	if !bytes.Equal(raw, tx.Quadlet) {
		if err = u.writeFile(u.cfg.Quadlet, tx.Quadlet, os.FileMode(p.Before.Mode)); err != nil {
			return u.policyRecoveryWait(tx, err)
		}
		u.point("rollback-engine-pin")
	}
	// Always reload/read back on recovery. Same image ID is never proof of
	// original effective policy, even in recovery_wait or after a reboot.
	if err = u.policyReload(p.Effective); err != nil {
		return u.policyRecoveryWait(tx, err)
	}
	engine, engineErr := u.restartSample(false)
	console, consoleErr := u.restartSample(true)
	engineRestored := engineErr == nil && engine.Image == tx.PreviousID && ((engine == p.Engine && priorPhase == "prepared") || (p.RestoredInvocation != "" && engine.Invocation == p.RestoredInvocation && engine.UnitRestarts == 0 && engine.ContainerRestarts == 0))
	// An interruption before the first file write also has a conclusive untouched
	// baseline. Rechecking the full sample (not just the image) makes this safe.
	if bytes.Equal(raw, tx.Quadlet) && currentPolicy == p.Effective && engineErr == nil && engine == p.Engine {
		engineRestored = true
	}
	consoleRestored := consoleErr == nil && console.Image == tx.Console.Previous.ID && bytes.Equal(cq, tx.Console.Quadlet)
	if !combined && (!consoleRestored || (!boot && console != p.Console)) {
		return u.policyRecoveryWait(tx, errors.New("policy-only console baseline changed"))
	}
	if !engineRestored || (combined && !consoleRestored) {
		if p.RecoveryStarts >= 2 || u.now().Before(p.NextRecovery) {
			return u.policyRecoveryWait(tx, errors.New("bounded recovery start budget/backoff; operator authorization required after two starts"))
		}
		p.RecoveryStarts++
		p.NextRecovery = u.now().Add(time.Minute)
		if err = u.save(); err != nil {
			return err
		}
		u.point("policy-recovery-reserved")
		if combined && !consoleRestored {
			if err = u.writeFile(u.cfg.ConsoleQuadlet, tx.Console.Quadlet, 0644); err != nil {
				return u.policyRecoveryWait(tx, err)
			}
			u.point("rollback-console-pin")
			if err = u.consoleRestart(); err != nil {
				return u.policyRecoveryWait(tx, err)
			}
			u.point("rollback-console-restart")
		}
		if !engineRestored {
			if err = u.restartPolicyEngine(); err != nil {
				return u.policyRecoveryWait(tx, err)
			}
			u.point("rollback-engine-restart")
			engine, err = u.restartSample(false)
			if err != nil || engine.Image != tx.PreviousID || engine.UnitRestarts != 0 || engine.ContainerRestarts != 0 {
				return u.policyRecoveryWait(tx, errors.New("restored engine unstable"))
			}
		}
	}
	p.RestoredInvocation = engine.Invocation
	if err = u.save(); err != nil {
		return err
	}
	if err = u.beginWatch(); err != nil {
		return u.policyRecoveryWait(tx, err)
	}
	since := u.now()
	if !u.healthy(tx.PreviousDigest, tx.PreviousVersion, tx.PreviousBuild, since) || !u.consoleHealthy(tx.PreviousDigest, tx.PreviousVersion, since) || !u.stableWindow() {
		return u.policyRecoveryWait(tx, errors.New("original readiness not recovered"))
	}
	im, ie := u.current()
	ci, ce := u.consoleCurrent()
	effective, ee := u.observePolicyActive(true)
	restored, rm, re := u.readPolicyFile(u.cfg.Quadlet, 65536, 0644)
	config, configErr := u.policyConfigHash()
	if configErr != nil || config != p.ConfigHash || ie != nil || ce != nil || im.ID != tx.PreviousID || ci.ID != tx.Console.Previous.ID || ee != nil || effective != p.Effective || re != nil || rm != p.Before || !bytes.Equal(restored, tx.Quadlet) {
		return u.policyRecoveryWait(tx, errors.New("original identity/file/effective policy not restored"))
	}
	if err = u.cleanupPolicyHold(&budget, false); err != nil {
		return u.policyRecoveryWait(tx, err)
	}
	if err = u.cleanupPolicyHold(&budget, true); err != nil {
		return u.policyRecoveryWait(tx, err)
	}
	if err = u.clearPending(tx.Request.Hash); err != nil {
		return err
	}
	if err = u.policyOutcome("policy_rolled_back"); err != nil {
		return err
	}
	u.state.Active = nil
	if err = u.save(); err != nil {
		return err
	}
	u.point("rollback-complete")
	return removeOptional(u.marker())
}

// Explicit local root authorization changes only the bounded attempt record.
// It neither mutates/restarts workloads nor waives any subsequent validation.
func (u *Updater) authorizePolicyRetry() error {
	if _, err := os.Lstat(filepath.Join(u.cfg.PrivateDir, "host-transaction.json")); !os.IsNotExist(err) {
		return errors.New("unresolved host transaction")
	}
	if tx := u.state.Active; tx != nil {
		if tx.EnginePolicy == nil || tx.Phase != "recovery_wait" {
			return errors.New("policy retry requires no transaction or policy recovery_wait")
		}
		if err := u.validatePolicyTransaction(tx); err != nil {
			return err
		}
		tx.EnginePolicy.RecoveryStarts = 0
		tx.EnginePolicy.NextRecovery = time.Time{}
		return u.policyOutcome("policy_recovery_retry_authorized")
	}
	if err := u.cleanupOrphanPolicyHold(); err != nil {
		return err
	}
	b, err := u.readPolicyBudget()
	if err != nil {
		return err
	}
	b.Used = false
	b.Outcome = "retry_authorized"
	if err = u.writePolicyBudget(b); err != nil {
		return err
	}
	return u.policyOutcome("policy_retry_authorized")
}
