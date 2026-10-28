// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

// This opt-in, installer-authorized transaction has no HTTP client, enrollment,
// service management or workload writes. It is not the signed host channel.
import (
	"bytes"
	"context"
	"crypto/ed25519"
	"encoding/base32"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/user"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"time"
)

type localPaths struct {
	binary, receipt, config, bootstrap, active, updater, private, launch string
	owner, service                                                       uint32
	// Test roots only; production always verifies every parent back to /.
	boundary string
}

const localSystemCredentials = "/etc/brrdfeeder/secrets/brrdfeeder.creds"

func localHelperCLI(args []string) error {
	f := flag.NewFlagSet("upgrade-local-helper", flag.ContinueOnError)
	hash := f.String("sha256", "", "verified installer pin; installer use only")
	if e := f.Parse(args); e != nil {
		return e
	}
	if f.NArg() != 0 || os.Geteuid() != 0 || !imageIDRE.MatchString(*hash) {
		return errors.New("root and verified installer SHA256 required")
	}
	u, e := user.Lookup("brrdfeeder")
	if e != nil {
		return errors.New("existing service identity required")
	}
	uid, e := strconv.ParseUint(u.Uid, 10, 32)
	if e != nil {
		return e
	}
	self, e := os.Executable()
	if e != nil {
		return e
	}
	candidate, e := readBounded(self, maxHostBinary)
	if e != nil {
		return e
	}
	p := localPaths{binary: hostBinary, receipt: "/etc/brrdfeeder/.updater-helper.sha256",
		config: "/etc/brrdfeeder/config.yaml", bootstrap: localSystemCredentials,
		active: "/var/lib/brrdfeeder/credentials/brrdfeeder.creds", updater: "/etc/brrdfeeder/updater.json",
		private: "/var/lib/brrdfeeder-updater", launch: "/usr/local/libexec/brrdfeeder-release-launch",
		service: uint32(uid), boundary: "/"}
	if e = upgradeLocal(p, candidate, *hash, probeLocal, func(string) {}); e != nil {
		return e
	}
	fmt.Println("Helper build 4 verified; identity, credentials and workload state preserved. No enrollment performed.")
	return nil
}

