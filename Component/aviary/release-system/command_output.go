// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"context"
	"errors"
	"sync"
)

var errOutputBound = errors.New("command output too large")

type boundedOutput struct {
	mu       sync.Mutex
	buf      bytes.Buffer
	cancel   context.CancelFunc
	overflow bool
}

func (w *boundedOutput) Write(p []byte) (int, error) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.overflow || len(p) > (1<<20)-w.buf.Len() {
		w.overflow = true
		w.cancel()
		return 0, errOutputBound
	}
	return w.buf.Write(p)
}
