<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Customer installer (not the aviary refresh script)

This is the customer installer. `Component/aviary/deploy/bootstrap/brrdfeeder-install.sh`
is a separate refresh/migration tool; do not substitute it.

**Self-Update** adds automatic pull-only updates for both engine and console, and a
separately SHA256-pinned host updater downloaded from get.cybrrd.com. The updater
is never extracted from either image. `--ring dev|staging|general` selects the
installed release ring (default general); signed metadata must match it. Boot
recovery uses local retained images, with freshness/restart-aware health gates
and automatic rollback. Host self-release verification/recovery is implemented;
host metadata signing/publication is separately gated, not enabled for Friday.
See [release-system](../../aviary/release-system/README.md) for the trust/health
contract, publication steps and limits. Enabling Self-Update on an older unit requires a
one-time local uninstall and reinstall; there is no silent migration.
For now the PUBLIC command below stays on the previous release. Self-Update's
isolated `https://get.cybrrd.com/dev` path is for the approved brrdg1s1 dev proof.
After that passes, Cy switches the public bootstrap and general release in one
publish, without rebuilding images. Existing team installs then need
that one-time reinstall. See the [Friday runbook](../../../governance/reviews/d44-rework/FRIDAY.md).

## Standard single command

From your normal sudo-enabled account on the Pi:

```sh
curl -fsSL https://get.cybrrd.com | bash
curl -fsSL https://get.cybrrd.com | bash -s status
curl -fsSL https://get.cybrrd.com | bash -s support-bundle
curl -fsSL https://get.cybrrd.com | bash -s uninstall
```

The bootstrap downloads and checksum-verifies as your account, then requests
sudo with stdin attached to your terminal. It may ask for your sudo password.
It uses `ps` (the stock `procps` package) to identify the concrete controlling
terminal: sudo 1.9.16p2 can still lose input through the `/dev/tty` alias alone.
Do not put sudo between the pipe and bash: sudo's own PTY can otherwise receive
only the download pipe, not your keyboard. Root-only adapter/config preparation
runs inside the verified installer and its default log. The run ID and bootstrap
notes/log path are passed explicitly; no broad `sudo -E` is used.

After elevation, root creates a private 0700 directory under `/run`, copies the
download into it as a 0600 root-owned file, re-verifies that copy against the
bootstrap's pinned SHA-256, and executes only that copy. The pin, arguments and
staging code travel on sudo's command line, not in a user-writable control file.
The private copy is removed on exit. This closes a same-UID process swapping the
download after its first check or during the sudo password prompt. It cannot
protect against an already-root attacker, a compromised bootstrap/pin, or an
attacker already controlling the invoking shell/its command arguments.

Install and uninstall acknowledge work immediately, print a line at each phase's
start and completion, and print elapsed-time feedback at least every four
seconds while a phase is quiet. Confirmation pauses that heartbeat while waiting
for your answer. Verbose output is the default: routine status and command output
also reach the terminal. Add `--no-verbose` for the quieter progress-only display;
warnings, errors, GPS/BLE status, final instructions and Device Flow's code/link
remain visible. The redacted log always captures all output in either mode.
The code is displayed to you but redacted before any log write. Time estimates
are approximate, not deadlines for slow storage or downloads.

```sh
curl -fsSL https://get.cybrrd.com | bash -s -- --no-verbose
curl -fsSL https://get.cybrrd.com | bash -s uninstall --no-verbose
sudo brrdfeeder uninstall --no-verbose
sudo bash brrdfeeder-install.sh --no-verbose  # re-run with existing configuration
sudo brrdfeeder --help
```

For terminal-free automation, arrange administrator-approved noninteractive sudo
permission first. Removal additionally requires explicit confirmation:

```sh
curl -fsSL https://get.cybrrd.com | bash -s uninstall --yes
```

No sudo or refused permission produces a named administrator-action refusal;
the bootstrap never installs sudo. Already-root noninteractive invocations remain
supported. Removal confirmation has a 60-second input deadline; an unreachable
legacy sudo PTY fails without removing anything and names the working command.
The local `sudo brrdfeeder` recovery commands below still work without downloading.

## Expert pinned installer invocation

Fresh installations require **operator-approved release digests**
for **both** engine and intrinsic BRRDhouse console, supplied through the trusted
installer/release distribution channel. From the customer's non-root sudo account,
one fresh invocation supplies the capture interface and
intended LAN listener (replace these example values):