func (p localPaths) read(path string, owner uint32, mask os.FileMode, limit int64) ([]byte, error) {
	for parent := filepath.Dir(path); ; parent = filepath.Dir(parent) {
		st, e := os.Lstat(parent)
		if e != nil {
			return nil, e
		}
		uid := st.Sys().(*syscall.Stat_t).Uid
		if !st.IsDir() || st.Mode()&os.ModeSymlink != 0 || st.Mode().Perm()&0022 != 0 || (uid != p.owner && uid != owner) {
			return nil, errors.New("unsafe helper-upgrade parent")
		}
		if parent == p.boundary || parent == "/" {
			break
		}
	}
	f, e := os.OpenFile(path, os.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
	if e != nil {
		return nil, e
	}
	defer f.Close()
	st, e := f.Stat()
	if e != nil {
		return nil, e
	}
	s := st.Sys().(*syscall.Stat_t)
	if !st.Mode().IsRegular() || s.Uid != owner || s.Nlink != 1 || st.Mode().Perm()&mask != 0 || st.Size() > limit {
		return nil, errors.New("unsafe helper-upgrade file")
	}
	// Read the checked descriptor, not the pathname again.
	b, e := io.ReadAll(io.LimitReader(f, limit+1))
	if e != nil || int64(len(b)) != st.Size() || len(b) == 0 {
		return nil, errors.New("helper-upgrade file changed during read or empty")
	}
	return b, nil
}

func probeLocal(path, verb string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	b, e := exec.CommandContext(ctx, path, verb).Output()
	return string(b), e
}

func nkey(s string, prefix byte) ([]byte, error) {
	b, e := base32.StdEncoding.WithPadding(base32.NoPadding).DecodeString(s)
	if e != nil || len(b) != 35 || b[0] != prefix || !nkeyCRC(b) {
		return nil, errors.New("invalid credential key")
	}
	return b[1:33], nil
}
func nkeyCRC(b []byte) bool {
	var crc uint16
	for _, v := range b[:len(b)-2] {
		crc ^= uint16(v) << 8
		for i := 0; i < 8; i++ {
			if crc&0x8000 != 0 {
				crc = (crc << 1) ^ 0x1021
			} else {
				crc <<= 1
			}
		}
	}
	return binary.LittleEndian.Uint16(b[len(b)-2:]) == crc
}

type localTrust struct{ issuer, account string }

func credentialTrust(raw []byte, node string) (localTrust, error) {
	bad := errors.New("invalid or inconsistent local credentials")
	part := func(begin, end string) string {
		s := string(raw)
		if strings.Count(s, begin) != 1 || strings.Count(s, end) != 1 {
			return ""
		}
		_, s, _ = strings.Cut(s, begin)
		s, _, _ = strings.Cut(s, end)
		return strings.TrimSpace(s)
	}
	token := part("-----BEGIN NATS USER JWT-----", "------END NATS USER JWT------")
	seed := part("-----BEGIN USER NKEY SEED-----", "------END USER NKEY SEED------")
	parts := strings.Split(token, ".")
	if len(parts) != 3 {
		return localTrust{}, bad
	}
	decode := func(s string) []byte { b, _ := base64.RawURLEncoding.DecodeString(s); return b }
	var h struct{ Typ, Alg string }
	var c struct {
		Sub, Iss, Name string
		Nats           struct {
			Type    string
			Version int
			Account string `json:"issuer_account"`
		}
	}
	if json.Unmarshal(decode(parts[0]), &h) != nil || json.Unmarshal(decode(parts[1]), &c) != nil || h.Typ != "JWT" || h.Alg != "ed25519-nkey" || c.Name != node || c.Nats.Type != "user" || c.Nats.Version != 2 {
		return localTrust{}, bad
	}
	sub, e := nkey(c.Sub, 160)
	if e != nil {
		return localTrust{}, bad
	}
	issuer, e := nkey(c.Iss, 0)
	if e != nil {
		return localTrust{}, bad
	}
	if _, e = nkey(c.Nats.Account, 0); e != nil {
		return localTrust{}, bad
	}
	if !ed25519.Verify(issuer, []byte(parts[0]+"."+parts[1]), decode(parts[2])) {
		return localTrust{}, bad
	}
	sb, e := base32.StdEncoding.WithPadding(base32.NoPadding).DecodeString(seed)
	if e != nil || len(sb) != 36 || sb[0] != 149 || sb[1] != 0 || !nkeyCRC(sb) {
		return localTrust{}, bad
	}
	key := ed25519.NewKeyFromSeed(sb[2:34])
	if !bytes.Equal(key[32:], sub) {
		return localTrust{}, bad
	}
	// Expiry is deliberately not an enrollment trigger. A helper repair must
	// preserve even expired but intact credentials for explicit owner recovery.
	return localTrust{c.Iss, c.Nats.Account}, nil
}

type localSnapshot struct {
	old, receipt, floor []byte
	state               map[string]json.RawMessage
	build               uint64
}

func localPreflight(p localPaths, candidate []byte, hash string, probe func(string, string) (string, error)) (localSnapshot, error) {
	s := localSnapshot{}
	if contentHash(candidate) != hash {
		return s, errors.New("installer candidate SHA256 mismatch")
	}
	machine := uint16(183)
	if runtime.GOARCH == "amd64" {
		machine = 62
	}
	if len(candidate) < 20 || string(candidate[:4]) != "\x7fELF" || candidate[4] != 2 || candidate[5] != 1 || binary.LittleEndian.Uint16(candidate[18:20]) != machine { // LOCAL MUTATE architecture
		return s, errors.New("wrong helper architecture")
	}
	config, e := p.read(p.config, p.owner, 0022, maxDocument)
	if e != nil {
		return s, e
	}
	// The installed product already requires PyYAML. Isolated interpreter,
	// bounded input, no config writes and no raw YAML/credential diagnostics.
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "python3", "-I", "-c", `import json,sys,yaml
class Unique(yaml.SafeLoader): pass
def mapping(loader,node):
    pairs=loader.construct_pairs(node)
    if len(dict(pairs))!=len(pairs): raise ValueError()
    return dict(pairs)
Unique.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG,mapping)
c=yaml.load(sys.stdin.buffer.read(),Loader=Unique)
node=c.get('node',{}).get('id','')
if not isinstance(node,str): raise ValueError()
configured=c.get('backhaul',{}).get('credentials_path','')
if not isinstance(configured,str): raise ValueError()
print(json.dumps({'node_id':node,'credentials_path':configured}))`)
	cmd.Stdin = bytes.NewReader(config)
	nodeRaw, e := cmd.Output()
	if e != nil {
		return s, errors.New("invalid existing config")
	}
	var identity struct {
		Node        string `json:"node_id"`
		Credentials string `json:"credentials_path"`
	}
	if strictJSON(nodeRaw, &identity) != nil {
		return s, errors.New("invalid existing config")
	}
	node := identity.Node
	if !tokenRE.MatchString(node) {
		return s, errors.New("missing or invalid node ID")
	} // LOCAL MUTATE node
	b, e := p.read(p.updater, p.owner, 0022, maxDocument)
	if e != nil {
		return s, e
	}
	var cfg Config
	if strictJSON(b, &cfg) != nil || cfg.Node != node || cfg.PrivateDir != p.private {
		return s, errors.New("inconsistent updater identity or private directory")
	}
	bootstrap, e := p.read(p.bootstrap, p.owner, 0027, 32768)
	if e != nil {
		return s, errors.New("missing or unsafe bootstrap credentials")
	} // LOCAL MUTATE bootstrap
	activePath, activeOwner, activeMask := p.active, p.service, os.FileMode(0077)
	// 0.8.31 uses the root-owned system credential directly. Only an absent
	// runtime pathname and that exact configured path permit this fallback.
	// Lstat keeps dangling symlinks on the strict runtime-reader path.
	if _, err := os.Lstat(p.active); os.IsNotExist(err) && identity.Credentials == localSystemCredentials {
		activePath, activeOwner, activeMask = p.bootstrap, p.owner, 0027
	}
	active, e := p.read(activePath, activeOwner, activeMask, 32768)
	if e != nil {
		return s, errors.New("missing or unsafe active credentials")
	}
	t, e := credentialTrust(bootstrap, node)
	if e != nil {
		return s, e
	}
	a, e := credentialTrust(active, node)
	if e != nil || a != t {
		return s, errors.New("active credentials do not match bootstrap trust")
	}
	s.old, e = p.read(p.binary, p.owner, 0022, maxHostBinary)
	if e != nil {
		return s, e
	}
	s.receipt, e = p.read(p.receipt, p.owner, 0077, 1024)
	if e != nil {
		return s, e
	}
	if string(s.receipt) != contentHash(s.old)+"  "+p.binary+"\n" {
		return s, errors.New("corrupt helper receipt")
	} // LOCAL MUTATE receipt
	if reply, e := probe(p.binary, "self-test"); e != nil || reply != selfTestReply {
		return s, errors.New("old helper self-test failed")
	}
	build, e := probe(p.binary, "build-seq")
	if e != nil {
		return s, errors.New("old helper build unavailable")
	}
	s.build, e = strconv.ParseUint(strings.TrimSuffix(build, "\n"), 10, 64)
	if e != nil || s.build == 0 {
		return s, errors.New("invalid old helper build")
	}
	st, e := os.Lstat(p.private)
	if e != nil || !st.IsDir() || st.Mode().Perm()&0077 != 0 || st.Sys().(*syscall.Stat_t).Uid != p.owner {
		return s, errors.New("unsafe private updater directory")
	}
	for _, name := range []string{"host-transaction.json", "pending_update.json"} {
		if _, e := os.Lstat(filepath.Join(p.private, name)); !os.IsNotExist(e) {
			return s, errors.New("pending updater work; recover or finish before helper upgrade")
		}
	}
	if state, e := p.read(filepath.Join(p.private, "state.json"), p.owner, 0077, 256*1024); e == nil {
		var v State
		if strictJSONLimit(state, &v, 256*1024) != nil || v.Active != nil {
			return s, errors.New("active or invalid workload transaction")
		}
	} else if !os.IsNotExist(e) {
		return s, e
	}
	s.state = map[string]json.RawMessage{}
	s.floor, e = p.read(filepath.Join(p.private, "host-state.json"), p.owner, 0077, maxDocument)
	if e == nil {
		if strictJSON(s.floor, &s.state) != nil || s.state == nil {
			return s, errors.New("invalid host floor")
		}
		var floor uint64
		if json.Unmarshal(s.state["build_seq"], &floor) != nil {
			return s, errors.New("invalid host build floor")
		}
		s.build = max(s.build, floor)
	} else if !os.IsNotExist(e) {
		return s, e
	}
	if s.build > 4 {
		return s, errors.New("helper downgrade refused")
	} // LOCAL MUTATE downgrade
	if s.build == 4 && contentHash(s.old) != hash {
		return s, errors.New("equal-build different-hash helper refused")
	} // LOCAL MUTATE equal
	return s, nil
}

