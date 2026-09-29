// SPDX-License-Identifier: AGPL-3.0-or-later
package main

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

const pollFloor = 15 * time.Minute
const attemptCap = 2

type Runner interface {
	Run(string, ...string) ([]byte, error)
}
type commands struct{}

func (commands) Run(bin string, args ...string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, bin, args...)
	b, e := cmd.CombinedOutput()
	if e != nil {
		return nil, fmt.Errorf("%s failed: %w", bin, e)
	}
	if len(b) > 1<<20 {
		return nil, errors.New("command output too large")
	}
	return b, nil
}

type Updater struct {
	cfg           Config
	state         State
	keys          []ed25519.PublicKey
	run           Runner
	client        *http.Client
	now           func() time.Time
	sleep         func(time.Duration)
	healthTimeout time.Duration
	watch         *restartWatch
	checkpoint    func(string) // Test-only process-kill boundary; nil in production.
}

func (u *Updater) point(name string) {
	if u.checkpoint != nil {
		u.checkpoint(name)
	}
}

func newUpdater(c Config) *Updater {
	return &Updater{cfg: c, keys: pinnedKeys, run: commands{}, client: &http.Client{Timeout: 30 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("release redirects refused") }}, now: time.Now, sleep: time.Sleep, healthTimeout: 180 * time.Second}
}
func (u *Updater) execute(mode string) error {
	unlock, e := u.lock()
	if e != nil {
		return e
	}
	defer unlock()
	if e = u.load(); e != nil {
		return e
	}
	if u.state.Active != nil {
		if e = u.recoverBoot(); e != nil {
			return e
		}
		if mode == "poll-updater" {
			// This invocation serviced a package transaction. Host code waits
			// for a later independent tick, even if recovery just succeeded.
			return nil
		}
	} else {
		if e = removeOptional(u.marker()); e != nil {
			return e
		}
	}
	switch mode {
	case "recover":
		// Boot recovery is local-only: no poll, pull or package installation.
		return nil
	case "poll-updater":
		return u.pollHost()
	case "poll":
		// The timer is the only trigger: no PathExists reactivation on a
		// retained mailbox. Recovery occurs before fetching, under one lock.
		if staged, err := u.preparePending(); err != nil {
			return err
		} else if staged {
			// A staged release is independent of the manifest server's availability.
			return u.apply()
		}
		if e = u.poll(); e != nil {
			return e
		}
		return u.apply()
	case "apply":
		return u.apply()
	default:
		return errors.New("unknown mode")
	}
}

type EngineStatus struct {
	Schema    int       `json:"schema_version"`
	Written   time.Time `json:"written_at"`
	Interval  uint64    `json:"status_interval_secs"`
	Heartbeat struct {
		Node    string `json:"node_id"`
		Digest  string `json:"image_digest"`
		Version string `json:"engine_version"`
		Build   uint64 `json:"build_seq"`
		Radio   string `json:"radio_status"`
		Trusted *bool  `json:"os_clock_trusted"`
		GPS     *struct {
			State   string `json:"state"`
			Quality uint8  `json:"fix_quality"`
		} `json:"gps"`
	} `json:"heartbeat"`
}

func (u *Updater) status() (EngineStatus, error) {
	var s EngineStatus
	b, e := readBounded(u.cfg.StatusFile, 1<<20)
	if e != nil {
		return s, e
	}
	if e = json.Unmarshal(b, &s); e != nil {
		return s, e
	}
	now := u.now()
	if s.Schema != 1 || s.Interval == 0 || s.Interval > uint64((1<<63-1)/int64(time.Second)/3) || s.Written.IsZero() || s.Written.After(now) {
		return s, errors.New("invalid status time")
	}
	if now.Sub(s.Written) > time.Duration(s.Interval)*3*time.Second {
		return s, errors.New("stale status")
	}
	h := s.Heartbeat
	if h.Node != u.cfg.Node || !digestRE.MatchString(h.Digest) || !versionRE.MatchString(h.Version) || h.Build == 0 || (h.Trusted != nil && !*h.Trusted) {
		return s, errors.New("unverified running identity or clock")
	}
	return s, nil
}

type Image struct {
	ID          string   `json:"Id"`
	Digest      string   `json:"Digest"`
	RepoDigests []string `json:"RepoDigests"`
}

