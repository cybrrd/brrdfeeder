// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"errors"
	"path/filepath"
	"time"
)

// Separate files preserve compatibility with older helpers' strict State JSON.
// Never persist command stderr, response bodies, URLs or transport credentials.
type Attempt struct {
	Schema           int       `json:"schema_version"`
	Mode             string    `json:"mode"`
	Started          time.Time `json:"started_at"`
	Finished         time.Time `json:"finished_at"`
	Phase            string    `json:"phase"`
	ErrorCode        string    `json:"error_code"`
	NetworkAttempted bool      `json:"network_attempted"`
	HTTPStatus       int       `json:"http_status"`
	HelperBuild      string    `json:"helper_build"`
	ConsoleDigest    string    `json:"verified_console_digest,omitempty"`
	ConsoleRevision  string    `json:"verified_console_revision,omitempty"`
	ConsoleBuild     uint64    `json:"verified_console_build,omitempty"`
	Outcome          string    `json:"update_outcome,omitempty"`
}

func (u *Updater) phase(phase string) {
	if u.attempt != nil {
		u.attempt.Phase = phase
	}
}
func (u *Updater) finishAttempt(result error) error {
	a := u.attempt
	defer func() { u.attempt = nil }()
	a.Finished = u.now().UTC()
	if result != nil {
		a.ErrorCode = a.Phase
	}
	a.Outcome = u.state.Outcome
	b, err := json.Marshal(a)
	if err != nil {
		return err
	}
	if len(b) > 4096 {
		return errors.New("attempt receipt exceeds bound")
	}
	// Best effort at both destinations, but never claim durable evidence if either fails.
	return errors.Join(u.writeFile(filepath.Join(u.cfg.PrivateDir, "attempt.json"), b, 0600), u.writeFile(filepath.Join(u.cfg.StateDir, "release_attempt.json"), b, 0644))
}
