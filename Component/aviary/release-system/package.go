// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// Rootless commands always run in the installed console account's namespace.
// No shell interpolation, inherited user choice, or rootful console store.
func (u *Updater) consoleRun(bin string, args ...string) ([]byte, error) {
	if u.cfg.ConsoleUID < 100 || u.cfg.ConsoleUser != "brrdhouse" {
		return nil, errors.New("invalid console service identity")
	}
	runtime := "/run/user/" + strconv.FormatUint(uint64(u.cfg.ConsoleUID), 10)
	argv := []string{"-u", u.cfg.ConsoleUser, "--", "env", "XDG_RUNTIME_DIR=" + runtime, "DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus", bin}
	return u.run.Run("runuser", append(argv, args...)...)
}
func (u *Updater) componentRun(console bool, bin string, args ...string) ([]byte, error) {
	if console {
		return u.consoleRun(bin, args...)
	}
	return u.run.Run(bin, args...)
}
func (u *Updater) componentImage(console bool, ref string) (Image, error) {
	b, e := u.componentRun(console, "podman", "image", "inspect", ref)
	if e != nil {
		return Image{}, e
	}
	var images []Image
	if json.Unmarshal(b, &images) != nil || len(images) != 1 {
		return Image{}, errors.New("invalid component image inspection")
	}
	im := images[0]
	im.ID = strings.TrimPrefix(im.ID, "sha256:")
	if !digestRE.MatchString(im.Digest) || !imageIDRE.MatchString(im.ID) {
		return Image{}, errors.New("unverified component image")
	}
	return im, nil
}
func (u *Updater) consoleCurrent() (Image, error) {
	b, e := u.consoleRun("podman", "inspect", "--type", "container", "brrdhouse")
	if e != nil {
		return Image{}, e
	}
	var c []struct {
		Image string
		State struct{ Running bool }
	}
	if json.Unmarshal(b, &c) != nil || len(c) != 1 || !c[0].State.Running || !imageIDRE.MatchString(strings.TrimPrefix(c[0].Image, "sha256:")) {
		return Image{}, errors.New("console container unavailable")
	}
	return u.componentImage(true, c[0].Image)
}
func packageTarget(m Release) string {
	// Attempts are bound to component bytes, not rollout metadata: a new sequence
	// or salt cannot reset a broken pair's quarantine.
	return repository + "@" + m.Digest + "+" + consoleRepository + "@" + m.ConsoleDigest
}
func (u *Updater) checkFloors(m Release, s EngineStatus, engine, console Image) error {
	sameEngine := contains(engine.RepoDigests, repository+"@"+m.Digest)
	sameConsole := contains(console.RepoDigests, consoleRepository+"@"+m.ConsoleDigest)
	floor := max(s.Heartbeat.Build, u.state.AppliedBuild)
	cfloor := max(u.cfg.ConsoleBuild, u.state.AppliedConsoleBuild)
	if m.BuildSeq < floor || (!sameEngine && m.BuildSeq <= floor) || (sameEngine && (m.BuildSeq != s.Heartbeat.Build || m.Version != s.Heartbeat.Version)) || m.ConsoleBuild < cfloor || (!sameConsole && m.ConsoleBuild <= cfloor) {
		return errors.New("component downgrade or sequence/digest equivocation refused")
	}
	return nil
}
func (u *Updater) consoleRestart() error {
	if _, e := u.consoleRun("systemctl", "--user", "daemon-reload"); e != nil {
		return e
	}
	if _, e := u.consoleRun("systemctl", "--user", "reset-failed", "brrdhouse.service"); e != nil {
		return e
	}
	_, e := u.consoleRun("systemctl", "--user", "restart", "brrdhouse.service")
	return e
}

var renderedTime = regexp.MustCompile(`data-engine-written-at="([^"]+)"`)

