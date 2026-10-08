// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"syscall"
	"time"
)

type Config struct {
	Node           string `json:"node_id"`
	Ring           string `json:"ring"`
	StatusFile     string `json:"status_file"`
	ConsoleQuadlet string `json:"console_quadlet"`
	ConsoleUID     uint32 `json:"console_uid"`
	ConsoleUser    string `json:"console_user"`
	ConsoleURL     string `json:"console_url"`
	ConsoleBuild   uint64 `json:"console_build_seq"`
	GPSRequired    bool   `json:"gps_required"`
	BaseURL        string `json:"base_url"`
	StateDir       string `json:"state_dir"`
	PrivateDir     string `json:"private_dir"`
	Quadlet        string `json:"quadlet"`
	EngineUnit     string `json:"engine_unit"`
	Container      string `json:"engine_container"`
}

func defaults() Config {
	return Config{Ring: "general", StatusFile: "/var/lib/brrdfeeder-status/status.json", ConsoleQuadlet: "/etc/brrdfeeder/brrdhouse.container", ConsoleUser: "brrdhouse", BaseURL: "https://get.cybrrd.com/releases/v1", StateDir: "/var/lib/brrdfeeder", PrivateDir: "/var/lib/brrdfeeder-updater", Quadlet: "/etc/containers/systemd/brrdfeeder-engine.container", EngineUnit: "brrdfeeder-engine.service", Container: "brrdfeeder-engine"}
}

type Request struct {
	Target  string   `json:"target"`
	Version string   `json:"version"`
	Build   uint64   `json:"build_seq"`
	Issued  int64    `json:"issued_ms"`
	Hash    string   `json:"hash"`
	Release *Release `json:"release,omitempty"`
}
type Transaction struct {
	FailureCounted  bool        `json:"failure_counted,omitempty"`
	Request         Request     `json:"request"`
	Previous        string      `json:"previous"`
	PreviousID      string      `json:"previous_id"`
	PreviousDigest  string      `json:"previous_digest"`
	PreviousBuild   uint64      `json:"previous_build"`
	PreviousVersion string      `json:"previous_version"`
	Quadlet         []byte      `json:"quadlet"`
	Anchor          string      `json:"anchor"`
	Phase           string      `json:"phase"`
	Started         time.Time   `json:"started"`
	EngineChanged   bool        `json:"engine_changed"`
	Console         *Checkpoint `json:"console,omitempty"`
}
type Checkpoint struct {
	Quadlet  []byte `json:"quadlet"`
	Previous Image  `json:"previous"`
	Next     Image  `json:"next"`
	Anchor   string `json:"anchor"`
	Changed  bool   `json:"changed"`
}
type Seen struct {
	Sequence uint64 `json:"sequence"`
	Hash     string `json:"hash"`
}
type State struct {
	NextPoll            time.Time       `json:"next_poll"`
	Seen                Seen            `json:"seen"`
	AppliedBuild        uint64          `json:"applied_build"`
	AppliedIssued       int64           `json:"applied_issued"`
	Attempts            map[string]int  `json:"attempts"`
	Quarantined         map[string]bool `json:"quarantined"`
	Active              *Transaction    `json:"active,omitempty"`
	Anchor              string          `json:"anchor"`
	ConsoleAnchor       string          `json:"console_anchor"`
	AppliedConsoleBuild uint64          `json:"applied_console_build"`
	RunningSequence     uint64          `json:"running_sequence"`
	LastReleaseCheck    time.Time       `json:"last_release_check_at"`
	Outcome             string          `json:"update_outcome"`
	RunningEngineDigest string          `json:"running_engine_digest"`
}