```sh
sudo bash brrdfeeder-install.sh \
  --image ghcr.io/cybrrd/brrdfeeder@sha256:<approved-engine-64-hex-digest> \
  --console-image ghcr.io/cybrrd/brrdhouse@sha256:<approved-console-64-hex-digest> \
  --interface wlan1 \
  --console-listen 192.0.2.50:8080
```

The angle-bracketed value is a placeholder, not a runnable release selection.
Do not resolve `:latest` on the customer unit to obtain it. The installer does
not select a release or verify a signature: it pulls the immutable reference
if absent and checks exact `RepoDigests` membership. This binds integrity to
the supplied digest, **not to a signing key**. No host-wide policy is replaced;
any existing Podman policy still applies when pulling.

The first run creates a locked, non-login `brrdfeeder` system account and writes
`/etc/brrdfeeder/config.yaml` using the capture interface. If enrollment
is needed, owner approval of the existing OAuth device flow resumes the same invocation.
The capture interface is first-install only; existing configurations are preserved.
Omitting it retains the template flow (exit 2). Position is **not prompted or
required from the owner**: the service reads a measured GPS fix before launching
the engine, without blocking installation. A missing GPS reports “GPS not detected:
plug in the GPS”; a cold receiver reports “Waiting for GPS fix”. Watch the console
or `sudo systemctl status brrdfeeder-engine.service`. The engine starts automatically
after the fix. The receiver must emit checksummed GGA including measured altitude;
estimated/manual/simulator positions are not used. Waiting has no deadline.

Automatic `/dev/cybrrd_gps` mapping supports USB IDs **1546:01a5, 1546:01a6,
1546:01a7, 1546:01a8 and 1546:01a9**, restricted to tty devices with dialout/0660
permissions. This is the supported mapping list, not a claim that every firmware
variant has been tested on a Pi. Connect **one GPS receiver at a time**. A receiver
using a different ID (including a clone) is not automatically mapped or probed.
Pre-flight prints unsupported u-blox and USB-serial candidate IDs; candidates may
be unrelated serial hardware. Send that log for a reviewed mapping/config change
instead of applying a vendor-wide wildcard. A clone without a tty driver cannot
be identified as GPS by this passive inventory. Receiver NMEA output must still be
enabled at the configured baud rate.

While waiting for the initial fix, unplug/replug is retried automatically. The
waiter checks that the configured path still names its open device, closes a
replaced/disconnected descriptor, discards old partial sentences/observations,
and reopens the path. Fifteen seconds without bytes also triggers a reported
reopen, rather than waiting forever on a silent descriptor. The journal and
`systemctl status` name missing, disconnected/replaced or silent receiver states;
the console's existing missing/unavailable/waiting view remains offline. No
receiver commands are sent and a receiver held by another process is not taken.
Once the engine starts, GPS belongs to the engine; its own hotplug recovery is
outside this pre-start helper's scope.

The startup record, console and journal report satellites used (GGA), satellites
in view and maximum/average SNR (complete GSV cycles), fix quality (GGA), fix mode
(GSA: 1=no fix, 2=2D, 3=3D), and seconds since the last checksum-valid NMEA
sentence. Unknown is not zero: a silent receiver has no recent NMEA, while a
receiver reporting quality 0/mode 1 is talking but not locked. Observations expire
after 15 seconds. SNR excludes blank readings; combined GN cycles take precedence
over per-constellation counts. No raw NMEA or coordinates enter public startup status.

Experts may optionally supply paired `--latitude` and `--longitude` on first install.
That explicit override uses the historical configured elevation of 0; automatic GPS
seeding never invents coordinates or altitude. Existing valid locations are not
rewritten or reseeded on reboot. The current engine still falls back to configured
position when GPS is stale; fixing that requires a future engine image.

The package automatically creates a separate locked `brrdhouse` account, allocates
rootless Podman mappings, enables user-service linger, pulls and verifies the console
digest, and runs the **shipped** console status provisioner. It configures
`node.status_file`, engine RW and console RO mounts at `/var/lib/brrdfeeder-status`,
then starts the console and queues the engine service. The directory is 0755, owned by the real engine service
UID/GID; D17 writes 0644 files. Credentials remain separate. No additional console
command, manual provisioning, config or Quadlet edit is required. The pre-start
helper writes a separate `startup.json` every five seconds, not an engine heartbeat.
The updated console always renders it as offline/waiting; after fifteen seconds
without updates it reports stale startup status. It is removed before engine start.
The helper is `/usr/local/libexec/brrdfeeder-gps-seed`, run only as the engine's
`ExecStartPre`; it refuses competing device owners and closes GPS before the engine
opens it. Uninstall stops that same service before deleting the helper and sidecar.