func (u *Updater) consoleHealthy(digest, version string, since time.Time) bool {
	deadline := u.now().Add(u.healthTimeout)
	for u.now().Before(deadline) {
		if !u.stableWindow() {
			return false
		}
		s, e := u.status()
		if e == nil && s.Heartbeat.Digest == digest && s.Heartbeat.Version == version {
			// Listen address is validated as a literal local HTTP address at install.
			req, e := http.NewRequest(http.MethodGet, u.cfg.ConsoleURL+"/status", nil)
			if e == nil {
				client := &http.Client{Timeout: 5 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("console redirect refused") }}
				res, e := client.Do(req)
				if e == nil {
					b, re := io.ReadAll(io.LimitReader(res.Body, 1<<20+1))
					res.Body.Close()
					match := renderedTime.FindSubmatch(b)
					if re == nil && len(b) <= 1<<20 && res.StatusCode == 200 && len(match) == 2 {
						written, te := time.Parse(time.RFC3339Nano, string(match[1]))
						body := string(b)
						if te == nil && written.After(since) && !written.After(u.now()) && u.now().Sub(written) <= time.Duration(s.Interval)*3*time.Second && strings.Contains(body, `data-engine-state="healthy"`) && strings.Contains(body, "<dd>"+digest+"</dd>") && strings.Contains(body, "<dd>"+version+"</dd>") {
							return true
						}
					}
				}
			}
		}
		u.sleep(2 * time.Second)
	}
	return false
}

