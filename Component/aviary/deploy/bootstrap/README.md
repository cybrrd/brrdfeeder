<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# BRRDfeeder bootstrap kit and installer refresh

D12 supports the containerized fleet. D11 packages the release after D12 merges;
this source change is not a published kit or an authorized deploy. Current and
clearly separated historical build metadata are in `manifest.txt`; a built
kit's `KIT-MANIFEST` is authoritative for file verification.

## Kit layout

Retain the historical single top-level `brrdfeeder-bootstrap/` directory.
Keep installer and companions together after extraction. The flat kit contains:

| File | Use |
| --- | --- |
| `brrdfeeder-install.sh` | Config selection, substrate detection, previews, receipts and refresh |
| `brrdfeeder-blackbox-flush.sh` | Bounded persistent journal tail |
| `brrdfeeder-rfkill-boot-state.sh` | Bluetooth unblock and persisted restore state |
| `brrdfeeder-image-identity.sh` | D33 per-start host inspection; include beside the installer in every new kit |
| `brrdfeeder-engine.container` | Template for an existing rootful Quadlet |
| `99-cybrrd-brrdfeeder.rules` | Reference; installer renders matching 0660/dialout rules inline |
| `config.yaml.template`, `config.yaml.mobile.template` | References; never replace a fleet config with a template |
| `engine` | Historical kit payload; D11 supplies the verified ARM64 binary; refresh does not install it |
| `README.md`, `manifest.txt`, `KIT-MANIFEST` | Instructions, historical metadata, current commit/file hashes |

In the repository the template is `../quadlet/brrdfeeder-engine.container`;
in the flat kit it is beside the installer. Missing templates on container
nodes fail before mutation. Credentials and other secrets are never kit contents.

## Modes and prerequisites

Run as root through sudo, with Bash, coreutils, awk, diffutils,
systemd, udev, usbutils, rfkill, and the existing runtime installed. The script
does not install packages or fetch an image.

