// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"syscall"
)

// Reserve for bounded journals, pins and receipts, not an estimate of image size.
// Image extraction can still exhaust a store; pulls remain pre-transaction.
const metadataReserve = 16 << 20
const inodeReserve = 64

func freeSpace(path string) (uint64, uint64, error) {
	var st syscall.Statfs_t
	if err := syscall.Statfs(path, &st); err != nil {
		return 0, 0, err
	}
	if st.Bsize <= 0 {
		return 0, 0, errors.New("invalid filesystem block size")
	}
	return st.Bavail * uint64(st.Bsize), st.Ffree, nil
}

func (u *Updater) resourceCheck() error {
	u.phase("resource")
	paths := []string{u.cfg.PrivateDir, u.cfg.StateDir, filepath.Dir(u.cfg.Quadlet), filepath.Dir(u.cfg.ConsoleQuadlet)}
	for _, console := range []bool{false, true} {
		b, err := u.componentRun(console, "podman", "info", "--format", "{{.Store.GraphRoot}}")
		if err != nil {
			return fmt.Errorf("container store unavailable: %w", err)
		}
		path := strings.TrimSpace(string(b))
		if !filepath.IsAbs(path) || strings.ContainsAny(path, "\r\n\x00") {
			return errors.New("invalid container store path")
		}
		paths = append(paths, path)
	}
	measure := u.space
	if measure == nil {
		measure = freeSpace
	}
	for _, path := range paths {
		bytes, inodes, err := measure(path)
		if err != nil {
			return fmt.Errorf("storage measurement failed: %w", err)
		}
		if bytes < metadataReserve || inodes < inodeReserve {
			return errors.New("insufficient free bytes or inodes; update deferred")
		}
	}
	return nil
}
