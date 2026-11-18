// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"
)

const policyRule = "engine-restart-policy-v1"
const policyCapability = "brrdfeeder-release recovery engine-restart-policy-v1 schema=1\n"

// Build 6 is the recovery bridge. Build 7 enables initiation in the following
// commit/release, after build 6 is the authenticated retained executable.
const policyInitiationEnabled = false

type effectivePolicy struct {
	IntervalUS uint64 `json:"interval_us"`
	Burst      uint64 `json:"burst"`
	Restart    string `json:"restart"`
	RestartUS  uint64 `json:"restart_us"`
	Source     string `json:"source"`
	Fragment   string `json:"fragment"`
	DropIns    string `json:"drop_ins"`
}

type policyFile struct {
	Hash string `json:"sha256"`
	UID  uint32 `json:"uid"`
	GID  uint32 `json:"gid"`
	Mode uint32 `json:"mode"`
}

// Quadlet in Transaction is the sole original-byte backup. The versioned
// extension is present only while an actual policy transaction is active.
type enginePolicyTransaction struct {
	Schema             int             `json:"schema"`
	Rule               string          `json:"rule"`
	ID                 string          `json:"id"`
	Before             policyFile      `json:"before"`
	After              []byte          `json:"after"`
	AfterHash          string          `json:"after_sha256"`
	Effective          effectivePolicy `json:"effective_before"`
	ConfigHash         string          `json:"config_sha256"`
	Engine             restartSample   `json:"engine_baseline"`
	Console            restartSample   `json:"console_baseline"`
	TemporaryHold      bool            `json:"temporary_hold"`
	RecoveryStarts     uint32          `json:"recovery_starts"`
	NextRecovery       time.Time       `json:"next_recovery"`
	RestoredInvocation string          `json:"restored_invocation,omitempty"`
}

// Paths/owner overrides and initiation override are unexported test seams.
// Production uses canonical installed paths and root ownership, never config
// input or an environment variable to weaken these checks.
type policyEnvironment struct {
	quadlet, config, binary, receipt, boundary string
	owner                                      uint32
	initiate                                   *bool
	executable                                 func() (string, error)
	probe                                      func(string) (string, error)
}

func (u *Updater) policyEnvironment() policyEnvironment {
	if u.policyTest != nil {
		return *u.policyTest
	}
	return policyEnvironment{quadlet: defaults().Quadlet, config: "/etc/brrdfeeder/config.yaml", binary: hostBinary,
		receipt: "/etc/brrdfeeder/.updater-helper.sha256", boundary: "/", executable: os.Executable}
}

func (u *Updater) policyEnabled() bool {
	if p := u.policyEnvironment(); p.initiate != nil {
		return *p.initiate
	}
	return policyInitiationEnabled
}