The console retains a 64-process limit, read-only root, zero capabilities,
no-new-privileges and its non-root identity on all supported hosts. Its 96 MiB
Podman memory cap is applied only when the running console user's cgroup actually
delegates memory control. Otherwise it is omitted (no memory cap), with the decision
and reason in the install log. This permits stock Pi boot configurations without
memory accounting; the installer changes no boot settings and requires no reboot.
Operators may separately choose `cgroup_enable=memory` in their Pi boot configuration
(resolving any conflicting disable setting); kernel support alone is insufficient
without user-service delegation. Rerun the installer after such a host change to
re-evaluate the cap.

The advertised piped one-liner can show OAuth on stdout's terminal or a writable
controlling terminal; it does not need to read a code from stdin. The bootstrap
reconnects stdin to an openable terminal at handoff. With no display, enrollment
still refuses and names the manual-credentials fallback. Codes remain visible to
the owner but redacted in the saved log.

Read the console at the supplied HTTP address. Use a specific private LAN IP and
unprivileged port (IPv6 example: `[fd00::50]:8080`). Wildcards, public IPs and tailnet
IPs are refused. Loopback is accepted for local tests but is not LAN access. Exact
Host-header validation remains mandatory. If DHCP changes the LAN address, rerun
with `--console-listen` set to the new address; pins remain unchanged. Do not expose
the unauthenticated page on the internet.

There is no `--user` override and `SUDO_USER` does not select service ownership.
UID/GID are allocated locally. Existing service accounts must be non-root system
accounts with a dedicated group and non-login shell; their home is read from
passwd. The engine gets the numeric dialout supplementary group, service-writable
state, and read-only service-group access to root-owned NATS credentials. The
refresh token remains root-only. The identity helper is embedded, installed by
this script, and equality-checked against the canonical repo helper by the guard.

After both pinned Quadlets exist, rerun without either image argument or the
first-install config arguments to preserve both digests and the listener. To add
the console to an existing engine install, supply `--console-image` and
`--console-listen` once; the engine pin is preserved. A conflicting digest or a tag/local-alias/multiple-Image existing
Quadlet is refused: **installer reruns are not an update or downgrade channel**.
Engine AND console updates belong to the signed release poller, not Blue.
Login-owned legacy installs and existing
tag-based deployments require an explicit migration; no `synth`/invoker paths
are guessed or cleaned up.

`--dry-run` plans without account/config creation. With no config it exits 2
(edit required), not success. Its provisional UID/GID labels are not allocated
identities. `--verify` retains legacy warning-based semantics and is **not a
readiness gate**. The Documentation URL in the Quadlet points at the current
internal documentation projection; public customer documentation is still a
publication task, not assumed available under an invented hostname.

## Status, support, and uninstall

```sh
sudo brrdfeeder status
sudo brrdfeeder support-bundle
sudo brrdfeeder uninstall
```

The installer places a self-contained command in `/usr/local/sbin` before account
or configuration setup, including on partial installs. Removal needs no download,
image pins, adapter name, or other install parameters. It shows a short summary
and asks once, on your terminal: `Remove BRRDfeeder from this Pi? [y/N]`.
Enter or `n` cancels without removal. The full plan and execution detail go to the
redacted log, whose path is printed. `status` only reads local service state.

For installations made before this command existed:

```sh
curl -fsSL https://get.cybrrd.com | bash -s uninstall
```

The compatibility route downloads the checksum-verified installer, but skips
install-only LAN, adapter and image setup. For an explicit plan or automation:

```sh
sudo brrdfeeder uninstall --dry-run  # log-only plan, no confirmation or removal
sudo brrdfeeder uninstall --yes     # automation: explicit confirmation
sudo bash brrdfeeder-install.sh --verify  # legacy detailed installer verification
```

Without a terminal, removal refuses unless `--yes` is supplied. The local command
removes itself last; after successful removal it no longer exists. A retained
installer or the compatibility one-liner can verify an already-absent installation.

Uninstall works offline, without either image parameter, and after a failed/partial
install. It stops the updater and both container services before removing Quadlets,
reloads both active systemd namespaces, removes package containers/images and local
configuration/credentials/state, undoes the package's udev/chrony/journald overrides,
then removes the dedicated accounts and console linger. Missing items are reported
as `nothing to do` in the log; a second uninstall is valid. Empty Quadlet directories are
removed, but shared nonempty directories are retained.

