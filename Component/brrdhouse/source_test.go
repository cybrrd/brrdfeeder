// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

const sourceFixtureRevision = "0123456789abcdef0123456789abcdef01234567"

func TestRequestCannotAddSourceLink(t *testing.T) {
	c, _ := testConsole(t)
	r := httptest.NewRequest("GET", "http://console.test:8080/?source=https://evil.example/repo&revision=main", nil)
	r.Header.Set("X-Forwarded-Host", "evil.example")
	w := httptest.NewRecorder()
	c.ServeHTTP(w, r)
	body := w.Body.String()
	if w.Code != 200 || strings.Contains(body, "Source (AGPL-3.0)") || strings.Contains(body, "evil.example") {
		t.Fatalf("request changed the source-free page: %s", body)
	}
}

func TestSourceArchiveGETAndHEADAreNotFound(t *testing.T) {
	c, _ := testConsole(t)
	for _, method := range []string{"GET", "HEAD"} {
		w := httptest.NewRecorder()
		c.ServeHTTP(w, httptest.NewRequest(method, "http://console.test:8080/source.tar.gz", nil))
		if w.Code != 404 {
			t.Fatalf("%s archive: got %d", method, w.Code)
		}
	}
	if w := render(c, "/LICENSE"); w.Code != 200 || !strings.Contains(w.Body.String(), "GNU AFFERO GENERAL PUBLIC LICENSE") {
		t.Fatal("license route no longer available")
	}
}

func TestBuildNeedsNoSourceRepository(t *testing.T) {
	bin := t.TempDir()
	// Exercise the real helper, stubbing only its external build/export tools.
	for name, script := range map[string]string{
		"git":    "#!/bin/sh\ncase \"$*\" in 'rev-parse --verify HEAD') echo " + sourceFixtureRevision + ";; 'rev-parse --is-shallow-repository') echo false;; 'rev-list --count HEAD') echo 123;; 'status --porcelain --untracked-files=all -- .') ;; *) exit 2;; esac\n",
		"podman": "#!/bin/sh\nprintf '%s\\n' \"$@\"\n",
		"tar":    "#!/bin/sh\nexit 0\n",
	} {
		if err := os.WriteFile(filepath.Join(bin, name), []byte(script), 0700); err != nil {
			t.Fatal(err)
		}
	}
	cmd := exec.Command("sh", "build.sh", "amd64")
	for _, value := range os.Environ() {
		if !strings.HasPrefix(value, "SOURCE_REPO_URL=") && !strings.HasPrefix(value, "PATH=") {
			cmd.Env = append(cmd.Env, value)
		}
	}
	cmd.Env = append(cmd.Env, "PATH="+bin+":"+os.Getenv("PATH"))
	out, err := cmd.CombinedOutput()
	if err != nil || strings.Contains(string(out), "SOURCE_REPO_URL") || !strings.Contains(string(out), "SOURCE_REVISION="+sourceFixtureRevision) {
		t.Fatalf("build without URL: err=%v %s", err, out)
	}
	if !strings.Contains(string(out), "BUILD_SEQ=1123") {
		t.Fatal("Self-Update build sequence lost from source-free build helper")
	}
}

func TestSourceArchiveRemovedFromImageRecipe(t *testing.T) {
	b, err := os.ReadFile("Containerfile")
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(b), "source.tar") {
		t.Fatal("appliance image still builds/bundles source archive")
	}
}

func TestCustomerConsoleName(t *testing.T) {
	c, _ := testConsole(t)
	body := render(c, "/").Body.String()
	for _, old := range []string{"BRRDhouse Open", "OPEN EDITION", "Corresponding source", "href=\"/source.tar.gz\""} {
		if strings.Contains(body, old) {
			t.Errorf("obsolete customer text %q", old)
		}
	}
}
