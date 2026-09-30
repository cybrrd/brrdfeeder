<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Install log and support bundle — specification

**Status:** APPROVED by Cy, 2026-09-23 ("i've read and approve"). Codex implements, Kimi gates against §I.
**Ruling it implements (Cy):** *"make sure we have our default log generator too that generates a
comprehensive log as it installs… users will need to email the comprehensive, easily attainable
log if they're running into problems… you and I will be using that to debug user experiences in
the field who are having failed installs and we cannot remote onto their PC."*

## The design constraint in one line

**The reader has no access to the machine.** Everything the reader needs to reconstruct what
happened must be *in the file*, and nothing in the file may be something the user should not have
emailed. Both a human and an AI will read it, and the AI reads it first.

## What exists today (starting point, not a blank page)

`brrdfeeder-install.sh` already has `--audit-log=<path>`: `exec > >(tee -a "$AUDIT_LOG") 2>&1`,
so every subcommand's stdout and stderr is captured, plus an environment header (`uname -a`,
`/etc/os-release`, `lsusb`, `df -h`). Structured line prefixes exist: `>>>` phase, ` OK `, ` !! `,
`FATAL`. Keep all of it. The work is to make it **default**, **redacted**, **timestamped**,
**summarised**, and **collectable**.

---

## A. Default-on install log

- Every run writes `/var/log/brrdfeeder/install-<UTC-ISO-timestamp>-<run_id>.log`, mode 0640
  root:root, and updates a symlink `/var/log/brrdfeeder/install-latest.log`.
- `--audit-log=<path>` remains as an **override of the path**, not the switch that enables logging.
- Dry-run and `--verify` and `--uninstall` also log, to the same directory, with the mode in the
  filename (`install-`, `verify-`, `uninstall-`, `dryrun-`). A dry-run someone emails is still
  evidence.
- If `/var/log/brrdfeeder` cannot be created (read-only root, disk full) the installer prints
  ONE line saying logging is degraded to `/tmp/…` and continues. Never fail an install because the
  log could not be opened; never silently run without a log either.

## B. Run ID — the correlation handle

- Generate an 8-hex-character `run_id` at start (`head -c4 /dev/urandom | od -An -tx1 | tr -d ' '`).
- It appears: in the log **filename**, in the log **header**, as the **first line printed to the
  terminal**, and in the **last line printed to the terminal**. Support asks "what run ID is on
  your screen?" and can match it to the file.
- The bootstrap generates it and passes it down (`BRRDFEEDER_RUN_ID` env), so bootstrap and
  installer share one ID.

## C. File structure — one file, human-first, machine-parseable blocks

Plain UTF-8 text. **No ANSI escape codes** (strip if any subcommand emits them). Every line the
installer itself writes is prefixed with an ISO-8601 UTC timestamp. The file has four fixed
blocks, in this order, with these exact block markers so a reader can jump straight to any one:

```
==== BRRDFEEDER INSTALL LOG ====
run_id=3fa2c9b1
mode=install
started=2026-09-23T02:14:05Z
installer_sha256=fedee81df2f417cdcf0c15b89ffff66fcbe9c00c506b6922dbafbcabf0b63ee3
bootstrap_sha256=d377148230aed0bb7fd9c10c8a82cba15039390f8f36da7aee237b873b4f55aa
argv=--image ghcr.io/macawi-ai/brrdfeeder-open@sha256:b327… --console-image ghcr.io/… --console-listen 192.0.2.52:8080 --interface wlan1
invoked_by_uid=0 sudo_user=sagan

==== ENVIRONMENT ====
os=Debian GNU/Linux 13 (trixie)
kernel=6.18.50+rpt-rpi-v8
arch=aarch64
board=Raspberry Pi 4 Model B Rev 1.4
uptime_s=84240
mem_total_mb=3792
disk_root_free_mb=52100
throttled=0x0
podman=5.4.2
timezone=UTC    clock_ntp_synced=yes
usb=
  0e8d:7961 MediaTek Inc. Wireless_Device
  0bda:876e Realtek Semiconductor Corp. Bluetooth Radio
  1546:01a7 U-Blox AG [u-blox 7]
net=
  wlan0 192.0.2.52/24 (default route)
  wlan1 (no address)
phys=
  phy0 wlan1 mt7921u monitor=yes
  phy1 wlan0 brcmfmac monitor=no
reach=                       # from pre-flight, the same checks a customer's network must pass
  ingest.cybrrd.com:4222 open
  hospitality.cybrrd.com:443 open
  ghcr.io:443 open
  globe.cybrrd.com:443 open
dns_resolver=192.0.2.1

==== BOOTSTRAP ====
console_listen=192.0.2.52:8080 (auto-detected on wlan0)
capture_interface=wlan1 (only monitor-capable adapter)
position=GPS service will wait for a measured fix (2026-09-23 GPS ruling; no owner prompt)
config_state=abandoned-template-removed     # or fresh | rerun
installer_checksum=verified

==== TRANSCRIPT ====
2026-09-23T02:14:05Z [PHASE] pre-flight
2026-09-23T02:14:05Z [OK]    service user brrdfeeder (uid=999 gid=985)
2026-09-23T02:14:06Z [CMD]   apt-get install -y --no-install-recommends jq curl   rc=0  dur=11.8s
2026-09-23T02:14:18Z [OK]    enrollment bootstrap deps present
2026-09-23T02:14:19Z [PHASE] images
2026-09-23T02:14:19Z [CMD]   podman pull ghcr.io/macawi-ai/brrdfeeder-open@sha256:b327…   rc=125  dur=4.2s
    Error: initializing source docker://ghcr.io/…: pinging container registry ghcr.io:
    Get "https://ghcr.io/v2/": dial tcp: lookup ghcr.io: Temporary failure in name resolution
2026-09-23T02:14:23Z [FAIL]  step=images/pull-engine rc=125

==== RESULT ====
result=FAILED
failed_phase=images
failed_step=images/pull-engine
exit_code=125
duration_s=18
last_output=
    Error: initializing source docker://ghcr.io/…: pinging container registry ghcr.io:
    Get "https://ghcr.io/v2/": dial tcp: lookup ghcr.io: Temporary failure in name resolution
log=/var/log/brrdfeeder/install-2026-09-23T021405Z-3fa2c9b1.log
support_bundle_cmd=sudo bash brrdfeeder-install.sh --support-bundle
```

