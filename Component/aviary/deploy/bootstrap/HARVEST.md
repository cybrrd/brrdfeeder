# Bootstrap kit — harvested into version control 2026-07-18

**This kit provisions customer hardware and, until now, existed in exactly one place: on the
feeders themselves.** No repo copy, no backup. If robin had died, the install ritual would have
died with it — the pack's one rule (no store may be the sole holder) violated on a
customer-facing artifact. That is FABLE5 **B4 (installer version-control)** as a live risk rather
than a checkbox, and closing it is why this directory exists.

## Provenance

Harvested from `robin:~/brrdfeeder-bootstrap/` on 2026-07-18 during edge-beta Increment 0.
Original build: **2026-06-07 18:10:08**, built on **cardinal** by `synth`, kernel `7.0.0-1009-raspi`.

**Verified by execution** — the harvested files match the sha256 values recorded in the kit's own
`manifest.txt`:

| File | sha256 (manifest) | Result |
|---|---|---|
| `brrdfeeder-install.sh` | `58c008ab090ed2c6…` | ✅ match |
| `config.yaml.mobile.template` | `b29849cd632ea2b5…` | ✅ match |

So this is provably the same artifact that provisioned the live fleet, not a drifted copy.

## What was NOT found in the sibling repo

`github.com/cybrrd/brrdfeeder` — long referenced as the home of the customer install
scripts, and cited by REQ-BRRD-001 as one of the two places the udev rule lived — is **completely
empty**: zero commits, zero files. It is a placeholder that was created and never populated. Every
reference pointing customers or maintainers at it was dangling. Recorded here because that
misdirection is what let the installer live unversioned on devices for six weeks.

## Contents

| File | Purpose |
|---|---|
| `brrdfeeder-install.sh` | The install ritual (657 lines). "Path D substrate-architectural install" — Synth-drafted 2026-06-06, Gemini-ratified architecture (udev contract + systemd-user-unit + Quadlet-ready abstraction), Cy crosses the host-tenant wall via `sudo bash`. Closes task #149 (udev symlinks) and #146 (systemd unit surviving reboot/power-cycle/SSH disconnect). |
| `manifest.txt` | Build manifest: engine ELF description, sizes, sha256s, and the source capabilities (`cap_net_admin,cap_net_raw=ep`) with the load-bearing note that **tar does not preserve them**. |
| `config.yaml.mobile.template` | Mobile-profile config template. |
| `README.md` | Kit README as shipped. |

## Known gap — CTS-001 B3 (uninstaller) does NOT exist

Verified by grep across the installer: no `uninstall`, `--remove`, `purge`, `severance`, or
`rollback` path exists. CTS-001 makes **install manifest + uninstaller** release gates for anything
shipped to customer premises, so this is a genuine open item, not an oversight in the harvest.

Note the distinction that matters for authoring it: `manifest.txt` is a **build** manifest (what was
produced) — CTS-001 also wants an **install** manifest (what lands on the host, where, with what
ownership and capabilities) so that clean severance is checkable rather than hopeful. The
`brrdfeeder-install-audit.log` on each node (206 lines on robin) is raw material for that.

**Authoring the uninstaller requires a full read of the 657-line installer**, because an uninstaller
that misses artifacts leaves residue and one that over-reaches destroys host state that was never
ours (Condo Tenancy: the tenant drafts, the host crosses). It is deliberately not improvised here.

## Do not edit this copy casually

Until the installer's single source of truth is formally relocated here, treat these files as a
**harvested snapshot**. Changing them without re-deploying to the fleet re-creates exactly the drift
this harvest exists to end.