func readBounded(path string, max int64) ([]byte, error) {
	f, e := os.OpenFile(path, os.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
	if e != nil {
		return nil, e
	}
	defer f.Close()
	st, e := f.Stat()
	if e != nil {
		return nil, e
	}
	if !st.Mode().IsRegular() || st.Size() > max {
		return nil, errors.New("invalid file")
	}
	b, e := io.ReadAll(io.LimitReader(f, max+1))
	if int64(len(b)) > max {
		return nil, errors.New("file too large")
	}
	return b, e
}
func atomicFile(path string, raw []byte, mode os.FileMode) error {
	dir := filepath.Dir(path)
	f, e := os.CreateTemp(dir, ".update-*")
	if e != nil {
		return e
	}
	tmp := f.Name()
	defer os.Remove(tmp)
	if e = f.Chmod(mode); e == nil {
		_, e = f.Write(raw)
	}
	if e == nil {
		e = f.Sync()
	}
	ce := f.Close()
	if e != nil {
		return e
	}
	if ce != nil {
		return ce
	}
	if e = os.Rename(tmp, path); e != nil {
		return e
	}
	d, e := os.Open(dir)
	if e != nil {
		return e
	}
	defer d.Close()
	return d.Sync()
}
func atomicJSON(path string, v any, mode os.FileMode) error {
	b, e := json.Marshal(v)
	if e != nil {
		return e
	}
	return atomicFile(path, b, mode)
}
func (u *Updater) writeFile(path string, raw []byte, mode os.FileMode) error {
	if u.write != nil {
		return u.write(path, raw, mode)
	}
	return atomicFile(path, raw, mode)
}
func (u *Updater) statePath() string { return filepath.Join(u.cfg.PrivateDir, "state.json") }
func (u *Updater) pending() string   { return filepath.Join(u.cfg.PrivateDir, "pending_update.json") }
func (u *Updater) marker() string    { return filepath.Join(u.cfg.StateDir, "update_transaction.json") }
func (u *Updater) load() error {
	u.state = State{Attempts: map[string]int{}, Quarantined: map[string]bool{}}
	b, e := readBounded(u.statePath(), 256*1024)
	if os.IsNotExist(e) {
		// Bootstrap the new host-owned anti-replay floor from the existing Blue
		// watermark; never turn an already-applied signed rollback into a new one.
		if legacy, le := readBounded(filepath.Join(u.cfg.StateDir, "policy_state.json"), maxDocument); le == nil {
			var p struct {
				Issued int64  `json:"last_applied_unix_ms"`
				Build  uint64 `json:"last_applied_build_seq"`
			}
			if le = strictJSON(legacy, &p); le != nil {
				return le
			}
			u.state.AppliedBuild = p.Build
			u.state.AppliedIssued = p.Issued
		} else if !os.IsNotExist(le) {
			return le
		}
		return nil
	}
	if e != nil {
		return e
	}
	if e = strictJSONLimit(b, &u.state, 256*1024); e != nil {
		return e
	}
	if u.state.Attempts == nil || u.state.Quarantined == nil {
		return errors.New("invalid durable update state")
	}
	return nil
}
func (u *Updater) save() error {
	b, e := json.Marshal(u.state)
	if e != nil {
		return e
	}
	if len(b) > 256*1024 {
		return errors.New("update journal exceeds bound")
	}
	if e = u.writeFile(u.statePath(), b, 0600); e != nil {
		return e
	}
	var checked *time.Time
	if !u.state.LastReleaseCheck.IsZero() {
		checked = &u.state.LastReleaseCheck
	}
	return atomicJSON(filepath.Join(u.cfg.StateDir, "release_currency.json"), map[string]any{
		"node_id": u.cfg.Node, "last_release_check_at": checked,
		"release_seq_seen": u.state.Seen.Sequence, "running_release_seq": u.state.RunningSequence,
		"engine_digest": u.state.RunningEngineDigest, "update_outcome": u.state.Outcome,
	}, 0644)
}
func (u *Updater) lock() (func(), error) {
	if e := os.MkdirAll(u.cfg.PrivateDir, 0700); e != nil {
		return nil, e
	}
	st, e := os.Lstat(u.cfg.PrivateDir)
	if e != nil {
		return nil, e
	}
	if !st.IsDir() || st.Mode().Perm()&0077 != 0 || st.Sys().(*syscall.Stat_t).Uid != uint32(os.Geteuid()) {
		return nil, errors.New("private updater state must be owner-only directory")
	}
	f, e := os.OpenFile(filepath.Join(u.cfg.PrivateDir, "lock"), os.O_CREATE|os.O_RDWR|syscall.O_NOFOLLOW, 0600)
	if e != nil {
		return nil, e
	}
	if e = syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); e != nil {
		f.Close()
		return nil, e
	}
	return func() { f.Close() }, nil
}

// Link instead of overwrite: single-slot signed-release mailbox. Blue is retired.
func stage(path string, raw []byte) error {
	f, e := os.CreateTemp(filepath.Dir(path), ".pending-*")
	if e != nil {
		return e
	}
	defer os.Remove(f.Name())
	if _, e = f.Write(raw); e == nil {
		e = f.Sync()
	}
	if closeErr := f.Close(); e == nil {
		e = closeErr
	}
	if e != nil {
		return e
	}
	if e = os.Link(f.Name(), path); e != nil {
		return e
	}
	d, e := os.Open(filepath.Dir(path))
	if e != nil {
		return e
	}
	defer d.Close()
	return d.Sync()
}
