// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

// The template is the exact CFGEOF body from the v0.8.31 installer at
// 6eed7b81a15415a4f9f9564cdb85341b886520cb. Only its enrollment placeholder
// is replaced. This is a source-derived temporary-root test, not a native
// installation; the native rehearsal must run the historical installer.
func oldInstallerLayout(t *testing.T) *localFixture {
	t.Helper()
	f := newLocalFixture(t)
	config, e := os.ReadFile("tests/fixtures/v0.8.31-config.yaml")
	if e != nil {
		t.Fatal(e)
	}
	config = bytes.Replace(config, []byte(`id: "unenrolled"`), []byte(`id: "node-one"`), 1)
	if e = os.WriteFile(f.p.config, config, 0600); e != nil {
		t.Fatal(e)
	}
	if e = os.Remove(f.p.active); e != nil {
		t.Fatal(e)
	}
	bootstrap := filepath.Join(f.p.boundary, "etc/brrdfeeder/secrets/brrdfeeder.creds")
	if e = os.MkdirAll(filepath.Dir(bootstrap), 0755); e != nil {
		t.Fatal(e)
	}
	if e = os.Rename(f.p.bootstrap, bootstrap); e != nil {
		t.Fatal(e)
	}
	f.p.bootstrap = bootstrap
	f.p.active = filepath.Join(f.p.boundary, "var/lib/brrdfeeder/credentials/brrdfeeder.creds")
	// v0.8.31 installer lines 2894–2895: 0640 root:service-group.
	if e = os.Chmod(f.p.bootstrap, 0640); e != nil {
		t.Fatal(e)
	}
	return f
}

func TestLocalUpgradeOldInstallerLayout(t *testing.T) {
	f := oldInstallerLayout(t)
	before := treeBytes(t, f.p.boundary)
	if e := f.run(func(string) {}); e != nil {
		t.Fatalf("real 0.8.31 installer layout refused: %v", e)
	}
	assertProtected(t, f, before)
	for _, path := range []string{f.p.active, filepath.Dir(f.p.active)} {
		if _, e := os.Lstat(path); !os.IsNotExist(e) {
			t.Fatalf("old-layout upgrade created credential state: %s", path)
		}
	}
	before = treeBytes(t, f.p.boundary)
	if e := f.run(func(string) {}); e != nil {
		t.Fatalf("old-layout idempotent repeat refused: %v", e)
	}
	if !reflect.DeepEqual(before, treeBytes(t, f.p.boundary)) {
		t.Fatal("old-layout repeat changed installed state")
	}
}

func TestLocalUpgradeRenewalLayout(t *testing.T) {
	f := newLocalFixture(t)
	config := "node:\n  id: node-one\nbackhaul:\n  credentials_path: /var/lib/brrdfeeder/credentials/brrdfeeder.creds\n"
	if e := os.WriteFile(f.p.config, []byte(config), 0600); e != nil {
		t.Fatal(e)
	}
	before := treeBytes(t, f.p.boundary)
	if e := f.run(func(string) {}); e != nil {
		t.Fatalf("renewal layout refused: %v", e)
	}
	assertProtected(t, f, before)
}

func TestLocalUpgradeLayoutRefusals(t *testing.T) {
	cases := []struct {
		name, want string
		change     func(*localFixture)
	}{
		{"runtime_group_read", "active", func(f *localFixture) { os.Chmod(f.p.active, 0640) }},
		{"runtime_wrong_owner", "active", func(f *localFixture) { f.p.service++ }},
		{"runtime_empty", "active", func(f *localFixture) { os.WriteFile(f.p.active, nil, 0600) }},
		{"runtime_directory", "active", func(f *localFixture) { os.Remove(f.p.active); os.Mkdir(f.p.active, 0700) }},
		{"runtime_hardlink", "active", func(f *localFixture) { os.Link(f.p.active, f.p.active+".linked") }},
		{"runtime_dangling_symlink", "active", func(f *localFixture) { os.Remove(f.p.active); os.Symlink("missing", f.p.active) }},
		{"runtime_symlink", "active", func(f *localFixture) { os.Remove(f.p.active); os.Symlink(f.p.bootstrap, f.p.active) }},
		{"runtime_trust_mismatch", "credentials", func(f *localFixture) { os.WriteFile(f.p.active, testCredsIssuer("node-one", 23, 18), 0600) }},
		{"system_missing", "bootstrap", func(f *localFixture) { os.Remove(f.p.bootstrap) }},
		{"system_group_write", "bootstrap", func(f *localFixture) { os.Chmod(f.p.bootstrap, 0660) }},
		{"system_other_read", "bootstrap", func(f *localFixture) { os.Chmod(f.p.bootstrap, 0644) }},
		{"system_hardlink", "bootstrap", func(f *localFixture) { os.Link(f.p.bootstrap, f.p.bootstrap+".linked") }},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			f := newLocalFixture(t)
			// Even when the configured path permits the old-layout fallback,
			// present runtime state must still pass every existing strict check.
			os.WriteFile(f.p.config, []byte("node:\n  id: node-one\nbackhaul:\n  credentials_path: /etc/brrdfeeder/secrets/brrdfeeder.creds\n"), 0600)
			c.change(f)
			before := treeBytes(t, f.p.boundary)
			e := f.run(func(string) {})
			if e == nil || !strings.Contains(e.Error(), c.want) {
				t.Errorf("unsafe layout accepted: want %s refusal, got %v", c.want, e)
			}
			if !reflect.DeepEqual(before, treeBytes(t, f.p.boundary)) {
				t.Error("REFUSAL MUTATED INSTALLED STATE")
			}
		})
	}
}

func TestLocalUpgradeAbsentRuntimeRequiresSystemConfig(t *testing.T) {
	for _, path := range []string{"/var/lib/brrdfeeder/credentials/brrdfeeder.creds", "/custom/credentials", ""} {
		t.Run(strings.ReplaceAll(path, "/", "_"), func(t *testing.T) {
			f := oldInstallerLayout(t)
			os.WriteFile(f.p.config, []byte("node:\n  id: node-one\nbackhaul:\n  credentials_path: "+path+"\n"), 0600)
			before := treeBytes(t, f.p.boundary)
			e := f.run(func(string) {})
			if e == nil {
				t.Error("absent runtime accepted without the configured system path")
			}
			if !reflect.DeepEqual(before, treeBytes(t, f.p.boundary)) {
				t.Error("REFUSAL MUTATED INSTALLED STATE")
			}
		})
	}
}