Rules for the TRANSCRIPT block:

- Every `run` of a subcommand emits a `[CMD]` line with the command, its **exit code**, and its
  **wall duration**. Its stdout+stderr follow, indented four spaces, so it is visually and
  syntactically inside that step. This is what lets a reader see that the failure was in `podman
  pull`, not `apt-get`, without reading either's output in full.
- Step names are stable identifiers `phase/step` (e.g. `images/pull-engine`), not prose. They are
  what support and the AI grep for across many users' logs.
- `[FAIL]` is followed immediately by the RESULT block. Nothing runs after a failure except
  writing RESULT and printing the terminal summary.
- The `[OK]`/`[FAIL]`/`[PHASE]`/`[CMD]`/`[WARN]` tokens are fixed width so the file aligns.
- Console cgroup policy is recorded in TRANSCRIPT as `console_memory_limit=96m`
  or `console_memory_limit=omitted`, with `reason=` and the resolved user-service
  `cgroup=`. Omission means no memory cap, not a failed install. Dry-run records
  `console_memory_limit=deferred reason=dry-run`; it must not claim delegation
  that has not been checked. Kernel controller support alone is not delegation.

Rules for the RESULT block (this is the part the AI reads first):

- Always present, even on success (`result=OK`, `failed_step=` empty).
- `last_output` is the **last 40 lines** of the failed step's captured output, verbatim. The
  full output is already in TRANSCRIPT; this is the summary so the reader does not have to find it.
- Every field is `key=value`, one per line, greppable.

## D. Redaction — mandatory, applied at write time, never after

The user will email this file. Nothing below may appear in it:

| never log | how it enters | redact as |
|---|---|---|
| OAuth device flow `user_code` and `device_code` | printed by the enrollment step for the user to type | the user sees it on the terminal; the log gets `[REDACTED:device-code]` |
| contents of `/etc/brrdfeeder/secrets/brrdfeeder.creds` | any `cat`, any error that echoes it | `[REDACTED:nats-creds]` — the file is never read into the log, and the path is fine to log |
| `Authorization:` header values, bearer tokens | curl verbose, error bodies | `[REDACTED:token]` |
| anything matching a JWT (`eyJ…`) or a 32+ char base64/hex token following `token`, `secret`, `key`, `password` | error bodies, env dumps | `[REDACTED:secret]` |
| the console's status page HTML if it ever contains a claim code | unlikely; defensive | as above |

Implementation: the `tee` becomes `tee >(sed -E -f "$REDACT_RULES" >> "$LOG")` or equivalent —
redaction sits **between** the process and the file, so a secret never touches disk unredacted
even for a moment. The terminal still shows the device code, because the user needs to type it.

**Prove it can fail:** the acceptance test seeds a fake device code, a fake JWT and a fake creds
file, runs the installer to the point they would be logged, and asserts none of the three strings
appears in the log while the `[REDACTED:…]` markers do. A redaction rule never seen catching
something is not known to be a redaction rule.

**Deliberately NOT redacted**, because support needs them and they are not secrets: LAN IPs,
interface names, USB IDs, the node's public identity, image digests, hostnames, coordinates.

## E. The terminal summary — the last thing a failing user sees

On failure the final lines on screen are exactly:

```
[brrdfeeder-install] FAILED at images/pull-engine  (run 3fa2c9b1)
[brrdfeeder-install] Log: /var/log/brrdfeeder/install-2026-09-23T021405Z-3fa2c9b1.log
[brrdfeeder-install] For support, run:  sudo bash brrdfeeder-install.sh --support-bundle
[brrdfeeder-install]   then email the file it names.
```

