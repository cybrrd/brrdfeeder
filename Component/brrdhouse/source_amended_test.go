// SPDX-License-Identifier: AGPL-3.0-or-later
package main

import (
	"os"
	"regexp"
	"strings"
	"testing"
)

func TestConsolePageHasNoSourceDisclosure(t *testing.T) {
	c, _ := testConsole(t)
	body := render(c, "/").Body.String()
	footer := regexp.MustCompile(`(?s)<footer>(.*?)</footer>`).FindString(body)
	if footer == "" {
		t.Fatal("missing console footer")
	}
	for _, forbidden := range []string{"source", "repository", "revision", "github"} {
		if strings.Contains(strings.ToLower(footer), forbidden) {
			t.Errorf("console footer still features %q: %s", forbidden, footer)
		}
	}
	for _, link := range regexp.MustCompile(`<a\b[^>]*href="([^"]*)"`).FindAllStringSubmatch(body, -1) {
		if link[1] != "/" && link[1] != "/LICENSE" {
			t.Errorf("unexpected page link: %q", link[1])
		}
	}
	if !strings.Contains(footer, `href="/LICENSE">AGPL-3.0-or-later</a>`) {
		t.Fatal("existing license notice/link removed")
	}
}

func TestImageRecipeNeedsNoSourceURL(t *testing.T) {
	b, err := os.ReadFile("Containerfile")
	if err != nil {
		t.Fatal(err)
	}
	for _, forbidden := range []string{"SOURCE_REPO_URL", "check-build-source", "main.sourceRepository", "main.sourceRevision"} {
		if strings.Contains(string(b), forbidden) {
			t.Errorf("image recipe still depends on page source metadata: %s", forbidden)
		}
	}
}