**This deletes local data and both images.** Each removed image ID and its digest
references are recorded in the log. A reinstall re-pulls roughly 120 MB plus the console.
There is no `--keep-images`: retaining the console's private store would conflict
with removing its account/home and the greenfield test. Images with unrelated
aliases or other container users are refused, not forced away. System packages
(including Podman, jq, curl, iw if present, and other shared dependencies) are never
uninstalled. Existing external audit logs and shared journals are retained.

Account names are not proof of ownership. New installs write root-only creation
receipts **only for accounts they actually create**. Old unmarked `brrdfeeder` and
`brrdhouse` accounts are automatically recognised, with a visible notice, only
when their locked password, nologin shell, fixed `/var/lib/<name>` home, dedicated
group, ownership and UID checks pass; no sudo/adm/wheel/root membership, recorded
login history or unrelated process is allowed. Active package processes must be
in their verified systemd unit cgroups (plus the console's user-manager/DBus
infrastructure). Unreadable/unknown history formats refuse. The check covers
retained binary wtmp/lastlog and Debian Trixie's SQLite replacements; absent or
rotated-away history cannot prove an account has never logged in historically.
The expert `--adopt-legacy-accounts` acknowledgement remains accepted but bypasses
none of these safety checks.

Adoption does not bypass checks: UID below 100, login/shared accounts, unexpected
homes/groups, changed receipts, symlinked config/paths, mounted deletion roots and
unknown contents are refused. Resolve the reported ambiguity before retrying;
there is no broad force/delete-anything flag. Dry-run writes only its own redacted
`dryrun-*.log` (and required log-directory scaffolding), creates no accounts,
starts/stops no units and never initializes Podman
stores. It lists image selection rules; actual offline digest enumeration happens
on apply, and can still refuse a shared image.

Local removal **does not deregister the node with flock**. The same authenticated
owner (Zitadel subject) and hostname renew the same node ID with fresh credentials;
a different owner or hostname can create a new node. Server records remain. No uninstall
step contacts flock, ingest or NATS. Historical clock changes, lost volatile logs,
and files overwritten by earlier installers cannot be reconstructed locally.

See the [uninstall report](../../../governance/reviews/2026-09-23-installer-uninstall-REPORT.md)
for mutation inventory, acceptance evidence and native-hardware limitations.

### Install records and support

Python 3 is required before running the installer; supported Raspberry Pi OS
images already ship it. If absent, the installer writes a Bash-only diagnostic
and asks you to run `sudo apt install python3`, then retry. Uninstall and support
collection never install packages.

Install, verify, uninstall and dry-run log to `/var/log/brrdfeeder/<mode>-<UTC>-<run_id>.log`
(0640 root:root for sudo runs); `--audit-log=/absolute/path.log` overrides the path.
Use a new override filename per run: existing files and symlinks are never
overwritten. An unavailable/unsafe destination falls back to a clearly named
`/tmp/brrdfeeder-*.log`. Normal runs update `install-latest.log`; dry-run does not.
The run ID appears on the first and last terminal lines. The bootstrap's ID,
decisions and log reference are preserved when supplied.

Logs include environment, timestamped phases, action-command output/exit/duration,
and an always-present RESULT with the failed step's last 40 lines. Secrets and ANSI
escapes are filtered before file writes; enrollment codes remain visible on screen.
Do not send an unfiltered terminal recording in place of the redacted log.

`--support-bundle` is a standalone mode (do not combine with other flags). It
writes a redacted archive under `/tmp`, below 2 MB, and prints
its path, size and current-state summary.
The archive is 0600 and, under sudo, owned by the validated invoking user so it
can be attached to email without making it world-readable. It collects the newest
10 default logs,
bootstrap records, config, units, system/user state and bounded runtime logs;
`MANIFEST.txt` explicitly records missing/unreadable/truncated items. It works
after an early failure or uninstall, and collects readable portions without sudo.

The summary labels the last install's own run ID separately from the bundle's
new run ID, for example `last install: OK (run c6056171); bundle run 11223344`.
An older log with no valid run ID is reported as `run unknown`, never assigned
the bundle's ID.
It performs only four bounded TCP reachability probes (ingest:4222, hospitality:443,
GHCR:443, globe:443); failures are recorded without aborting collection. Nothing
is uploaded. Review the archive and choose whether to email it to support.

