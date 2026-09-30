// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"os"
	"strings"
	"testing"
	"time"
)

const (
	productVersionFixture = "0.8.20"
	sourceRevisionFixture = "0123456789abcdef0123456789abcdef01234567"
)

func versionFixture(t *testing.T, productVersion *string) string {
	t.Helper()
	c, path := testConsole(t)
	fixture(t, path, time.Second, 30, false)
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var document map[string]any
	if err := json.Unmarshal(b, &document); err != nil {
		t.Fatal(err)
	}
	heartbeat := document["heartbeat"].(map[string]any)
	heartbeat["engine_version"] = sourceRevisionFixture
	heartbeat["build_seq"] = 1042
	if productVersion != nil {
		heartbeat["product_version"] = *productVersion
	}
	b, err = json.Marshal(document)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, b, 0644); err != nil {
		t.Fatal(err)
	}
	return render(c, "/").Body.String()
}

func requireSoftwareValue(t *testing.T, body, label, value string) {
	t.Helper()
	want := "<dt>" + label + "</dt><dd>" + value + "</dd>"
	if !strings.Contains(body, want) {
		t.Fatalf("missing reported software value %q; body:\n%s", want, body)
	}
}

func TestConsoleShowsProductVersionWithBuildIdentity(t *testing.T) {
	body := versionFixture(t, stringPointer(productVersionFixture))
	requireSoftwareValue(t, body, "Engine version", productVersionFixture)
	requireSoftwareValue(t, body, "Source revision", sourceRevisionFixture)
	requireSoftwareValue(t, body, "Build number", "1042")
}

func TestConsoleDoesNotGuessMissingProductVersion(t *testing.T) {
	body := versionFixture(t, nil)
	requireSoftwareValue(t, body, "Engine version", "Unknown")
	requireSoftwareValue(t, body, "Source revision", sourceRevisionFixture)
	requireSoftwareValue(t, body, "Build number", "1042")
	if strings.Contains(body, "<dt>Engine version</dt><dd>"+sourceRevisionFixture+"</dd>") {
		t.Fatal("source revision was guessed to be the product version")
	}
}

func stringPointer(value string) *string { return &value }