// Deliberately not a general systemd editor. Preserve every byte except the
// recognized interval value. Keeping burst=3 is harmless when interval=0.
func transformEnginePolicy(raw []byte) ([]byte, bool, error) {
	bad := errors.New("policy_refused_layout: unrecognized engine restart policy")
	if len(raw) == 0 || len(raw) > 65536 || bytes.ContainsAny(raw, "\r\x00") || !bytes.HasSuffix(raw, []byte("\n")) {
		return nil, false, bad
	}
	lines := strings.Split(string(raw), "\n")
	section := ""
	sections := map[string]bool{}
	values := map[string]string{}
	containerName := false
	intervalLine := -1
	for i, line := range lines {
		trim := strings.TrimSpace(line)
		if trim == "" || strings.HasPrefix(trim, "#") || strings.HasPrefix(trim, ";") {
			continue
		}
		if strings.HasSuffix(trim, "\\") {
			return nil, false, bad
		}
		if strings.HasPrefix(trim, "[") {
			if trim != line || !strings.HasSuffix(line, "]") {
				return nil, false, bad
			}
			section = strings.TrimSuffix(strings.TrimPrefix(line, "["), "]")
			if sections[section] || (section != "Unit" && section != "Container" && section != "Service" && section != "Install") {
				return nil, false, bad
			}
			sections[section] = true
			continue
		}
		key, value, ok := strings.Cut(line, "=")
		if !ok || section == "" || key != strings.TrimSpace(key) || key == "" {
			return nil, false, bad
		}
		if key == "Image" && section != "Container" {
			return nil, false, bad
		}
		if key == "ContainerName" {
			if section != "Container" || containerName || value != "brrdfeeder-engine" {
				return nil, false, bad
			}
			containerName = true
		}
		if strings.HasPrefix(key, "StartLimit") || strings.HasPrefix(key, "Restart") {
			if _, exists := values[key]; exists {
				return nil, false, bad
			}
			switch key {
			case "StartLimitIntervalSec":
				if section != "Unit" || (value != "300" && value != "0") {
					return nil, false, bad
				}
				intervalLine = i
			case "StartLimitBurst":
				if section != "Unit" || value != "3" {
					return nil, false, bad
				}
			case "Restart":
				if section != "Service" || value != "always" {
					return nil, false, bad
				}
			case "RestartSec":
				if section != "Service" || value != "5" {
					return nil, false, bad
				}
			default:
				return nil, false, bad
			}
			values[key] = value
		}
	}
	if !sections["Container"] || !containerName || intervalLine < 0 || values["Restart"] != "always" || values["RestartSec"] != "5" {
		return nil, false, bad
	}
	if _, err := previousPin(raw); err != nil {
		return nil, false, bad
	}
	if _, err := repin(raw, repository+"@"+strings.Repeat("0", 64)); err != nil {
		return nil, false, bad
	}
	if values["StartLimitIntervalSec"] == "0" {
		return append([]byte(nil), raw...), false, nil
	}
	if values["StartLimitBurst"] != "3" {
		return nil, false, bad
	}
	lines[intervalLine] = "StartLimitIntervalSec=0" // POLICY MUTATE transform
	return []byte(strings.Join(lines, "\n")), true, nil
}