func (u *Updater) image(ref string) (Image, error) {
	var images []Image
	b, e := u.run.Run("podman", "image", "inspect", ref)
	if e != nil {
		return Image{}, e
	}
	if e = json.Unmarshal(b, &images); e != nil || len(images) != 1 {
		return Image{}, errors.New("invalid image inspection")
	}
	im := images[0]
	im.ID = strings.TrimPrefix(im.ID, "sha256:")
	if !digestRE.MatchString(im.Digest) || !imageIDRE.MatchString(im.ID) {
		return Image{}, errors.New("unknown image identity")
	}
	return im, nil
}
func (u *Updater) current() (Image, error) {
	b, e := u.run.Run("podman", "inspect", "--type", "container", u.cfg.Container)
	if e != nil {
		return Image{}, e
	}
	var c []struct {
		Image string
		State struct{ Running bool }
	}
	if json.Unmarshal(b, &c) != nil || len(c) != 1 || !imageIDRE.MatchString(strings.TrimPrefix(c[0].Image, "sha256:")) || !c[0].State.Running {
		return Image{}, errors.New("container image unavailable")
	}
	return u.image(c[0].Image)
}
func contains(list []string, s string) bool {
	for _, v := range list {
		if v == s {
			return true
		}
	}
	return false
}
func targetValid(target string) bool {
	return strings.HasPrefix(target, repository+"@") && digestRE.MatchString(strings.TrimPrefix(target, repository+"@"))
}

func (u *Updater) releaseRequest(raw []byte) (Request, error) {
	if e := u.trustedClock(); e != nil {
		return Request{}, e
	}
	m, e := verifyRelease(raw, u.keys, u.now())
	if e != nil {
		return Request{}, e
	}
	if !m.eligible(u.cfg.Node, u.cfg.Ring) {
		return Request{}, errors.New("release audience/cohort excluded")
	}
	hash := contentHash(m.canonical())
	if m.Sequence < u.state.Seen.Sequence || (m.Sequence == u.state.Seen.Sequence && u.state.Seen.Hash != "" && hash != u.state.Seen.Hash) {
		return Request{}, errors.New("release replay/equivocation")
	}
	return Request{Target: m.Repository + "@" + m.Digest, Version: m.Version, Build: m.BuildSeq, Hash: contentHash(raw), Release: &m}, nil
}
func (u *Updater) poll() error {
	if e := u.trustedClock(); e != nil {
		return e
	}
	if u.now().Before(u.state.NextPoll) {
		return nil
	} // D44 poll floor, including hints/reboots
	var entropy [8]byte
	if _, e := rand.Read(entropy[:]); e != nil {
		return e
	}
	u.state.NextPoll = u.now().Add(pollFloor + time.Duration(binary.BigEndian.Uint64(entropy[:])%300)*time.Second)
	if e := u.save(); e != nil {
		return e
	}
	endpoint, e := url.Parse(strings.TrimRight(u.cfg.BaseURL, "/") + "/" + u.cfg.Ring + "/release.json")
	if e != nil || endpoint.Scheme != "https" || endpoint.Host == "" || endpoint.User != nil || endpoint.RawQuery != "" || endpoint.Fragment != "" {
		return errors.New("credential-free HTTPS release URL required")
	}
	res, e := u.client.Get(endpoint.String())
	if e != nil {
		return e
	}
	defer res.Body.Close()
	if res.StatusCode != 200 {
		return fmt.Errorf("release HTTP %d", res.StatusCode)
	}
	raw, e := io.ReadAll(io.LimitReader(res.Body, maxDocument+1))
	if e != nil {
		return e
	}
	m, e := verifyRelease(raw, u.keys, u.now())
	if e != nil {
		return e
	}
	hash := contentHash(m.canonical())
	if m.Ring != u.cfg.Ring || m.Sequence < u.state.Seen.Sequence || (m.Sequence == u.state.Seen.Sequence && u.state.Seen.Hash != "" && hash != u.state.Seen.Hash) {
		return errors.New("release ring/replay/equivocation refused")
	}
	u.state.Seen = Seen{m.Sequence, hash}
	u.state.LastReleaseCheck = u.now()
	if e = u.save(); e != nil {
		return e
	}
	if !m.eligible(u.cfg.Node, u.cfg.Ring) {
		return nil
	}
	s, e := u.status()
	if e != nil {
		return e
	}
	im, e := u.current()
	if e != nil {
		return e
	}
	if im.Digest != s.Heartbeat.Digest {
		return errors.New("status does not match running container")
	}
	console, e := u.consoleCurrent()
	if e != nil {
		return e
	}
	if e = u.checkFloors(m, s, im, console); e != nil {
		return e
	}
	target := packageTarget(m)
	if u.state.Quarantined[target] {
		return u.outcome("quarantined", target, u.state.Attempts[target])
	}
	if contains(im.RepoDigests, repository+"@"+m.Digest) &&
		contains(console.RepoDigests, consoleRepository+"@"+m.ConsoleDigest) &&
		u.state.RunningSequence == m.Sequence {
		return nil
	}
	if _, e = os.Lstat(u.pending()); e == nil {
		return nil
	} else if !os.IsNotExist(e) {
		return e
	}
	if e = stage(u.pending(), raw); e != nil {
		return e
	}
	u.point("staged")
	return nil
}

// No expiry-sensitive decision is allowed without an explicit fresh clock-trust
// report. Recovery deliberately does not call this: old pins restore offline.
func (u *Updater) trustedClock() error {
	s, e := u.status()
	if e != nil || s.Heartbeat.Trusted == nil || !*s.Heartbeat.Trusted {
		return errors.New("release clock untrusted: require fresh explicit os_clock_trusted=true")
	}
	return nil
}

