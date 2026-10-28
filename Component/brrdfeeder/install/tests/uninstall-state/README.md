<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Four-tree uninstall coverage

The [pre-fix inventory](../../../../../docs/uninstall-state-inventory.md) records
writers at base `ab9328488cd166215a1b474ff653869ed56594ce`. This suite covers
`/var/lib/brrdfeeder` (S), `/var/lib/brrdfeeder-updater` (P),
`/var/lib/brrdfeeder-status` (T), and `/etc/brrdfeeder` (E).

`check-writers.py` extracts path constructors, filesystem/atomic-write call
arguments, literal path names, string bindings, literal arrays and numeric
ranges from Go/Rust production source; Python uses its AST. It scans new source
files too, plus installer Python heredocs. Go/Rust calls are followed through
project writer wrappers, and literal arguments at other calls are also retained.
It excludes test files and Rust
`cfg(test)` items, not the production code that follows them. The readable
`writer-paths.json` census is compared on every CI run. It contains expressions,
not file hashes: comments/formatting do not constitute a new path.

Literal destinations and rooted Go joins are projected directly. Known dynamic
families (Go and Python temporaries, credential keys, capture slots, upward lanes
and status temporaries) have explicit projections based on extracted writer
spellings. These paths are checked against the actual shell dispatch and the
embedded validator's name rules. A new literal Go destination still fails if
someone refreshes the census without adding uninstall coverage. Contracts also
assert that required dynamic families cannot disappear from the projection.

The census is deliberately conservative: it includes read paths and external
paths when a generic expression cannot be resolved. A change to one of those
requires review rather than an automatic assumption that it is harmless. This
is not a whole-program filesystem-effect proof. Reflection, new filesystem APIs,
arbitrary computed strings and dynamic imports require
extending the extractor; do not merely regenerate the census to dismiss its
failure. Runtime configuration coverage is the installed defaults plus the
documented optional S/status.json and S/capture placements. Other configured
destinations do not become deletion roots. No test claims to enumerate arbitrary
operator-supplied paths.

`test-container.py` runs complete dry-run and apply uninstalls with real files,
hardlinks, modes, service accounts and deletion tools. Systemd, Podman and
loginctl observations are inert; mount-refusal cases inject a findmnt observation.
The OS container must have networking disabled and only a read-only repository
mount. Tests fail instead of falling back to host execution. v0.8.29/v0.8.30
fixtures model their source-derived layouts; 0.9.0 adds active renewal secrets.
These are filesystem/account tests, not claims of native systemd integration.

New private files require root:root 0600, public attempt records root:root 0644,
and renewal secrets the service UID/GID with 0700 directories and 0600 files.
Renewal temporary keys must decode as NATS user public keys with valid CRCs.
Optional capture directories accept the deployed 0700/0750/0755 modes and private
0600 slots. Status and capture require matching configuration. Temporary names
follow their writers' generated formats; arbitrary suffixes are rejected.

The sole multi-link exception is a root-owned 0600 pending file and a valid
`.pending-<uint32>` temporary, both inside root:root 0700 P, with the same device
and inode and exactly two links. There is no recovery mutation during validation
or dry-run. Normal tree removal unlinks both names after stopping services. A
third link, an external link, two temporary names without the pending target,
wrong metadata or a symlink refuses. Credentials retain private modes throughout
removal and disappear before their owning account; cleanup never broadens modes.

`test-mutations.py` removes each new acceptance rule independently. A complete
positive dry-run/apply control precedes every mutation. Each mutant must refuse
both dry-run and apply without changing seeded state. Separate catalogue
mutations delete or deregister the structural gate; the independent public-CI
guard must catch each. Writer-addition/rename/range negative controls run in
`test-contract.py` with their own clean controls and are reported separately.

The full offline runner includes these suites in installer, mutation and
container groups. Supply an already-provisioned immutable `TEST_OS_IMAGE`.

## Deferred follow-up (PR B)

The approved split explicitly defers:

- External `.update-*`, `.host-recover-*`, `.helper-candidate-*`, and private
  `.brrdfeeder-helper-*` staging directories
  in `/usr/local/libexec`, `/etc/systemd/system`, `/usr/local/bin` and
  `/etc/containers/systemd`; ownership in shared directories needs its own design.
  The helper dispatcher stages beside the installed executable because `/run`
  may be mounted `noexec`. Ordinary success/failure removes its private staging
  directory; process death may leave it for evidence. This does not authorize
  broad deletion from those shared directories.
- Narrowing T's existing acceptance of any regular non-symlink file.
- Narrowing the existing broad S/`*.tmp` pattern.
- Recursive provenance for `P/rejected-pending-*/entry/**` quarantined input.

PR A preserves those existing behaviors. Unknown-entry refusal claims apply to
S/P/E outside the explicitly retained patterns, and to credentials/capture
children; they do not claim the deferred broad policies are already fixed.
