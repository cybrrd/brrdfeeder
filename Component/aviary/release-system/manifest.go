// SPDX-License-Identifier: AGPL-3.0-or-later
package main

import (
	"bytes"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"regexp"
	"time"
)

const repository = "ghcr.io/cybrrd/brrdfeeder"
const consoleRepository = "ghcr.io/cybrrd/brrdhouse"
const releaseSchema = "cybrrd.release.v1"
const maxDocument = 16384

var digestRE = regexp.MustCompile(`^sha256:[0-9a-f]{64}$`)
var imageIDRE = regexp.MustCompile(`^[0-9a-f]{64}$`)
var versionRE = regexp.MustCompile(`^[0-9a-f]{40}$`)
var tokenRE = regexp.MustCompile(`^[A-Za-z0-9_-]{1,64}$`)

// Same public trust root as blue_policy::KEYRING. No runtime key override.
// A source-contract test prevents drift; rotation is a reviewed image change.
var pinnedKeys = []ed25519.PublicKey{mustHex("1d75eca3b0452b54a530dfdf8ba4265c1c18e9f09e718db70ea19cabe4712f1d")}

func mustHex(s string) []byte {
	b, err := hex.DecodeString(s)
	if err != nil {
		panic(err)
	}
	return b
}

type Release struct {
	Schema         string `json:"schema"`
	Ring           string `json:"ring"`
	Audience       string `json:"audience"`
	Repository     string `json:"repository"`
	Digest         string `json:"digest"`
	Version        string `json:"version"`
	BuildSeq       uint64 `json:"build_seq"`
	ConsoleDigest  string `json:"console_digest"`
	ConsoleVersion string `json:"console_version"`
	ConsoleBuild   uint64 `json:"console_build_seq"`
	Sequence       uint64 `json:"sequence"`
	Rollout        uint32 `json:"rollout_pct"`
	Salt           string `json:"salt"`
	Published      string `json:"published_at"`
	Expires        string `json:"expires_at"`
	Signature      string `json:"sig"`
}

// An array fixes order/types, JSON escapes delimiters unambiguously, and the
// leading schema separates this signature domain from Blue's || byte string.
func (m Release) canonical() []byte {
	b, _ := json.Marshal([]any{m.Schema, m.Ring, m.Audience, m.Repository, m.Digest, m.Version, m.BuildSeq, m.ConsoleDigest, m.ConsoleVersion, m.ConsoleBuild, m.Sequence, m.Rollout, m.Salt, m.Published, m.Expires})
	return b
}
func audienceFor(ring string) string {
	switch ring {
	case "dev", "staging", "general":
		return "configured-ring:" + ring
	}
	return ""
}
func cohort(node, salt string) uint32 {
	b, _ := json.Marshal([]string{node, salt})
	sum := sha256.Sum256(b)
	return uint32(binary.BigEndian.Uint64(sum[:8]) % 100)
}
func (m Release) valid(now time.Time) error {
	if m.Schema != releaseSchema || m.Repository != repository || !digestRE.MatchString(m.Digest) || !digestRE.MatchString(m.ConsoleDigest) { // D44 MUTATE target
		return errors.New("release requires the approved repository and a sha256 digest, never a tag")
	}
	if audienceFor(m.Ring) == "" || m.Audience != audienceFor(m.Ring) || !versionRE.MatchString(m.Version) || !versionRE.MatchString(m.ConsoleVersion) || m.ConsoleBuild == 0 || m.BuildSeq == 0 || m.Sequence == 0 || !tokenRE.MatchString(m.Salt) || m.Rollout > 100 || (m.Ring != "general" && m.Rollout != 100) {
		return errors.New("invalid release policy")
	}
	p, e := time.Parse(time.RFC3339, m.Published)
	x, xe := time.Parse(time.RFC3339, m.Expires)
	if e != nil || xe != nil || p.After(now) || !x.After(now) || !x.After(p) || x.Sub(p) > 366*24*time.Hour {
		return errors.New("release time outside signed validity")
	}
	return nil
}
func verifyRelease(raw []byte, keys []ed25519.PublicKey, now time.Time) (Release, error) {
	var m Release
	if err := strictJSON(raw, &m); err != nil {
		return m, err
	}
	sig, err := hex.DecodeString(m.Signature)
	if err != nil {
		return m, errors.New("invalid signature encoding")
	}
	verified := false
	for _, key := range keys {
		if ed25519.Verify(key, m.canonical(), sig) {
			verified = true
		}
	}
	if !verified { // D44 MUTATE signature (wrong-key and tamper independently)
		return m, errors.New("release signature rejected by pinned keyring")
	}
	return m, m.valid(now)
}
func (m Release) eligible(node, ring string) bool {
	if !tokenRE.MatchString(node) || m.Ring != ring || m.Audience != audienceFor(m.Ring) {
		return false
	}
	return m.Ring != "general" || cohort(node, m.Salt) < m.Rollout // D44 MUTATE cohort
}
func contentHash(b []byte) string { s := sha256.Sum256(b); return hex.EncodeToString(s[:]) }

// Reject duplicate keys at every object depth, unknown fields, trailing JSON,
// and oversized inputs. Signature verification never has parser ambiguity.
func strictJSON(raw []byte, dst any) error {
	return strictJSONLimit(raw, dst, maxDocument)
}
func strictJSONLimit(raw []byte, dst any, limit int) error {
	if len(raw) > limit {
		return errors.New("document too large")
	}
	d := json.NewDecoder(bytes.NewReader(raw))
	var walk func() error
	walk = func() error {
		t, e := d.Token()
		if e != nil {
			return e
		}
		if delim, ok := t.(json.Delim); ok {
			switch delim {
			case '{':
				seen := map[string]bool{}
				for d.More() {
					k, e := d.Token()
					if e != nil {
						return e
					}
					s, ok := k.(string)
					if !ok || seen[s] {
						return errors.New("duplicate JSON key")
					}
					seen[s] = true
					if e := walk(); e != nil {
						return e
					}
				}
			case '[':
				for d.More() {
					if e := walk(); e != nil {
						return e
					}
				}
			default:
				return errors.New("invalid JSON delimiter")
			}
			_, e = d.Token()
			return e
		}
		return nil
	}
	if e := walk(); e != nil {
		return e
	}
	if _, e := d.Token(); e != io.EOF {
		return errors.New("trailing JSON")
	}
	d = json.NewDecoder(bytes.NewReader(raw))
	d.DisallowUnknownFields()
	if e := d.Decode(dst); e != nil {
		return fmt.Errorf("invalid JSON: %w", e)
	}
	return nil
}