D42 does not prove the release image is appropriate, hardware works, OAuth/NATS
enrollment succeeds, or the engine runs under the rendered contract. No customer
deployment approval is implied. Journald ordering, partial-install rollback,
updater deduplication, refresh-token renewal, atomic writes and stronger verify
semantics remain follow-ups. See the
[D42 report](../../../governance/reviews/2026-09-21-D42-installer-customer-blockers-REPORT.md).
See also the [package acceptance report](../../../governance/reviews/2026-09-22-package-console-REPORT.md)
for the execution evidence, explicit fixtures and native-Pi denominator.

## GPS placement matters

GPS satellites are mostly in the southern sky. Put the GPS where it can see south
and overhead. North-facing windows, metal roofs and tree cover can prevent a fix.
This advice is for the northern-hemisphere installation; clear overhead sky is
important everywhere. A powered receiver indoors is not evidence of a usable fix.
Open the included read-only console: POSITION shows Good / Marginal / Poor / No
fix, used satellites, HDOP and fix quality when the engine reports them. While
waiting, it shows satellites in view, max/average SNR and valid-NMEA age; unknown
means not reported recently. The engine currently does not publish in-view/SNR.
The service waits for a measured fix; you do not need to enter coordinates.
Default rating thresholds and operator tuning flags are in the
[console README](../../brrdhouse/README.md#gps-reception-rating).

## RID.BLE controller ownership

BRRDfeeder includes Wi-Fi and Bluetooth RID. Passive USB inventory names the supported
Realtek **0bda:876e** receiver without sending HCI commands. On fresh installs
and reruns, **only a missing `sensors.rid_ble` key** is added when exactly one
supported receiver is present, as `enabled: true`, `unblock_rfkill: true` with
`adapter.usb_id: 0bda:876e`. Reruns also add only an absent `unblock_rfkill` field
to an already-enabled, validated RID.BLE configuration. Explicit true/false wins.
No receiver means no automatic enablement. Multiple identical receivers require
an explicit BD_ADDR selection. Explicit existing config (including `enabled:
false`) is preserved, not silently changed; config migration may reformat YAML
comments but preserves existing values. The engine, not the installer, performs
the targeted soft-unblock of its selected adapter at each startup. This handles
stock Pi OS/systemd-rfkill restoring a blocked state after reboot. Hard blocks
still require physical/operator attention. No CHANGE_ALL or installer rfkill
write is used. This is not a claim every Bluetooth USB adapter supports RID.

When RID.BLE is enabled the installer stops the engine/waiter, records the prior
`bluetooth.service` enablement and active state in root-only
`/etc/brrdfeeder/.bluetooth-prior.json`, then stops/disables/masks bluetoothd before
restarting the engine. Bluetooth keyboards/audio on that host will not be served
by bluetoothd while RID.BLE owns it. A rerun retains the original receipt.
The helper is `/usr/local/libexec/brrdfeeder-bluetooth`. Uninstall validates the
receipt before mutation, stops the engine first, restores the recorded service
state, and only then removes the helper/receipt. Partial failures retain the
receipt for retry; legacy installations without a receipt leave bluetoothd alone.
Disabling RID.BLE explicitly and rerunning also restores a recorded prior state.
Dry-run only describes these actions. After engine stop and bluetoothd stop/mask,
the helper identifies the configured **supported USB** controller through sysfs
(`hciN/device` ancestry to `idVendor`/`idProduct`; optional configured BD_ADDR is
matched against sysfs too). It sends only **HCIDEVDOWN** to that index, reads
**HCIGETDEVINFO** flags, and refuses if it remains UP or its identity changes.
Zero/multiple matches refuse; no guessed hci0, discovery tools, inquiry, reset,
device-UP or global rfkill writes. Uninstall restores bluetoothd; it need not
power the adapter UP. Other configured adapter families are not automatically
taken DOWN: they need a reviewed extension, not an identity guess.
With bluetoothd persistently masked the adapter is expected to remain DOWN at
boot, but **this has not been verified on a native Pi**. Cold boot, USER binding,
The engine's rfkill inventory is a preflight observation, not current unblock or
health proof. The install summary reads the last BLE lifecycle transition in the
current systemd invocation, gated by fresh D17 status (age <= 3x its interval).
Missing/stale data is Unknown, waiting for GPS is Waiting, and Failed remains
Failed with its reason. This does not promise advertising traffic will be present.

Uninstall restores recorded bluetoothd service settings, **not rfkill bits**.
The engine's targeted unblock may be retained by systemd-rfkill in
`/var/lib/systemd/rfkill/`. Reinstating an old block could override later user
radio choices; installer/uninstaller never write that shared OS state. The
uninstall summary discloses this persistent effect. Native cold boot, rfkill and
real-air BLE reception still require Cy's test; agent Pi runs = 0.