func (u *Updater) readPolicyFile(path string, limit int64, mode os.FileMode) ([]byte, policyFile, error) {
	p := u.policyEnvironment()
	var meta policyFile
	// Reuse the helper's descriptor-based safe-parent/single-link reader.
	reader := localPaths{owner: p.owner, boundary: p.boundary}
	raw, err := reader.read(path, p.owner, 0022, limit)
	if err != nil {
		return nil, meta, err
	}
	f, err := os.OpenFile(path, os.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
	if err != nil {
		return nil, meta, err
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return nil, meta, err
	}
	s, ok := st.Sys().(*syscall.Stat_t)
	if !ok || !st.Mode().IsRegular() || s.Uid != p.owner || s.Gid != p.owner || s.Nlink != 1 || st.Mode().Perm() != mode || st.Mode()&(os.ModeSetuid|os.ModeSetgid|os.ModeSticky) != 0 {
		return nil, meta, errors.New("policy_refused_metadata")
	}
	checked, err := io.ReadAll(io.LimitReader(f, limit+1))
	if err != nil || !bytes.Equal(raw, checked) {
		return nil, meta, errors.New("policy_conflict: file changed during inspection")
	}
	meta = policyFile{contentHash(raw), s.Uid, s.Gid, uint32(st.Mode().Perm())}
	return raw, meta, nil
}

func parsePolicyDuration(value string) (uint64, error) {
	// systemctl's show serializer uses these exact representations for this
	// rule. No rounding, infinity, alternate unit files or generic expression.
	switch value {
	case "0", "0us":
		return 0, nil
	case "5min", "300s", "300000000us":
		return 300000000, nil
	case "5s", "5000ms", "5000000us":
		return 5000000, nil
	default:
		return 0, errors.New("policy_refused_effective: unsupported duration")
	}
}

func (u *Updater) observePolicy() (effectivePolicy, error) { return u.observePolicyActive(false) }

func (u *Updater) observePolicyActive(requireActive bool) (effectivePolicy, error) {
	var result effectivePolicy
	p := u.policyEnvironment()
	if u.cfg.Quadlet != p.quadlet || u.cfg.EngineUnit != "brrdfeeder-engine.service" || u.cfg.Container != "brrdfeeder-engine" {
		return result, errors.New("policy_refused_path")
	}
	properties := []string{"StartLimitIntervalUSec", "StartLimitBurst", "Restart", "RestartUSec", "SourcePath", "FragmentPath", "DropInPaths", "ActiveState", "MainPID"}
	args := []string{"show", u.cfg.EngineUnit}
	for _, property := range properties {
		args = append(args, "--property="+property)
	}
	raw, err := u.run.Run("systemctl", args...)
	if err != nil || len(raw) > 8192 {
		return result, errors.New("policy_pending_prerequisite: unit observation unavailable")
	}
	values := map[string]string{}
	for _, line := range strings.Split(strings.TrimSuffix(string(raw), "\n"), "\n") {
		key, value, ok := strings.Cut(line, "=")
		if _, duplicate := values[key]; !ok || duplicate {
			return result, errors.New("policy_refused_effective: malformed observation")
		}
		values[key] = value
	}
	if len(values) != len(properties) {
		return result, errors.New("policy_refused_effective: incomplete observation")
	}
	for _, property := range properties {
		if _, ok := values[property]; !ok {
			return result, errors.New("policy_refused_effective: missing property")
		}
	}
	result.IntervalUS, err = parsePolicyDuration(values["StartLimitIntervalUSec"])
	if err != nil {
		return result, err
	}
	result.RestartUS, err = parsePolicyDuration(values["RestartUSec"])
	if err != nil {
		return result, err
	}
	result.Burst, err = strconv.ParseUint(values["StartLimitBurst"], 10, 32)
	if err != nil {
		return result, errors.New("policy_refused_effective: burst unavailable")
	}
	result.Restart, result.Source, result.Fragment, result.DropIns = values["Restart"], values["SourcePath"], values["FragmentPath"], values["DropInPaths"]
	pid, err := strconv.ParseUint(values["MainPID"], 10, 32)
	if err != nil || (requireActive && (pid == 0 || values["ActiveState"] != "active")) {
		return result, errors.New("policy_pending_prerequisite: engine must already be active")
	}
	if result.Restart != "always" || result.RestartUS != 5000000 || result.Source != p.quadlet || result.Fragment != "/run/systemd/generator/brrdfeeder-engine.service" || result.DropIns != "" {
		return result, errors.New("policy_refused_effective: custom policy or unit source")
	}
	return result, nil
}

func boundedPolicyProbe(path string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, path, "recovery-capabilities")
	out := &boundedOutput{cancel: cancel}
	cmd.Stdout, cmd.Stderr, cmd.WaitDelay = out, io.Discard, time.Second
	err := cmd.Run()
	if err != nil || out.overflow || out.buf.Len() > 256 {
		return "", errors.New("policy recovery probe failed")
	}
	return out.buf.String(), nil
}

func (u *Updater) policyRecoveryReady() error {
	p := u.policyEnvironment()
	bad := errors.New("policy_pending_recovery_helper")
	reader := localPaths{owner: p.owner, boundary: p.boundary}
	for _, name := range []string{"host-transaction.json"} {
		if _, err := os.Lstat(filepath.Join(u.cfg.PrivateDir, name)); !os.IsNotExist(err) {
			return bad
		}
	}
	current, err := reader.read(p.binary, p.owner, 0022, maxHostBinary)
	if err != nil {
		return bad
	}
	receipt, err := reader.read(p.receipt, p.owner, 0077, 4096)
	if err != nil || string(receipt) != contentHash(current)+"  "+p.binary+"\n" {
		return bad
	}
	executable := p.executable
	if executable == nil {
		executable = os.Executable
	}
	self, err := executable()
	if err != nil {
		return bad
	}
	running, err := reader.read(self, p.owner, 0022, maxHostBinary)
	if err != nil || !bytes.Equal(running, current) {
		return bad
	}
	previous := p.binary + ".previous"
	old, err := reader.read(previous, p.owner, 0022, maxHostBinary)
	if err != nil {
		return bad
	}
	hash, err := reader.read(previous+".sha256", p.owner, 0077, 65)
	if err != nil || string(hash) != contentHash(old)+"\n" {
		return bad
	}
	// Hashes are corruption guards rooted in the existing pinned-install or
	// signed-host chain. Root-owned receipts do not authorize arbitrary downloads.
	probe := p.probe
	if probe == nil {
		probe = boundedPolicyProbe
	}
	for _, path := range []string{p.binary, previous} {
		answer, err := probe(path)
		if err != nil || answer != policyCapability {
			return bad
		}
	}
	// Detect replacement during probes, while the launcher/updater locks are held.
	for _, x := range []struct {
		path string
		raw  []byte
	}{{p.binary, current}, {previous, old}, {p.receipt, receipt}, {previous + ".sha256", hash}} {
		checked, err := reader.read(x.path, p.owner, 0022, maxHostBinary)
		if err != nil || !bytes.Equal(checked, x.raw) {
			return bad
		}
	}
	return nil
}

