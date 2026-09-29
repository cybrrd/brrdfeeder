// SPDX-License-Identifier: AGPL-3.0-or-later
package main

import (
	"crypto/ed25519"
	"embed"
	"errors"
	"flag"
	"fmt"
	"net/http"
	"net/netip"
	"os"
	"os/user"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"
)

//go:embed units/*
var units embed.FS

//go:embed launcher.py
var launcher []byte

func main() {
	if e := cli(os.Args[1:]); e != nil {
		fmt.Fprintln(os.Stderr, "brrdfeeder-release:", e)
		os.Exit(1)
	}
}
func cli(args []string) error {
	if len(args) == 0 {
		return errors.New("usage: brrdfeeder-release publish|poll|apply|recover|install")
	}
	if args[0] == "publish" {
		return publish(args[1:])
	}
	if args[0] == "publish-updater" {
		return publishHost(args[1:])
	}
	if len(args) == 1 && args[0] == "self-test" {
		fmt.Print(selfTestReply)
		return nil
	}
	if len(args) == 1 && args[0] == "build-seq" {
		n, e := strconv.ParseUint(updaterBuild, 10, 64)
		if e != nil || n == 0 {
			return errors.New("invalid compiled updater build")
		}
		fmt.Println(n)
		return nil
	}
	// A zero-capability, networkless rollback anchor keeps the outgoing image
	// in use even during system prune. It never reads state or runs the engine.
	if len(args) == 1 && args[0] == "hold" {
		for {
			time.Sleep(time.Hour)
		}
	}
	f := flag.NewFlagSet(args[0], flag.ContinueOnError)
	path := f.String("config", "/etc/brrdfeeder/updater.json", "root-owned installed updater configuration")
	if e := f.Parse(args[1:]); e != nil {
		return e
	}
	if f.NArg() != 0 {
		return errors.New("unexpected arguments")
	}
	if os.Geteuid() != 0 {
		return errors.New("host commands require root")
	}
	if args[0] == "install" {
		return install(*path)
	}
	c := defaults()
	b, e := readBounded(*path, maxDocument)
	if e != nil {
		return e
	}
	st, e := os.Lstat(*path)
	if e != nil || st.Mode().Perm()&0022 != 0 || st.Sys().(*syscall.Stat_t).Uid != 0 {
		return errors.New("updater config must not be group/world writable")
	}
	if e = strictJSON(b, &c); e != nil {
		return e
	}
	if e = validateConfig(c); e != nil {
		return e
	}
	for _, p := range []string{c.StateDir, c.PrivateDir, c.Quadlet} {
		if !filepath.IsAbs(p) {
			return errors.New("absolute host paths required")
		}
	}
	return newUpdater(c).execute(args[0])
}
func publish(args []string) error {
	return publishWith(args, nil, pinnedKeys)
}
func publishWith(args []string, client *http.Client, keys []ed25519.PublicKey) error {
	f := flag.NewFlagSet("publish", flag.ContinueOnError)
	signer := f.String("signer-url", "", "HTTPS S5 /sign/release endpoint; private key stays on signer")
	ca := f.String("signer-ca", "", "optional signer CA PEM")
	token := f.String("signer-token-file", "", "optional bearer-token file (never a key)")
	out := f.String("out", "release.json", "output file; no upload is performed")
	ring := f.String("ring", "", "dev, staging, general")
	digest := f.String("digest", "", "sha256 digest, never a tag")
	version := f.String("version", "", "engine compiler-bound 40-hex revision")
	build := f.Uint64("build-seq", 0, "monotonic engine build sequence")
	consoleDigest := f.String("console-digest", "", "BRRDhouse sha256 digest, never a tag")
	consoleVersion := f.String("console-version", "", "BRRDhouse 40-hex source revision")
	consoleBuild := f.Uint64("console-build-seq", 0, "monotonic BRRDhouse build sequence")
	seq := f.Uint64("sequence", 0, "strictly increasing per-ring release sequence")
	pct := f.Uint("rollout-pct", 100, "general-only percentage")
	salt := f.String("salt", "", "stable cohort salt; preserve while raising percentage")
	published := f.String("published-at", time.Now().UTC().Format(time.RFC3339), "RFC3339 publication time")
	expires := f.String("expires-at", time.Now().UTC().Add(30*24*time.Hour).Format(time.RFC3339), "RFC3339 expiry; renew metadata before expiry")
	if e := f.Parse(args); e != nil {
		return e
	}
	if f.NArg() != 0 || *pct > 100 {
		return errors.New("invalid publisher arguments")
	}
	m := Release{Schema: releaseSchema, Ring: *ring, Audience: audienceFor(*ring), Repository: repository, Digest: *digest, Version: *version, BuildSeq: *build, Sequence: *seq, Rollout: uint32(*pct), Salt: *salt, Published: *published, Expires: *expires}
	m.ConsoleDigest, m.ConsoleVersion, m.ConsoleBuild = *consoleDigest, *consoleVersion, *consoleBuild
	if e := m.valid(time.Now()); e != nil {
		return e
	}
	var e error
	if client == nil {
		client, e = releaseSignerClient(*ca)
		if e != nil {
			return e
		}
	}
	m, e = signReleaseRemote(m, *signer, *token, client, keys)
	if e != nil {
		return e
	}
	return atomicJSON(*out, m, 0644)
}
func install(path string) error {
	// Consumes only the engine's validated nonsecret update-config projection.
	var info struct {
		Node         string `json:"node_id"`
		GPS          bool   `json:"gps_required"`
		Status       string `json:"status_file"`
		Upward       bool   `json:"upward_enabled"`
		Ring         string `json:"ring"`
		ConsoleUID   uint32 `json:"console_uid"`
		ConsoleURL   string `json:"console_url"`
		ConsoleBuild uint64 `json:"console_build_seq"`
	}
	b, e := readBounded(path, maxDocument)
	if e != nil {
		return e
	}
	if e = strictJSON(b, &info); e != nil {
		return e
	}
	if !tokenRE.MatchString(info.Node) || info.Status != "/var/lib/brrdfeeder-status/status.json" || !info.Upward {
		return errors.New("updates require the provisioned status directory and enabled D40 upward lane")
	}
	c := defaults()
	c.Node = info.Node
	c.GPSRequired = info.GPS
	c.Ring, c.ConsoleUID, c.ConsoleURL, c.ConsoleBuild = info.Ring, info.ConsoleUID, info.ConsoleURL, info.ConsoleBuild
	if e = validateConfig(c); e != nil {
		return e
	}
	if e = os.MkdirAll(c.PrivateDir, 0700); e != nil {
		return e
	}
	if e = atomicJSON("/etc/brrdfeeder/updater.json", c, 0600); e != nil {
		return e
	}
	if e = atomicFile("/usr/local/libexec/brrdfeeder-release-launch", launcher, 0755); e != nil {
		return e
	}
	entries, e := units.ReadDir("units")
	if e != nil {
		return e
	}
	for _, entry := range entries {
		if entry.Name() == "brrdfeeder-updater.path" {
			continue
		} // retired, never install a path watcher
		b, e := units.ReadFile("units/" + entry.Name())
		if e != nil {
			return e
		}
		target := "/etc/systemd/system/" + entry.Name()
		mode := os.FileMode(0644)
		if entry.Name() == "brrdfeeder-updater.sh" {
			target = "/usr/local/bin/" + entry.Name()
			mode = 0755
		}
		if e = atomicFile(target, b, mode); e != nil {
			return e
		}
	}
	_, e = (commands{}).Run("systemctl", "daemon-reload")
	if e != nil {
		return e
	}
	_, e = (commands{}).Run("systemctl", "enable", "--now", "brrdfeeder-release-recover.service", "brrdfeeder-release-poll.timer", "brrdfeeder-host-update.timer")
	return e
}

func validateConfig(c Config) error {
	if !tokenRE.MatchString(c.Node) || audienceFor(c.Ring) == "" || c.ConsoleUser != "brrdhouse" || c.ConsoleUID < 100 || c.ConsoleBuild == 0 {
		return errors.New("invalid node/ring/console identity or bootstrap console build floor")
	}
	account, e := user.Lookup(c.ConsoleUser)
	if e != nil || account.Uid != strconv.FormatUint(uint64(c.ConsoleUID), 10) {
		return errors.New("console UID does not match installed service user")
	}
	addr, e := netip.ParseAddrPort(strings.TrimPrefix(c.ConsoleURL, "http://"))
	if e != nil || !strings.HasPrefix(c.ConsoleURL, "http://") || addr.Port() == 0 || addr.Addr().IsUnspecified() || addr.Addr().IsMulticast() {
		return errors.New("console URL must use its literal HTTP listen address")
	}
	return nil
}
