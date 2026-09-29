package main

import (
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
)

func TestSnapshotMigrationKeepsHostDowngradeFloor(t *testing.T) {
	u, _, key := setup(t)
	if err := atomicJSON(filepath.Join(u.cfg.PrivateDir, "host-state.json"), HostState{Build: 824, Attempted: map[string]bool{}}, 0600); err != nil {
		t.Fatal(err)
	}
	m := hostManifest(u)
	m.Build = 823
	web := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/general/updater.json" {
			t.Error("below-floor host update tried artifact download")
			http.NotFound(w, r)
			return
		}
		w.Write(hostSign(m, key))
	}))
	defer web.Close()
	u.client = web.Client()
	u.cfg.BaseURL = web.URL
	err := u.pollHostAt(filepath.Join(t.TempDir(), "unmodified-binary"), filepath.Join(t.TempDir(), "unmodified-receipt"))
	if err == nil || !strings.Contains(err.Error(), "downgrade refused") {
		t.Fatalf("host floor was weakened: %v", err)
	}
}

func TestSnapshotSequenceExceedsFieldFloorsWithoutWeakeningThem(t *testing.T) {
	for _, field := range []uint64{618, 808, 823, 824} {
		u := &Updater{}
		u.cfg.ConsoleBuild = field
		u.state.AppliedBuild = field
		u.state.AppliedConsoleBuild = field
		var status EngineStatus
		status.Heartbeat.Build = field
		candidate := Release{BuildSeq: 1001, ConsoleBuild: 1001, Digest: "sha256:new-engine", ConsoleDigest: "sha256:new-console"}
		if err := u.checkFloors(candidate, status, Image{}, Image{}); err != nil {
			t.Fatalf("snapshot 1001 above %d refused: %v", field, err)
		}
		for _, seq := range []uint64{1, field - 1, field} {
			candidate.BuildSeq = seq
			candidate.ConsoleBuild = 1001
			if u.checkFloors(candidate, status, Image{}, Image{}) == nil {
				t.Fatalf("engine floor %d admitted %d", field, seq)
			}
			candidate.BuildSeq = 1001
			candidate.ConsoleBuild = seq
			if u.checkFloors(candidate, status, Image{}, Image{}) == nil {
				t.Fatalf("console floor %d admitted %d", field, seq)
			}
		}
	}
}

func TestCompiledPublicRepositories(t *testing.T) {
	if repository != "ghcr.io/cybrrd/brrdfeeder" || consoleRepository != "ghcr.io/cybrrd/brrdhouse" {
		t.Fatal("compiled public repository anchors still name former organization")
	}
}
