// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"testing"
	"time"
)

func TestGoodDrillConsoleHealth(t *testing.T) {
	c, path := testConsole(t)
	fixture(t, path, time.Second, 30, false)
	got := render(c, "/status")
	if got.Code != 200 || got.Header().Get("X-BRRDhouse-Release") != "0.8.31" {
		t.Fatalf("healthy drill response: HTTP %d, headers %v", got.Code, got.Header())
	}
	assertState(t, got.Body.String(), "healthy")
}