func policyID() (string, error) {
	var raw [16]byte
	_, err := rand.Read(raw[:])
	return hex.EncodeToString(raw[:]), err
}

func (u *Updater) validatePolicyTransaction(tx *Transaction) error {
	p := tx.EnginePolicy
	if p == nil {
		return nil
	}
	bad := errors.New("invalid engine policy recovery journal")
	if p.Schema != 1 || p.Rule != policyRule || len(p.ID) != 32 || p.Before.Hash != contentHash(tx.Quadlet) || p.AfterHash != contentHash(p.After) || len(p.After) > 65536 || p.Before.Mode != 0644 || p.Before.UID != u.policyEnvironment().owner || p.Before.GID != u.policyEnvironment().owner || p.RecoveryStarts > 2 || !imageIDRE.MatchString(p.ConfigHash) {
		return bad
	}
	if _, err := hex.DecodeString(p.ID); err != nil {
		return bad
	}
	if tx.Request.Release == nil || tx.Console == nil || !imageIDRE.MatchString(tx.PreviousID) || !targetValid(tx.Previous) || p.Engine.Image != tx.PreviousID || p.Console.Image != tx.Console.Previous.ID || len(p.Engine.Invocation) != 32 || len(p.Console.Invocation) != 32 {
		return bad
	}
	after, change, err := transformEnginePolicy(tx.Quadlet)
	if err != nil || !change {
		return bad
	}
	if tx.EngineChanged {
		after, err = repin(after, tx.Request.Target)
	}
	if err != nil || !bytes.Equal(after, p.After) || p.TemporaryHold == tx.EngineChanged || tx.Anchor != "brrdfeeder-policy-"+p.ID || p.Effective.IntervalUS != 300000000 || p.Effective.Burst != 3 || p.Effective.Restart != "always" || p.Effective.RestartUS != 5000000 || p.Effective.Source != u.policyEnvironment().quadlet || p.Effective.Fragment != "/run/systemd/generator/brrdfeeder-engine.service" || p.Effective.DropIns != "" {
		return bad
	}
	return nil
}

func (u *Updater) policyConfigHash() (string, error) {
	p := u.policyEnvironment()
	reader := localPaths{owner: p.owner, boundary: p.boundary}
	raw, err := reader.read(p.config, p.owner, 0022, maxDocument)
	if err != nil {
		return "", fmt.Errorf("policy_pending_prerequisite: config integrity unavailable: %w", err)
	}
	return contentHash(raw), nil
}

// A helper-only result is read-only and never claims a completed migration from
// a receipt. Full eligibility (signed request, health, holds and budget) is still
// checked by the later workload poll.
func (u *Updater) localPolicyStatus() string {
	raw, _, err := u.readPolicyFile(u.cfg.Quadlet, 65536, 0644)
	if err != nil {
		return "policy_pending_prerequisite"
	}
	_, changed, err := transformEnginePolicy(raw)
	if err != nil {
		return "policy_refused_source"
	}
	effective, err := u.observePolicy()
	if err != nil {
		return "policy_refused_effective"
	}
	if !changed {
		if effective.IntervalUS == 0 {
			return "policy_current"
		}
		return "policy_refused_effective"
	}
	if effective.IntervalUS != 300000000 || effective.Burst != 3 {
		return "policy_refused_effective"
	}
	if !u.policyEnabled() {
		return "policy_pending_migration_helper"
	}
	if err := u.policyRecoveryReady(); err != nil {
		return "policy_pending_recovery_helper"
	}
	return "policy_pending_next_eligible_poll"
}