type localJournal struct {
	Previous  string `json:"previous_sha256"`
	Candidate string `json:"candidate_sha256"`
	Local     bool   `json:"local_upgrade"`
	Floor     string `json:"previous_host_state"`
	FloorHash string `json:"previous_host_state_sha256"`
}

func upgradeLocal(p localPaths, candidate []byte, hash string, probe func(string, string) (string, error), point func(string)) error {
	// First pass is read-only: rejected prerequisites do not even create locks.
	s, e := localPreflight(p, candidate, hash, probe)
	if e != nil {
		return e
	}
	locks := []*os.File{}
	defer func() {
		for _, f := range locks {
			f.Close()
		}
	}()
	for _, name := range []string{"launch.lock", "lock"} {
		f, e := os.OpenFile(filepath.Join(p.private, name), os.O_CREATE|os.O_RDWR|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0600)
		if e != nil {
			return e
		}
		locks = append(locks, f)
		st, e := f.Stat()
		if e != nil || !st.Mode().IsRegular() || st.Mode().Perm()&0077 != 0 || st.Sys().(*syscall.Stat_t).Uid != p.owner {
			return errors.New("unsafe updater lock")
		}
		if e = syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); e != nil {
			return errors.New("updater or recovery is busy; retry later")
		}
	}
	s, e = localPreflight(p, candidate, hash, probe)
	if e != nil {
		return e
	}
	if contentHash(s.old) == hash {
		return nil
	}
	// Verify a staged candidate before installed-state mutation, then again
	// after replacement. No candidate argv can schedule an update or enroll.
	f, e := os.CreateTemp(filepath.Dir(p.binary), ".helper-candidate-*")
	if e != nil {
		return e
	}
	stage := f.Name()
	defer os.Remove(stage)
	if _, e = f.Write(candidate); e == nil {
		e = f.Chmod(0755)
	}
	ce := f.Close()
	if e != nil {
		return e
	}
	if ce != nil {
		return ce
	}
	check := func(path string) error {
		r, e := probe(path, "self-test")
		if e != nil || r != selfTestReply {
			return errors.New("candidate self-test failed")
		}
		r, e = probe(path, "build-seq")
		if e != nil || r != "4\n" {
			return errors.New("candidate build is not exactly 4")
		}
		return nil
	}
	if e = check(stage); e != nil {
		return e
	}
	// The explicit installer operation (not updater.json) may update the
	// stable launcher. This compatible version must be durable before its new
	// journal appears. Keep it on rollback so interrupted recovery stays safe.
	if e = atomicFile(p.launch, launcher, 0755); e != nil {
		return e
	}
	if e = atomicFile(p.binary+".previous", s.old, 0755); e != nil {
		return e
	}
	if e = atomicFile(p.binary+".previous.sha256", []byte(contentHash(s.old)+"\n"), 0600); e != nil {
		return e
	}
	tx := filepath.Join(p.private, "host-transaction.json")
	j := localJournal{contentHash(s.old), hash, true, base64.StdEncoding.EncodeToString(s.floor), contentHash(s.floor)}
	if e = atomicJSON(tx, j, 0600); e != nil {
		return e
	}
	rollback := func(cause error) error {
		if e := atomicFile(p.binary, s.old, 0755); e != nil {
			return e
		}
		if e := atomicFile(p.receipt, s.receipt, 0600); e != nil {
			return e
		}
		floor := filepath.Join(p.private, "host-state.json")
		if len(s.floor) > 0 {
			if e := atomicFile(floor, s.floor, 0600); e != nil {
				return e
			}
		} else if e := removeOptional(floor); e != nil {
			return e
		}
		if e := removeOptional(tx); e != nil {
			return e
		}
		return cause
	}
	point("prepared")
	if e = atomicFile(p.binary, candidate, 0755); e != nil {
		return rollback(e)
	}
	point("switched")
	if e = check(p.binary); e != nil {
		return rollback(e)
	}
	if e = atomicFile(p.receipt, []byte(hash+"  "+p.binary+"\n"), 0600); e != nil {
		return rollback(e)
	}
	point("receipted")
	if len(s.floor) == 0 {
		s.state = map[string]json.RawMessage{"next_poll": json.RawMessage(`"0001-01-01T00:00:00Z"`), "seen": json.RawMessage(`{"sequence":0,"hash":""}`), "attempted": json.RawMessage(`{}`)}
	}
	s.state["build_seq"] = json.RawMessage(`4`)
	if e = atomicJSON(filepath.Join(p.private, "host-state.json"), s.state, 0600); e != nil {
		return rollback(e)
	}
	point("floored")
	// Journal removal is the durable commit point. An uncertain fsync is an
	// error, never a reported success; retained previous bytes remain available.
	if e = removeOptional(tx); e != nil {
		// Re-establish the durable recovery marker if unlink succeeded but its
		// directory sync failed, then restore the previous coherent state.
		if je := atomicJSON(tx, j, 0600); je != nil {
			return errors.New("helper commit durability uncertain; local recovery inspection required")
		}
		return rollback(e)
	}
	return nil
}