func (u *Updater) hold(console bool, im Image, name string) error {
	// Bind the reviewed host helper even into scratch console images. The inert
	// process keeps the outgoing image referenced by a RUNNING container so
	// system prune cannot collect it. No memory cgroup requirement on stock Pis.
	args := []string{"run", "-d", "--name", name, "--network=none", "--user=65532:65532", "--cap-drop=all", "--security-opt=no-new-privileges", "--read-only", "--read-only-tmpfs=false", "--restart=always", "-v", "/usr/local/libexec/brrdfeeder-release:/hold:ro", "--entrypoint=/hold", im.ID, "hold"}
	if _, e := u.componentRun(console, "podman", args...); e != nil {
		b, ie := u.componentRun(console, "podman", "inspect", "--type", "container", name)
		var c []struct{ Image string }
		if ie != nil || json.Unmarshal(b, &c) != nil || len(c) != 1 || strings.TrimPrefix(c[0].Image, "sha256:") != im.ID {
			return errors.New("rollback anchor unavailable")
		}
		if _, e = u.componentRun(console, "podman", "start", name); e != nil {
			return e
		}
	}
	b, e := u.componentRun(console, "podman", "inspect", "--type", "container", name)
	var c []struct {
		Image string
		State struct{ Running bool }
	}
	if e != nil || json.Unmarshal(b, &c) != nil || len(c) != 1 || strings.TrimPrefix(c[0].Image, "sha256:") != im.ID || !c[0].State.Running {
		return errors.New("outgoing image is not protected by running anchor")
	}
	return nil
}
func consolePin(raw []byte) (string, error) {
	n := 0
	pin := ""
	for _, l := range strings.Split(string(raw), "\n") {
		if strings.HasPrefix(l, "Image=") {
			n++
			pin = strings.TrimPrefix(l, "Image=")
		}
	}
	if n != 1 || !strings.HasPrefix(pin, consoleRepository+"@") || !digestRE.MatchString(strings.TrimPrefix(pin, consoleRepository+"@")) {
		return "", errors.New("console Quadlet must be digest-pinned")
	}
	return pin, nil
}
func (u *Updater) pull(console bool, target string) (Image, error) {
	if _, e := u.componentRun(console, "podman", "pull", target); e != nil {
		// A staged release can use an already cached, exact verified digest
		// when WAN is unavailable; never fall back to a tag or different bytes.
		im, localErr := u.componentImage(console, target)
		if localErr == nil && contains(im.RepoDigests, target) {
			return im, nil
		}
		return Image{}, e
	}
	im, e := u.componentImage(console, target)
	if e != nil {
		return im, e
	}
	if !contains(im.RepoDigests, target) {
		return im, errors.New("pulled target absent from RepoDigests")
	}
	return im, nil
}
func (u *Updater) apply() error {
	raw, e := readBounded(u.pending(), maxDocument)
	if os.IsNotExist(e) {
		return nil
	}
	if e != nil {
		return e
	}
	req, e := u.releaseRequest(raw)
	if e != nil {
		return e
	}
	m := *req.Release
	target := packageTarget(m)
	if u.state.Quarantined[target] || u.state.Attempts[target] >= attemptCap { // D44 MUTATE cap
		u.state.Quarantined[target] = true
		if e = u.save(); e != nil {
			return e
		}
		if e = u.outcome("quarantined", target, u.state.Attempts[target]); e != nil {
			return e
		}
		return u.clearPending(req.Hash)
	}
	s, e := u.status()
	if e != nil {
		return e
	}
	im, e := u.current()
	if e != nil {
		return e
	}
	ci, e := u.consoleCurrent()
	if e != nil {
		return e
	}
	if s.Heartbeat.Digest != im.Digest {
		return errors.New("current engine identity mismatch")
	}
	if e = u.checkFloors(m, s, im, ci); e != nil {
		return e
	}
	if len(u.state.Attempts) >= 32 && u.state.Attempts[target] == 0 {
		return errors.New("failure history full; operator review required")
	}
	quad, e := readBounded(u.cfg.Quadlet, 65536)
	if e != nil {
		return e
	}
	old, e := previousPin(quad)
	if e != nil {
		return e
	}
	cq, e := readBounded(u.cfg.ConsoleQuadlet, 65536)
	if e != nil {
		return e
	}
	cp, e := consolePin(cq)
	if e != nil {
		return e
	}
	if !contains(im.RepoDigests, old) || !contains(ci.RepoDigests, cp) {
		return errors.New("outgoing pins do not match containers")
	}
	ec := !contains(im.RepoDigests, req.Target)
	cc := !contains(ci.RepoDigests, consoleRepository+"@"+m.ConsoleDigest)
	ni, nc := im, ci
	// Both downloads complete before either service is disturbed.
	if ec {
		ni, e = u.pull(false, req.Target)
		if e != nil {
			return e
		}
	}
	if cc {
		nc, e = u.pull(true, consoleRepository+"@"+m.ConsoleDigest)
		if e != nil {
			return e
		}
	}
	anchor := "brrdfeeder-rollback-" + req.Hash[:16]
	u.point("downloads")
	ca := "brrdhouse-rollback-" + req.Hash[:16]
	if ec {
		if e = u.hold(false, im, anchor); e != nil {
			return e
		}
	}
	if cc {
		if e = u.hold(true, ci, ca); e != nil {
			return e
		}
	}
	tx := &Transaction{Request: req, Previous: old, PreviousID: im.ID, PreviousDigest: im.Digest, PreviousBuild: s.Heartbeat.Build, PreviousVersion: s.Heartbeat.Version, Quadlet: quad, Anchor: anchor, Phase: "prepared", Started: u.now(), EngineChanged: ec, Console: &Checkpoint{Quadlet: cq, Previous: ci, Next: nc, Anchor: ca, Changed: cc}}
	u.point("anchors")
	u.state.Active = tx
	u.state.Seen = Seen{m.Sequence, contentHash(m.canonical())}
	if e = u.save(); e != nil {
		return e
	}
	if e = atomicJSON(u.marker(), map[string]bool{"active": true}, 0644); e != nil {
		return e
	}
	u.point("prepared")
	tx.Phase = "switching"
	tx.Started = u.now()
	if e = u.save(); e != nil {
		return e
	}
	if e = u.beginWatch(); e != nil {
		return u.failedUpdate()
	}
	u.point("switching")
	if ec {
		q, re := repin(quad, req.Target)
		if re != nil {
			return re
		}
		if e = atomicFile(u.cfg.Quadlet, q, 0644); e == nil {
			u.point("engine-pin")
			e = u.restart()
			u.point("engine-restart")
		}
		if e == nil {
			e = u.afterPlannedRestart(false, ni.ID)
		}
		if e != nil || !u.healthy(ni.Digest, m.Version, m.BuildSeq, tx.Started) {
			return u.failedUpdate()
		} // D44 MUTATE health rollback
	}
	if !ec && !u.healthy(im.Digest, m.Version, m.BuildSeq, tx.Started) {
		return u.failedUpdate()
	}
	if cc {
		q, re := repin(cq, consoleRepository+"@"+m.ConsoleDigest)
		if re != nil {
			return re
		}
		if e = atomicFile(u.cfg.ConsoleQuadlet, q, 0644); e == nil {
			u.point("console-pin")
			e = u.consoleRestart()
			u.point("console-restart")
		}
		if e == nil {
			e = u.afterPlannedRestart(true, nc.ID)
		}
		if e != nil {
			return u.failedUpdate()
		}
	}
	if !u.consoleHealthy(ni.Digest, m.Version, tx.Started) {
		return u.failedUpdate()
	}
	actual, e := u.current()
	ac, ce := u.consoleCurrent()
	if e != nil || ce != nil || actual.ID != ni.ID || ac.ID != nc.ID || !u.stableWindow() {
		return u.failedUpdate()
	}
	tx.Phase = "confirmed"
	u.point("health")
	if e = u.save(); e != nil {
		return e
	}
	return u.commit()
}
func (u *Updater) commit() error {
	tx := u.state.Active
	if tx == nil {
		return nil
	}
	m := tx.Request.Release
	if m == nil || tx.Console == nil {
		return errors.New("incomplete package journal")
	}
	target := packageTarget(*m)
	u.point("confirmed")
	if e := u.outcome("applied", target, u.state.Attempts[target]); e != nil {
		return e
	}
	u.state.AppliedBuild = max(u.state.AppliedBuild, m.BuildSeq)
	u.state.AppliedConsoleBuild = max(u.state.AppliedConsoleBuild, m.ConsoleBuild)
	u.state.RunningSequence = m.Sequence
	actual, err := u.current()
	if err != nil {
		return err
	}
	u.state.RunningEngineDigest = actual.Digest
	// Preserve the established engine watermark; only host-confirmed health
	// advances it. Release anti-replay lives in the root-owned private journal.
	if e := atomicJSON(filepath.Join(u.cfg.StateDir, "policy_state.json"), map[string]any{"last_applied_unix_ms": u.state.AppliedIssued, "last_applied_build_seq": u.state.AppliedBuild}, 0644); e != nil {
		return e
	}
	if e := u.clearPending(tx.Request.Hash); e != nil {
		return e
	}
	u.point("commit-pending-removed")
	for _, x := range []struct {
		console   bool
		changed   bool
		old, next string
	}{{false, tx.EngineChanged, u.state.Anchor, tx.Anchor}, {true, tx.Console.Changed, u.state.ConsoleAnchor, tx.Console.Anchor}} {
		if x.changed && x.old != "" && x.old != x.next {
			if _, e := u.componentRun(x.console, "podman", "rm", "--ignore", "-f", x.old); e != nil {
				return e
			}
		}
	}
	if tx.EngineChanged {
		u.state.Anchor = tx.Anchor
	}
	if tx.Console.Changed {
		u.state.ConsoleAnchor = tx.Console.Anchor
	}
	delete(u.state.Attempts, target)
	u.state.Active = nil
	if e := u.save(); e != nil {
		return e
	}
	u.point("commit-complete")
	return removeOptional(u.marker())
}
func (u *Updater) failedUpdate() error {
	tx := u.state.Active
	if !tx.FailureCounted {
		u.state.Attempts[packageTarget(*tx.Request.Release)]++
		tx.FailureCounted = true
		if e := u.save(); e != nil {
			return e
		}
	}
	return u.recover()
}
func (u *Updater) recover() error     { return u.recoverWithReadiness(false) }
func (u *Updater) recoverBoot() error { return u.recoverWithReadiness(true) }
func (u *Updater) recoverWithReadiness(boot bool) error {
	tx := u.state.Active
	if tx == nil {
		return nil
	}
	if tx.Phase == "confirmed" {
		return u.commit()
	}
	if tx.Request.Release == nil || tx.Console == nil {
		return errors.New("incomplete recovery journal")
	}
	target := packageTarget(*tx.Request.Release)
	retrying := tx.Phase == "recovery_wait"
	tx.Phase = "rollback"
	if e := u.save(); e != nil {
		return e
	}
	since := u.now()
	u.point("rollback-journal")
	u.watch = &restartWatch{}
	// Restore both pins even if the first restart fails; a console failure must
	// never prevent attempting to recover the engine.
	var faults []error
	{
		e := atomicFile(u.cfg.ConsoleQuadlet, tx.Console.Quadlet, 0644)
		if e == nil {
			u.point("rollback-console-pin")
			im, err := u.consoleCurrent()
			if !retrying || err != nil || im.ID != tx.Console.Previous.ID {
				e = u.consoleRestart()
			}
			u.point("rollback-console-restart")
		}
		if e == nil && !boot {
			e = u.afterPlannedRestart(true, tx.Console.Previous.ID)
		}
		if e != nil {
			faults = append(faults, e)
		}
	}
	{
		e := atomicFile(u.cfg.Quadlet, tx.Quadlet, 0644)
		if e == nil {
			u.point("rollback-engine-pin")
			im, err := u.current()
			if !retrying || err != nil || im.ID != tx.PreviousID {
				e = u.restart()
			}
			u.point("rollback-engine-restart")
		}
		if e == nil && !boot {
			e = u.afterPlannedRestart(false, tx.PreviousID)
		}
		if e != nil {
			faults = append(faults, e)
		}
	}
	if !boot && !u.healthy(tx.PreviousDigest, tx.PreviousVersion, tx.PreviousBuild, since) {
		faults = append(faults, errors.New("previous engine not healthy"))
	}
	if !boot && !u.consoleHealthy(tx.PreviousDigest, tx.PreviousVersion, since) {
		faults = append(faults, errors.New("previous console not healthy"))
	}
	im, ie := u.current()
	ci, ce := u.consoleCurrent()
	if ie != nil || ce != nil || im.ID != tx.PreviousID || ci.ID != tx.Console.Previous.ID {
		faults = append(faults, errors.New("rollback container mismatch"))
	}
	if len(faults) > 0 {
		// Readiness of the retained version is not evidence against the new
		// digest pair. Retain the journal; later timer ticks retry locally.
		tx.Phase = "recovery_wait"
		if e := u.save(); e != nil {
			return e
		}
		_ = u.outcome("rollback_failed", target, u.state.Attempts[target])
		return fmt.Errorf("rollback waiting; previous pins retained, retry on next tick: %w", errors.Join(faults...))
	}
	kind := "rolled_back"
	if u.state.Attempts[target] >= attemptCap {
		u.state.Quarantined[target] = true
		kind = "quarantined"
	} // D44 MUTATE quarantine after failure
	if e := u.outcome(kind, target, u.state.Attempts[target]); e != nil {
		return e
	}
	if e := u.clearPending(tx.Request.Hash); e != nil {
		return e
	}
	u.state.Active = nil
	if e := u.save(); e != nil {
		return e
	}
	if e := removeOptional(u.marker()); e != nil {
		return e
	}
	u.point("rollback-complete")
	if tx.EngineChanged {
		if _, e := u.run.Run("podman", "rm", "--ignore", "-f", tx.Anchor); e != nil {
			return e
		}
	}
	if tx.Console.Changed {
		if _, e := u.consoleRun("podman", "rm", "--ignore", "-f", tx.Console.Anchor); e != nil {
			return e
		}
	}
	return nil
}