Explicitly select the existing non-root account with
`sudo BRRDFEEDER_LEGACY_USER=operator bash brrdfeeder-install.sh ...` (replace
`operator` with the account's actual name). Its passwd home must be a dedicated
`/home/<account>` directory. No retired personal-account defaults are recognized.
The account and home examples below are illustrative, not fleet inventory.

- `--verify`: read-only inspection. It never adds dialout membership or writes
  audit files. Missing optional hardware/state is reported as a warning;
  exit 0 means inspection completed, not that a live health gate passed.
- `--dry-run`: print commands, writes, Quadlet diff, and prospective receipt
  path without changing files, groups, services, or rfkill.
- `--config <absolute-path>`: select an existing regular config. Without it,
  choose the first existing candidate: `/etc/brrdfeeder/config.yaml`, then
  `/home/operator/brrdfeeder/config.yaml`, then `/home/operator/config.yaml`.
  Print path and reason. Missing explicit path or no candidate exits 2 naming
  the candidates. Paths allow letters, digits, `_`, `.`, `/`, `-`; no symlinks.
  The resolved parent directory must not be group/world-writable: modes such
  as 0775/0757/0777 exit 2 before mutation, including previews; 0700/0755 are
  accepted. Refusal prevents a different local user from replacing a validated
  root-consumed config. Keep its owner and ancestor directories trusted too.
- `--restart-engine`: permit a rootful engine restart. Apply without it
  refreshes files, reloads systemd, prints `restart required`, and exits **3**.
  Treat this as pending restart. Even a successful explicit restart needs the
  operator NATS/heartbeat gate.
- `--audit-log=/absolute/path`: private apply trace. Previews stay on stdout.
  Apply receipts preserve any pre-existing audit file as well.

Presence of `/etc/containers/systemd/brrdfeeder-engine.container` selects the
rootful path. Render from the checked-in template, preserving its single running
`Image=` line verbatim (including whitespace). The only other substitution is
the config bind's host source, from the selected config; its container
destination remains `/etc/brrdfeeder/config.yaml`. Review the diff for device,
credential-volume, capability, UID/GID, and hardening changes. Ambiguous/missing
`Image=` fails. This path has no bare-metal binary precondition, user-service
creation, linger change, or process handoff.

Generated udev rules, journald drop-in, black-box units, Quadlet and legacy unit
are staged with final ownership/mode in same-directory temporary files and
renamed into place. Failed staging leaves the old target intact. This provides
atomic visibility, not a power-loss durability guarantee. Systemd daemon-reload
is issued only for changed unit content (rollback still reloads restored files).

The fleet uses rootful Quadlets. A future operator rootless unit would live at
`/home/operator/.config/containers/systemd/brrdfeeder-engine.container`. If found
without a rootful unit, exit 2 for review instead of entering the legacy path.
Other users' rootless installations are outside this procedure.
If the canonical rootful unit is absent, the installer also refuses legacy mode
when any `/etc/containers/systemd/*.container` mentions `brrdfeeder` (including
comments/case variants) or the system `brrdfeeder-engine.service` is active.
Review renamed/container-managed installations instead of starting a second
legacy engine; these checks do not modify or stop the detected service.

## Per-node refresh — only when the release approver authorizes the roll

| Node | Config passed to every invocation |
| --- | --- |
| test-node-3 | `/etc/brrdfeeder/config.yaml` |
| test-node-1 | `/home/operator/brrdfeeder/config.yaml` |
| test-node-2 | `/home/operator/config.yaml` |

Run one node at a time using its established Tailscale management address and
the release's exact archive/hash. Transport from lamplab follows the existing
`scp brrdfeeder-bootstrap-kit-<sha12>.tar.gz operator@<node>:` convention.
On the node, verify the release archive SHA-256, extract into a fresh private
staging directory, and verify `KIT-MANIFEST` as instructed by that release.
Do not copy the bundled engine or template over running files.

```sh
mkdir -m 0700 ~/bootstrap-refresh-<sha12>
tar xzf ~/brrdfeeder-bootstrap-kit-<sha12>.tar.gz -C ~/bootstrap-refresh-<sha12>
cd ~/bootstrap-refresh-<sha12>/brrdfeeder-bootstrap
CFG=/etc/brrdfeeder/config.yaml  # test-node-3; use the table for other nodes
LEGACY_USER=operator  # replace with the existing operator account
sudo BRRDFEEDER_LEGACY_USER="$LEGACY_USER" bash ./brrdfeeder-install.sh --config "$CFG" --verify
sudo BRRDFEEDER_LEGACY_USER="$LEGACY_USER" bash ./brrdfeeder-install.sh --config "$CFG" --dry-run
sudo BRRDFEEDER_LEGACY_USER="$LEGACY_USER" bash ./brrdfeeder-install.sh --config "$CFG"
# Expected exit 3; capture the printed installer receipt path.
```

Expected diff: unchanged `Image=`; template's `User=1001:20`, dropped/all then
three explicit capabilities, read-only root, journald, config bind, and
black-box timer dependency replace older settings. Confirm the pinned image,
credentials, and device permissions support that UID/capability configuration.
Inspect the receipt's MANIFEST and resulting Quadlet before restarting.
Image swaps are separate `notes/ops-2026-09-18/roll-engine.sh` work;
the installer never substitutes the template's `:verified` tag.

When authorized, supply `--restart-engine` during apply or restart the refreshed
unit separately:

```sh
START=$(date -u +%Y-%m-%dT%H:%M:%SZ)
sudo systemctl restart brrdfeeder-engine.service
sudo systemctl is-active brrdfeeder-engine.service
sudo podman logs --since "$START" brrdfeeder-engine 2>&1 | grep -F 'nats] connected'
```

Within **180 seconds**, require engine active, a fresh `nats] connected` marker,
and a new heartbeat for this exact node observed by the existing authorized
fleet/ingest observer. Recheck during the window; an old heartbeat or only an
active process does not pass. Record node ID and timestamps in operator notes
in the receipt. The installer does not claim live heartbeat verification.
If any gate fails, restore the receipt and health-gate a restart of the old config.

## Receipt and rollback

Before mutation, each apply creates root-private
`/var/lib/brrdfeeder-deploy/<UTC timestamp>-<pid>/installer/`.
`FILES` maps paths to numbered pre-change `files/` copies; absent paths are
recorded for removal. `MANIFEST` contains path, action, SHA-256 before/after,
even on partial failure; copies retain metadata. Coverage includes config,
udev, journald, fstab, Quadlet or legacy unit, image-identity helper, black-box helper/units/lock/tails,
existing Bluetooth restore files, optional audit log, and private sudoers and
group-membership evidence.

```sh
sudo bash /var/lib/brrdfeeder-deploy/<receipt>/installer/ROLLBACK.sh
```

Rollback stops black-box writers, restores/removes files, reloads systemd and
udev, restarts journald, and restores the prior timer enabled/active state.
Engine restart remains the operator's decision. It prints runtime state it
cannot undo: mounts, Bluetooth soft-block state, and legacy linger/session state.
If operator was added to dialout, the manual inverse is
`sudo gpasswd -d operator dialout` after reviewing current needs, followed by a
fresh user manager/session. Never restore a whole group database over later
account changes. Sudoers is evidence only and was not modified. Empty new
directories and receipt logs remain. Keep receipts private: config and journal
data may be sensitive. Repeat the engine/NATS/heartbeat gate after rollback.

## D33 runtime identity (rootful Quadlet refresh)

The installer installs the companion as root-owned mode 0755 at
`/usr/local/libexec/brrdfeeder-image-identity`, covered by the receipt. The
Quadlet owns `/run/brrdfeeder-identity` through `RuntimeDirectory`; the engine
gets a read-only mount. `ExecStartPre` invalidates prior state, and
`ExecStartPost` inspects the actual new container via Quadlet's cidfile, never
the mutable tag. The record and container environment share the systemd
invocation ID; an old invocation is rejected. No identity is persisted in
config or an install-time environment file.

Both hooks are nonfatal. The engine waits at most eight seconds for a valid
root-owned, non-group/world-writable regular record, then continues with an
absent digest and `running_identity_unverified` if resolution is unavailable.
This failure blocks updates, not sensing startup. Other existing startup gates
(capabilities/config/GPS) are unchanged. Do not change `Notify=false` to
application/health readiness: it would make this post-start handoff circular.

Build the new engine with the existing `tools/build-arm64.sh` route (which
derives GIT_SHA and BUILD_SEQ from committed source). The Containerfile now
bakes those arguments in the **builder** stage. Direct Cargo builds may provide
`BRRDFEEDER_BUILD_GIT_SHA` (40 lowercase hex) and `BRRDFEEDER_BUILD_SEQUENCE`
(positive u64) at compile time. Missing/malformed values are omitted, never
replaced with `unknown` or zero on Silver. Runtime `BRRDFEEDER_GIT_SHA`,
`BRRDFEEDER_BUILD_SEQ` and `BRRDFEEDER_IMAGE_DIGEST` are ignored. Build metadata
is an artifact-bound build assertion, not an independently verified provenance
chain. OCI labels are inventory assertions, not the engine's identity source.

An inspected digest alone does not authorize updates: digest plus both valid
baked metadata fields are required. Partial identity keeps the named refusal
and `management_status: blocked`. A known identity does not create `policy_ack`.
Digest means Podman's running-container manifest digest, not its image-config
ID, tag, base-image digest, or an assumed multi-platform index digest. Registry
index/target normalization is a separate update-plane concern.

The D33 report records real ARM64 **rootless sandbox** Quadlet mutation, not a
field/rootful hardware deployment. A future authorized rollout must install
the helper and refreshed unit along with the new engine; verify the reported
digest against the running container and the named refusal under unavailable
identity. Do not reuse an older flat kit lacking this companion.

## BRRDfeeder-tier behavior and legacy installs

The selected config drives `node.storage_class`: only `ephemeral` or
`persistent`, absent defaults persistent; typos abort by value before mutation.
BRRDfeeder retains volatile journald, the existing `/home/operator/capture` 50 MiB tmpfs,
and D8's two-tail black box (<=1 MiB, changed writes only, private 0600 lock in
a 0700 directory). The capture mount is the legacy capture location; container
forensic paths remain governed by image/config and are not relocated by D12.
Persistent skips BRRDfeeder ephemeral-storage setup and disables an enabled black-box timer; it does
not remove previously installed volatile-journal/tmpfs configuration. Review
existing state when changing tiers. Both paths persist Bluetooth unblocked
through per-device systemd-rfkill state when hardware is present.

**Deprecated bare-metal path:** only when neither known Quadlet exists. Retain
the historical kit layout, place its ARM64 engine at
`/home/operator/brrdfeeder-src/engine/target/release/engine`, and reapply
`cap_net_admin,cap_net_raw,cap_sys_time=ep` after extraction (tar loses file
capabilities). Provision `/home/operator/config.yaml` and credentials separately,
and keep helpers beside the installer. This historical binary reads relative
to its home working directory: a different selected legacy config exits 2.
Preview with `--config /home/operator/config.yaml --verify` and `--dry-run`.
Apply retains the historical automatic user-service handoff/restart, prints
deprecation, and records a rollback receipt. Do not use this legacy procedure
on test-node-1, test-node-2, or test-node-3.
