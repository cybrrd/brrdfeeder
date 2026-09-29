// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"context"
	"crypto/ed25519"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"
)

const hostSchema = "cybrrd.host-updater.v1"
const hostBinary = "/usr/local/libexec/brrdfeeder-release"
const selfTestReply = "brrdfeeder-release self-test v1\n"
const maxHostBinary = 32 << 20

// Build/release pipeline sets -X main.updaterBuild; the initial reviewed host
// artifact is build 1. A blank journal never removes this embedded floor.
var updaterBuild = "1"

// A different signed domain AND explicit boolean prevent a package release
// from smuggling a host executable into an engine/console transaction.
type HostRelease struct {
	Schema       string `json:"schema"`
	UpdaterOnly  bool   `json:"updater_only"`
	Ring         string `json:"ring"`
	Audience     string `json:"audience"`
	SHA256       string `json:"sha256"`
	Architecture string `json:"architecture"`
	Build        uint64 `json:"build_seq"`
	Sequence     uint64 `json:"sequence"`
	Rollout      uint32 `json:"rollout_pct"`
	Salt         string `json:"salt"`
	Published    string `json:"published_at"`
	Expires      string `json:"expires_at"`
	Signature    string `json:"sig"`
}

func (m HostRelease) canonical() []byte {
	b, _ := json.Marshal([]any{m.Schema, m.UpdaterOnly, m.Ring, m.Audience, m.SHA256, m.Architecture, m.Build, m.Sequence, m.Rollout, m.Salt, m.Published, m.Expires})
	return b
}
func (m HostRelease) valid(now time.Time) error {
	p, pe := time.Parse(time.RFC3339, m.Published)
	x, xe := time.Parse(time.RFC3339, m.Expires)
	if m.Schema != hostSchema || !m.UpdaterOnly || !imageIDRE.MatchString(m.SHA256) || (m.Architecture != "arm64" && m.Architecture != "amd64") || m.Build == 0 || m.Sequence == 0 || audienceFor(m.Ring) == "" || m.Audience != audienceFor(m.Ring) || m.Rollout > 100 || (m.Ring != "general" && m.Rollout != 100) || !tokenRE.MatchString(m.Salt) || pe != nil || xe != nil || p.After(now) || !x.After(now) || !x.After(p) || x.Sub(p) > 366*24*time.Hour {
		return errors.New("invalid explicitly separate host-updater release")
	}
	return nil
}
func verifyHost(raw []byte, keys []ed25519.PublicKey, now time.Time) (HostRelease, error) {
	var m HostRelease
	if e := strictJSON(raw, &m); e != nil {
		return m, e
	}
	sig, e := hex.DecodeString(m.Signature)
	if e != nil {
		return m, e
	}
	ok := false
	for _, k := range keys {
		if ed25519.Verify(k, m.canonical(), sig) {
			ok = true
		}
	}
	if !ok {
		return m, errors.New("host-updater signature refused")
	}
	return m, m.valid(now)
}

type HostState struct {
	NextPoll  time.Time       `json:"next_poll"`
	Seen      Seen            `json:"seen"`
	Build     uint64          `json:"build_seq"`
	Attempted map[string]bool `json:"attempted"`
}
type HostTransaction struct {
	Previous  string `json:"previous_sha256"`
	Candidate string `json:"candidate_sha256"`
}

func hostSelfTest(path string) error {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	b, e := exec.CommandContext(ctx, path, "self-test").Output()
	if e != nil || string(b) != selfTestReply {
		return errors.New("host-updater candidate cannot start or has incompatible self-test")
	}
	return nil
}
func (u *Updater) getReleaseBytes(path string, limit int64) ([]byte, error) {
	endpoint, e := url.Parse(strings.TrimRight(u.cfg.BaseURL, "/") + "/" + path)
	if e != nil || endpoint.Scheme != "https" || endpoint.Host == "" || endpoint.User != nil || endpoint.RawQuery != "" || endpoint.Fragment != "" {
		return nil, errors.New("credential-free HTTPS required")
	}
	r, e := u.client.Get(endpoint.String())
	if e != nil {
		return nil, e
	}
	defer r.Body.Close()
	if r.StatusCode != 200 {
		return nil, errors.New("host-updater download unavailable")
	}
	b, e := io.ReadAll(io.LimitReader(r.Body, limit+1))
	if int64(len(b)) > limit {
		return nil, errors.New("host-updater download exceeds limit")
	}
	return b, e
}
func (u *Updater) pollHost() error {
	return u.pollHostAt(hostBinary, "/etc/brrdfeeder/.updater-helper.sha256")
}

