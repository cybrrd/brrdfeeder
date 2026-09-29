// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"errors"
	"strings"
	"testing"
	"time"
)

func TestFreshActiveCrashLoopRollsBack(t *testing.T) {
	for _, counter := range []string{"engine-systemd", "console-systemd", "engine-container", "console-container", "engine-replaced", "console-replaced"} {
		t.Run(counter, func(t *testing.T) {
			u, h, key := setup(t)
			m := manifest(u)
			m.ConsoleDigest = newDigest
			m.ConsoleBuild = 2
			m.ConsoleVersion = newVersion
			queue(t, u, sign(m, key))
			sleep := u.sleep
			injected := false
			u.sleep = func(d time.Duration) {
				sleep(d)
				if !injected && h.active.Digest == newDigest {
					injected = true
					i := 0
					if strings.HasPrefix(counter, "console") {
						i = 1
					}
					if strings.HasSuffix(counter, "systemd") {
						h.unitAuto[i]++
					} else if strings.HasSuffix(counter, "container") {
						h.containerAuto[i]++
					} else {
						h.incarnation[i]++
					}
				}
			}
			if e := u.execute("apply"); e != nil {
				t.Fatal(e)
			}
			if !injected || h.active.Digest != oldDigest || h.consoleActive.Digest != oldDigest || outcome(t, u)["kind"] != "rolled_back" {
				t.Fatal("fresh active crash loop passed health", counter)
			}
		})
	}
}

type unavailableCounters struct{ Runner }

func (r unavailableCounters) Run(bin string, args ...string) ([]byte, error) {
	if strings.Contains(strings.Join(args, " "), "--property=NRestarts") {
		return nil, errors.New("counter unavailable")
	}
	return r.Runner.Run(bin, args...)
}
func TestRestartCountersFailClosed(t *testing.T) {
	u, h, _ := setup(t)
	u.run = unavailableCounters{h}
	if u.healthy(oldDigest, oldVersion, 1, u.now().Add(-time.Second)) {
		t.Fatal("unknown counters accepted")
	}
	if u.consoleHealthy(oldDigest, oldVersion, u.now().Add(-time.Second)) {
		t.Fatal("unknown console counters accepted")
	}
}
func TestStartupLoopBeforeFirstObservationRefused(t *testing.T) {
	u, h, _ := setup(t)
	if e := u.beginWatch(); e != nil {
		t.Fatal(e)
	}
	h.unitAuto[0] = 1
	if e := u.afterPlannedRestart(false, h.old.ID); e == nil {
		t.Fatal("startup loop passed")
	}
}
