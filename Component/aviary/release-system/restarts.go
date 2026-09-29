// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"errors"
	"strconv"
	"strings"
)

// A window covers BOTH workloads, including the unchanged component. Podman
// counters alone miss systemd recreating a container; NRestarts alone misses
// in-container restart policy. Invocation/container IDs catch replacement/reset.
type restartSample struct {
	UnitRestarts      uint64
	ContainerRestarts uint64
	Invocation        string
	Container         string
	Image             string
}
type restartWatch [2]restartSample

func componentIndex(console bool) int {
	if console {
		return 1
	}
	return 0
}

func (u *Updater) restartSample(console bool) (restartSample, error) {
	var result restartSample
	unit, container := u.cfg.EngineUnit, u.cfg.Container
	args := []string{}
	if console {
		unit, container = "brrdhouse.service", "brrdhouse"
		args = append(args, "--user")
	}
	args = append(args, "show", unit, "--property=NRestarts", "--property=InvocationID", "--property=ActiveState")
	b, e := u.componentRun(console, "systemctl", args...)
	if e != nil {
		return result, e
	}
	values := map[string]string{}
	for _, line := range strings.Split(strings.TrimSpace(string(b)), "\n") {
		key, value, ok := strings.Cut(line, "=")
		if !ok || values[key] != "" {
			return result, errors.New("invalid unit restart observation")
		}
		values[key] = value
	}
	result.UnitRestarts, e = strconv.ParseUint(values["NRestarts"], 10, 64)
	result.Invocation = values["InvocationID"]
	if e != nil || values["ActiveState"] != "active" || len(result.Invocation) != 32 {
		return result, errors.New("unit unavailable or restart counters unknown")
	}
	b, e = u.componentRun(console, "podman", "inspect", "--type", "container", container)
	if e != nil {
		return result, e
	}
	var rows []struct {
		ID           string `json:"Id"`
		Image        string
		RestartCount *uint64
		State        struct{ Running bool }
	}
	if json.Unmarshal(b, &rows) != nil || len(rows) != 1 || !rows[0].State.Running || rows[0].RestartCount == nil || !imageIDRE.MatchString(rows[0].ID) {
		return result, errors.New("container restart counters unavailable")
	}
	result.Container, result.Image = rows[0].ID, strings.TrimPrefix(rows[0].Image, "sha256:")
	result.ContainerRestarts = *rows[0].RestartCount
	return result, nil
}
func (u *Updater) beginWatch() error {
	var w restartWatch
	for _, console := range []bool{false, true} {
		s, e := u.restartSample(console)
		if e != nil {
			return e
		}
		w[componentIndex(console)] = s
	}
	u.watch = &w
	return nil
}
func (u *Updater) afterPlannedRestart(console bool, image string) error {
	s, e := u.restartSample(console)
	// reset-failed was issued before the explicit restart; a recreated container
	// starts at zero too. Catch loops even during systemctl's blocking start.
	if e != nil || s.UnitRestarts != 0 || s.ContainerRestarts != 0 || s.Image != image {
		return errors.New("restart during component start or wrong running image")
	}
	if u.watch == nil {
		u.watch = &restartWatch{}
	}
	u.watch[componentIndex(console)] = s
	return nil
}
func (u *Updater) stableWindow() bool {
	if u.watch == nil {
		return u.beginWatch() == nil
	}
	for _, console := range []bool{false, true} {
		s, e := u.restartSample(console)
		if e != nil || s != u.watch[componentIndex(console)] {
			return false
		} // D44 MUTATE restart-window
	}
	return true
}