On success, one line: `Log: <path>  (run <id>)`. A user who later has a problem still knows where
the install record is.

## F. `--support-bundle` — one command, one file, ready to email

`sudo bash brrdfeeder-install.sh --support-bundle` produces
`/tmp/brrdfeeder-support-<UTC-ts>-<run_id>.tar.gz` and prints ONE line naming it and its size.
The bundle is the install transcript **plus the current state**, because a failed install and a
sensor that installed fine but stopped working are the same support conversation:

```
bundle/
  MANIFEST.txt                 what is in here, generated when, run_id, host, redaction applied
  install-logs/                every /var/log/brrdfeeder/*.log (size-capped: newest 10)
  bootstrap-*.log              the bootstrap's own log(s) — its refusals happen BEFORE the installer
  environment.txt              the ENVIRONMENT block, re-taken NOW (not the install-time copy)
  config.yaml                  REDACTED copy of /etc/brrdfeeder/config.yaml
  quadlets/                    brrdfeeder-engine.container, brrdhouse.container, updater units, udev rule
  systemd/                     `systemctl status` (rootful) and `systemctl --user status` (console user) for each unit, `is-enabled`, `is-active`
  podman/                      `podman ps -a`, `podman images --digests`, in BOTH namespaces
  journal/                     last 500 lines of each unit's journal, REDACTED, both namespaces
  engine-log.txt               last 500 lines of the engine container log, REDACTED
  reach.txt                    the four endpoint reachability checks, re-run NOW
  clock.txt                    `timedatectl`, `date -u`, whether NTP synced — clock skew breaks TLS and enrollment
```

- Size cap: journal and container logs at 500 lines each; install logs newest 10. A bundle must
  be emailable — target under 2 MB. If a cap truncated something, MANIFEST.txt says so.
- **Redacted with the same rules as §D**, applied to every file in the bundle.
- Works on a machine where the install failed early: absent items are recorded in MANIFEST.txt
  as `absent: <path>`, not skipped silently. An absent quadlet *is* a finding.
- Works without root for the read-only parts, but says which parts it could not collect.
- Prints the path, the size, and **a one-line summary of what it thinks the state is**
  (`engine: active`, `console: inactive (never installed)`, `last install: FAILED at images/pull-engine`)
  so the user can tell support that much on the phone.

## G. Bootstrap integration

The bootstrap (`get.cybrrd.com`) runs *before* the installer and makes decisions that matter
(console address, capture interface, position, template recovery) and can *refuse* (checksum
mismatch, no monitor-capable adapter, wrong arch). Those must be in the record:

- The bootstrap writes `/var/log/brrdfeeder/bootstrap-<ts>-<run_id>.log` with its decisions and
  any refusal, using the same run_id it passes to the installer.
- It passes its decisions to the installer as `BRRDFEEDER_BOOTSTRAP_NOTES` (env), which the
  installer writes verbatim into the BOOTSTRAP block.
- A bootstrap refusal prints the same "Log: <path>" line, so a user refused for a missing adapter
  still has something to send.

## H. Why this shape suits an AI reader specifically

- Fixed block markers (`==== RESULT ====`) let the reader jump; it does not have to read 800
  lines of apt output to find the failure.
- `key=value` everywhere outside TRANSCRIPT means the environment and result are extractable
  without parsing prose.
- Stable `phase/step` identifiers mean the same failure in ten users' logs is the same string.
- `[CMD] … rc=N dur=Ns` per subcommand gives the reader the timeline and the exact failing
  command without inference.
- The failing step's output is quoted in RESULT, so the most important 40 lines are at the
  bottom of the file, not the middle.
- Redaction markers say *what kind* of thing was removed, so the reader knows a token was there
  without knowing its value — which is often itself the diagnostic ("it never got a token").

## I. Acceptance — what Kimi gates against

1. A run with no flags produces a log at the default path; the run_id on screen matches the file.
2. A forced failure at a mid-install step produces a RESULT block naming that step, with its last
   40 lines, and the terminal summary in §E.
3. **Redaction proven**: seeded device code, JWT and creds content are absent from the log and
   from the bundle; the `[REDACTED:…]` markers are present. Negative control: a run with no
   secrets produces no markers.
4. `--support-bundle` on a machine where install failed early produces a bundle under 2 MB whose
   MANIFEST lists the absent items explicitly.
5. `--support-bundle` on a healthy sensor produces a bundle whose one-line state summary says
   `engine: active`.
6. No ANSI codes anywhere in any file (`grep -P '\x1b\['` finds nothing).
7. The bootstrap's refusals (wrong arch, no adapter, checksum mismatch) each leave a bootstrap
   log naming the reason.

## J. Out of scope, stated so it is not assumed

- **No automatic upload.** The user chooses to email the bundle. Nothing leaves the device on
  its own. (A future opt-in "send to support" is a separate decision with a separate consent step.)
- **No remote access.** This spec exists precisely because there is none.
- Engine *runtime* logging (what the sensor logs while operating) is the engine's concern and
  already exists; the bundle *collects* it, it does not redesign it.