// Paths are fixed above for the host CLI. Parameterization lets sandbox tests
// exercise the complete signed HTTPS -> executable flow without writing /usr.
func (u *Updater) pollHostAt(binary, receipt string) error {
	if e := u.trustedClock(); e != nil {
		return e
	}
	path := filepath.Join(u.cfg.PrivateDir, "host-state.json")
	s := HostState{Attempted: map[string]bool{}}
	if b, e := readBounded(path, maxDocument); e == nil {
		if e = strictJSON(b, &s); e != nil || s.Attempted == nil {
			return errors.New("invalid host-update state")
		}
	} else if !os.IsNotExist(e) {
		return e
	}
	if u.now().Before(s.NextPoll) {
		return nil
	}
	compiled, e := strconv.ParseUint(updaterBuild, 10, 64)
	if e != nil || compiled == 0 {
		return errors.New("invalid compiled host build floor")
	}
	s.Build = max(s.Build, compiled)
	// Independent durable floor; deterministic node jitter distributes rebooted units.
	s.NextPoll = u.now().Add(pollFloor + time.Duration(cohort(u.cfg.Node, "host-poll"))*3*time.Second)
	if e := atomicJSON(path, s, 0600); e != nil {
		return e
	}
	b, e := u.getReleaseBytes(u.cfg.Ring+"/updater.json", maxDocument)
	if e != nil {
		return e
	}
	m, e := verifyHost(b, u.keys, u.now())
	if e != nil {
		return e
	}
	hash := contentHash(m.canonical())
	if m.Ring != u.cfg.Ring || m.Architecture != runtime.GOARCH || m.Sequence < s.Seen.Sequence || (m.Sequence == s.Seen.Sequence && s.Seen.Hash != "" && s.Seen.Hash != hash) || m.Build < s.Build {
		return errors.New("host-updater ring/architecture/replay/downgrade refused")
	}
	s.Seen = Seen{m.Sequence, hash}
	if e = atomicJSON(path, s, 0600); e != nil {
		return e
	}
	if m.Build == s.Build || s.Attempted[m.SHA256] || (m.Ring == "general" && cohort(u.cfg.Node, m.Salt) >= m.Rollout) {
		return nil
	}
	if len(s.Attempted) >= 32 {
		return errors.New("host-updater attempt history full; review required")
	}
	b, e = u.getReleaseBytes("updater/"+m.SHA256+"/linux-"+m.Architecture+"/brrdfeeder-release", maxHostBinary)
	if e != nil {
		return e
	}
	if contentHash(b) != m.SHA256 {
		return errors.New("host-updater artifact hash refused")
	}
	// A slow download must not outlive the trusted-clock/expiry decision.
	if e = u.trustedClock(); e != nil {
		return e
	}
	if e = m.valid(u.now()); e != nil {
		return e
	}
	// One attempt per signed executable, even across new release metadata.
	s.Attempted[m.SHA256] = true
	if e = atomicJSON(path, s, 0600); e != nil {
		return e
	}
	probe := func(path string) error {
		if e := hostSelfTest(path); e != nil {
			return e
		}
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		identity, e := exec.CommandContext(ctx, path, "build-seq").Output()
		if e != nil || string(identity) != strconv.FormatUint(m.Build, 10)+"\n" {
			return errors.New("signed updater build differs from candidate identity")
		}
		return nil
	}
	if e = u.switchHost(binary, receipt, b, probe); e != nil {
		return e
	}
	s.Build = m.Build
	return atomicJSON(path, s, 0600)
}
func (u *Updater) switchHost(binary, receipt string, candidate []byte, test func(string) error) error {
	if u.state.Active != nil {
		return errors.New("host update cannot share a package transaction")
	}
	old, e := readBounded(binary, maxHostBinary)
	if e != nil {
		return e
	}
	previous := binary + ".previous"
	if e = atomicFile(previous, old, 0755); e != nil {
		return e
	}
	if e = atomicFile(previous+".sha256", []byte(contentHash(old)+"\n"), 0600); e != nil {
		return e
	}
	tx := filepath.Join(u.cfg.PrivateDir, "host-transaction.json")
	if e = atomicJSON(tx, HostTransaction{contentHash(old), contentHash(candidate)}, 0600); e != nil {
		return e
	}
	u.point("host-prepared")
	if e = atomicFile(binary, candidate, 0755); e != nil {
		return e
	}
	u.point("host-switched")
	if e = test(binary); e != nil {
		if re := atomicFile(binary, old, 0755); re != nil {
			return re
		}
		if re := removeOptional(tx); re != nil {
			return re
		}
		return e
	}
	u.point("host-started")
	if e = atomicFile(receipt, []byte(contentHash(candidate)+"  "+binary+"\n"), 0600); e != nil {
		return e
	}
	return removeOptional(tx)
}
func publishHost(args []string) error {
	f := flag.NewFlagSet("publish-updater", flag.ContinueOnError)
	out := f.String("out", "updater.unsigned.json", "UNSIGNED preparation only; S5 host-signing approval required")
	m := HostRelease{Schema: hostSchema}
	f.BoolVar(&m.UpdaterOnly, "updater-only", false, "REQUIRED: explicitly authorize host-only release")
	f.StringVar(&m.Ring, "ring", "", "dev/staging/general")
	f.StringVar(&m.SHA256, "sha256", "", "standalone executable SHA256")
	f.StringVar(&m.Architecture, "architecture", "arm64", "host architecture")
	f.Uint64Var(&m.Build, "build-seq", 0, "monotonic updater build")
	f.Uint64Var(&m.Sequence, "sequence", 0, "monotonic independent host sequence")
	pct := f.Uint("rollout-pct", 100, "general cohort percentage")
	f.StringVar(&m.Salt, "salt", "", "cohort salt")
	f.StringVar(&m.Published, "published-at", time.Now().UTC().Format(time.RFC3339), "publication")
	f.StringVar(&m.Expires, "expires-at", time.Now().UTC().Add(30*24*time.Hour).Format(time.RFC3339), "expiry")
	if e := f.Parse(args); e != nil {
		return e
	}
	if *pct > 100 || f.NArg() != 0 {
		return errors.New("invalid publisher args")
	}
	m.Rollout, m.Audience = uint32(*pct), audienceFor(m.Ring)
	if e := m.valid(time.Now()); e != nil {
		return e
	}
	// the release approver's Friday ruling: preparation only until a separately validated S5
	// host-artifact signing endpoint is approved. No caller ever loads the key.
	if e := atomicJSON(*out, m, 0644); e != nil {
		return e
	}
	fmt.Println("UNSIGNED host-updater metadata prepared; signing/publication requires separate S5 approval. Do not serve as updater.json.")
	return nil
}
