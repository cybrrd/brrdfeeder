// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"io"
	"math"
	"os"
	"strconv"
	"strings"
	"syscall"
)

// This independent, credential-free host view remains available when the engine
// cannot restart. It does not make a stale engine status healthy.
func readHostMemory(snapshot, bootPath, uptimePath string) *memoryStatus {
	f, err := os.OpenFile(snapshot, os.O_RDONLY|syscall.O_NONBLOCK|syscall.O_NOFOLLOW, 0)
	if err != nil {
		return nil
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil || !info.Mode().IsRegular() || info.Size() > 16384 {
		return nil
	}
	raw, err := io.ReadAll(io.LimitReader(f, 16385))
	if err != nil || len(raw) > 16384 {
		return nil
	}
	var host struct {
		Schema  int     `json:"schema_version"`
		Boot    string  `json:"boot_id"`
		Sampled *uint64 `json:"sampled_boottime_secs"`
		memoryStatus
	}
	if json.Unmarshal(raw, &host) != nil || host.Schema != 1 || host.Sampled == nil {
		return nil
	}
	boot, err := os.ReadFile(bootPath)
	if err != nil || strings.TrimSpace(string(boot)) == "" || strings.TrimSpace(string(boot)) != host.Boot {
		return nil
	}
	uptime, err := os.ReadFile(uptimePath)
	if err != nil {
		return nil
	}
	fields := strings.Fields(string(uptime))
	if len(fields) != 2 {
		return nil
	}
	now, err := strconv.ParseFloat(fields[0], 64)
	if err != nil || math.IsNaN(now) || math.IsInf(now, 0) || now < 0 || now >= float64(^uint64(0)) {
		return nil
	}
	seconds := uint64(now)
	if seconds < *host.Sampled || seconds-*host.Sampled > 90 {
		return nil
	}
	return &host.memoryStatus
}

func memoryRepair(v *view) {
	if m := v.Memory; m != nil && m.Events != nil && *m.Events > 0 {
		v.Repairs = append(v.Repairs, repair{
			"The engine has had an out-of-memory event under its memory limit.",
			"Ask your installer to review the engine journal and memory limit. Repeated events can stop automatic restarts; this count is historical, not a current failure.",
		})
	}
}
