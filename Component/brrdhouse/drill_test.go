// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"testing"
	"time"
)

func TestReleaseConsoleHealth(t *testing.T) {
	c, path := testConsole(t)
	fixture(t, path, time.Second, 30, false)
	got := render(c, "/status")
	if got.Code != 200 || got.Header().Get("X-BRRDhouse-Release") != "0.9.0" {
		t.Fatalf("healthy release response: HTTP %d, headers %v", got.Code, got.Header())
	}
	assertState(t, got.Body.String(), "healthy")
}
