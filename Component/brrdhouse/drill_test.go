// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"testing"
	"time"
)

func TestDeliberateDrillHealthFailure(t *testing.T) {
	c, path := testConsole(t)
	fixture(t, path, time.Second, 30, false)
	c.drillHealthFailure = true
	if got := render(c, "/status").Code; got != 503 {
		t.Fatalf("deliberately unhealthy status = %d, want 503", got)
	}
	if got := render(c, "/"); got.Code != 200 {
		t.Fatalf("read-only root unavailable: %d", got.Code)
	} else {
		assertState(t, got.Body.String(), "healthy")
	}
}
