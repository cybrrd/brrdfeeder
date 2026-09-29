// SPDX-License-Identifier: AGPL-3.0-or-later
package main

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestPendingIsPrivateAndBadSlotsRecover(t *testing.T) {
	for _, kind := range []string{"junk", "symlink", "fifo", "expired"} {
		t.Run(kind, func(t *testing.T) {
			u, h, key := setup(t)
			if filepath.Dir(u.pending()) != u.cfg.PrivateDir {
				t.Fatal("engine can write mailbox")
			}
			m := manifest(u)
			raw := sign(m, key)
			switch kind {
			case "junk":
				queue(t, u, []byte("junk"))
			case "symlink":
				if e := os.Symlink(u.cfg.StatusFile, u.pending()); e != nil {
					t.Fatal(e)
				}
			case "fifo":
				if e := syscall.Mkfifo(u.pending(), 0600); e != nil {
					t.Fatal(e)
				}
			case "expired":
				m.Expires = u.now().Add(-time.Minute).Format(time.RFC3339)
				queue(t, u, sign(m, key))
			}
			s := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write(raw) }))
			defer s.Close()
			u.client = s.Client()
			u.cfg.BaseURL = s.URL
			if e := u.execute("poll"); e != nil {
				t.Fatal(e)
			}
			if h.active.Digest != newDigest {
				t.Fatal("slot wedged")
			}
			reasons, _ := filepath.Glob(filepath.Join(u.cfg.PrivateDir, "rejected-pending-*", "reason.json"))
			if len(reasons) != 1 {
				t.Fatal("missing quarantine reason", reasons)
			}
			evidence(t, "rework-mailbox-"+kind+".json", map[string]any{"recovered": true, "private_mailbox": u.pending(), "reason_file": reasons[0]})
		})
	}
}

func TestStagedAppliesWithoutManifestNetwork(t *testing.T) {
	u, h, key := setup(t)
	queue(t, u, sign(manifest(u), key))
	u.client = &http.Client{Transport: offlineTransport{}}
	u.run = offlineRunner{h} // cached exact image remains inspectable; pulls fail
	if e := u.execute("poll"); e != nil {
		t.Fatal(e)
	}
	if h.active.Digest != newDigest {
		t.Fatal("WAN prevented cached apply")
	}
}

func TestClockTrustPrecedesExpiryDecisions(t *testing.T) {
	for _, kind := range []string{"false", "absent", "stale", "future"} {
		t.Run(kind, func(t *testing.T) {
			u, h, key := setup(t)
			queue(t, u, sign(manifest(u), key))
			s, e := u.status()
			if e != nil {
				t.Fatal(e)
			}
			switch kind {
			case "false":
				b := false
				s.Heartbeat.Trusted = &b
			case "absent":
				s.Heartbeat.Trusted = nil
			case "stale":
				s.Written = s.Written.Add(-time.Hour)
			case "future":
				s.Written = s.Written.Add(time.Hour)
			}
			if e = atomicJSON(u.cfg.StatusFile, s, 0644); e != nil {
				t.Fatal(e)
			}
			for _, mode := range []string{"poll", "apply", "poll-updater"} {
				if e = u.execute(mode); e == nil || !strings.Contains(e.Error(), "clock untrusted") {
					t.Fatal(mode, e)
				}
			}
			if h.pulls != 0 || h.restarts != 0 {
				t.Fatal("untrusted clock mutated workload")
			}
			if _, e = os.Stat(u.pending()); e != nil {
				t.Fatal("clock outage destroyed pending")
			}
			h.write()
			u.client = &http.Client{Transport: offlineTransport{}}
			if e = u.execute("poll"); e != nil || h.active.Digest != newDigest {
				t.Fatal("trusted retry", e)
			}
		})
	}
}

type unavailableConsole struct{ host *fakeHost }

func (r unavailableConsole) Run(bin string, args ...string) ([]byte, error) {
	if bin == "runuser" {
		return nil, errors.New("user manager not ready at boot")
	}
	return r.host.Run(bin, args...)
}
func TestBootReadinessNeverQuarantinesGoodPair(t *testing.T) {
	u, h, key := setup(t)
	queue(t, u, sign(manifest(u), key))
	// Interrupt an actual switch; preserve its journal exactly as boot finds it.
	u.checkpoint = func(point string) {
		if point == "engine-pin" {
			panic("power loss fixture")
		}
	}
	func() {
		defer func() {
			if recover() == nil {
				t.Fatal("switch not interrupted")
			}
		}()
		_ = u.execute("apply")
	}()
	u.checkpoint = nil
	h.rollbackBad = true // no radio fix on retained engine at boot
	u.run = unavailableConsole{h}
	if e := u.execute("recover"); e == nil {
		t.Fatal("unavailable console falsely recovered")
	}
	target := packageTarget(manifest(u))
	if u.state.Quarantined[target] || u.state.Attempts[target] != 0 || u.state.Active == nil {
		t.Fatal("boot outage permanently quarantined good pair")
	}
	n := h.restarts
	if e := u.execute("recover"); e == nil {
		t.Fatal("console still absent")
	}
	if h.restarts != n {
		t.Fatal("restarted already-restored engine on retry")
	}
	u.run = h
	if e := u.execute("recover"); e != nil {
		t.Fatal(e)
	}
	if u.state.Active != nil || u.state.Quarantined[target] || h.active.Digest != oldDigest {
		t.Fatal("recovery failed")
	}
	evidence(t, "rework-boot-readiness.json", map[string]any{"quarantined": false, "failed_update_attempts": u.state.Attempts[target], "engine_restarts": h.restarts, "console_recovered": true})
}