// Called under the root-private lock. Quarantine by rename, never follow a
// symlink or read a FIFO. Keep the rejected entry inert for local support and
// give the next poll a free slot, including after an expired staged release.
func (u *Updater) preparePending() (bool, error) {
	st, e := os.Lstat(u.pending())
	if os.IsNotExist(e) {
		return false, nil
	}
	if e != nil {
		return false, e
	}
	reason := "non-regular pending entry"
	if st.Mode().IsRegular() {
		raw, err := readBounded(u.pending(), maxDocument)
		if err == nil {
			// A clock outage is not evidence that a staged manifest is invalid.
			if err = u.trustedClock(); err != nil {
				return false, err
			}
			if _, err = u.releaseRequest(raw); err == nil {
				return true, nil
			}
			reason = "pending signature/schema/audience/replay/expiry verification refused"
		} else {
			reason = "pending read/size verification refused"
		}
	}
	dir, e := os.MkdirTemp(u.cfg.PrivateDir, "rejected-pending-")
	if e != nil {
		return false, e
	}
	if e = atomicJSON(filepath.Join(dir, "reason.json"), map[string]string{"reason": reason}, 0600); e != nil {
		return false, e
	}
	// Do not preserve an executable/symlink/FIFO in support-visible state.
	// A nonempty directory is moved intact without traversing it.
	if st.IsDir() {
		e = os.Rename(u.pending(), filepath.Join(dir, "entry"))
	} else {
		e = os.Remove(u.pending())
	}
	if e != nil {
		return false, e
	}
	for _, path := range []string{dir, u.cfg.PrivateDir} {
		f, err := os.Open(path)
		if err != nil {
			return false, err
		}
		err = f.Sync()
		f.Close()
		if err != nil {
			return false, err
		}
	}
	return false, nil
}

// Blue update requests are retired. Only the signed release domain is accepted.
func (u *Updater) request(raw []byte, _ Image) (Request, error) {
	return u.releaseRequest(raw)
}

func (u *Updater) clearPending(hash string) error {
	b, e := readBounded(u.pending(), maxDocument)
	if os.IsNotExist(e) {
		return nil
	}
	if e != nil {
		return e
	}
	if contentHash(b) == hash {
		return removeOptional(u.pending())
	}
	return nil
}
func (u *Updater) outcome(kind, target string, attempts int) error {
	u.state.Outcome = kind
	if e := u.save(); e != nil {
		return e
	}
	return atomicJSON(filepath.Join(u.cfg.StateDir, "update_outcome.json"), map[string]any{"schema": "cybrrd.update.outcome.v1", "node_id": u.cfg.Node, "kind": kind, "target": target, "attempts": attempts, "observed_unix_ms": u.now().UnixMilli()}, 0644)
}
func (u *Updater) healthy(digest, version string, build uint64, since time.Time) bool {
	deadline := u.now().Add(u.healthTimeout)
	var first time.Time
	for u.now().Before(deadline) {
		if !u.stableWindow() {
			return false
		}
		s, e := u.status()
		h := s.Heartbeat
		ok := e == nil && s.Written.After(since) && h.Digest == digest && h.Build == build && (version == "" || h.Version == version) && h.Radio == "up" && (!u.cfg.GPSRequired || (h.GPS != nil && h.GPS.State == "healthy" && h.GPS.Quality > 0 && h.Trusted != nil && *h.Trusted))
		if ok {
			if !first.IsZero() && s.Written.After(first) {
				return true
			}
			first = s.Written
		}
		u.sleep(2 * time.Second)
	}
	return false
}
func (u *Updater) restart() error {
	if _, e := u.run.Run("systemctl", "daemon-reload"); e != nil {
		return e
	}
	if _, e := u.run.Run("systemctl", "reset-failed", u.cfg.EngineUnit); e != nil {
		return e
	}
	_, e := u.run.Run("systemctl", "restart", u.cfg.EngineUnit)
	return e
}
func repin(raw []byte, target string) ([]byte, error) {
	lines := strings.Split(string(raw), "\n")
	n := 0
	for i, l := range lines {
		if strings.HasPrefix(l, "Image=") {
			lines[i] = "Image=" + target
			n++
		}
	}
	if n != 1 {
		return nil, errors.New("Quadlet must contain exactly one Image pin")
	}
	return []byte(strings.Join(lines, "\n")), nil
}
func previousPin(raw []byte) (string, error) {
	for _, l := range strings.Split(string(raw), "\n") {
		if strings.HasPrefix(l, "Image=") {
			p := strings.TrimPrefix(l, "Image=")
			if targetValid(p) {
				return p, nil
			}
		}
	}
	return "", errors.New("previous Quadlet is not digest-pinned")
}

func removeOptional(path string) error {
	e := os.Remove(path)
	if os.IsNotExist(e) {
		return nil
	}
	if e != nil {
		return e
	}
	d, e := os.Open(filepath.Dir(path))
	if e != nil {
		return e
	}
	defer d.Close()
	return d.Sync()
}
