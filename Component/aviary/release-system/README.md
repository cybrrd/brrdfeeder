<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Signed releases and independent host convergence (D44 revival)

AGPL-3.0-or-later. Nodes autonomously PULL. Command and Blue no longer initiate
software updates. Blue's crypto, pinned Ed25519 public key and applied-build
watermark remain; verify-blue is a diagnostic, not an update effector.

## Package publisher and hosting

Build using this directory's digest-pinned Go 1.27.1 Containerfile.
governance/reviews/d44-revival/build-host.sh reproduces the standalone ARM64
host binary from two source paths with networking disabled. Ship its Go license.

Cy runs the publisher from an allowlisted host over HTTPS to S5; no private key
is accepted by this command (placeholders are deliberately invalid):

```sh
./brrdfeeder-release publish --signer-url https://<S5-TLS-name>/sign/release \
  --ring general --digest sha256:<engine-digest> \
  --version <engine-40-hex-revision> --build-seq <engine-build> \
  --console-digest sha256:<console-digest> \
  --console-version <console-40-hex-revision> --console-build-seq <console-build> \
  --sequence <ring-sequence> --rollout-pct 10 --salt stable-cohort \
  --out release.json
```

Publish atomically at
https://get.cybrrd.com/releases/v1/{dev,staging,general}/release.json.
This uses the EXISTING world Caddy site, read-only /etc/world/get mount and
repo-owned deployment runbook: no new service, domain or credential. Cy publishes;
this branch does not deploy. **Signature, not transport, is authority.** HTTPS
distributes signed bytes; another static mirror cannot authorize a new release.
Redirects, URL credentials and oversized documents are refused.

cybrrd.release.v1 binds BOTH digest/build/revision triples, ring,
configured-ring:<ring> audience, sequence, rollout, salt, publication and expiry.
Release.canonical defines the signature array. Unknown/duplicate fields fail.
Expiry defaults to 30 days; refresh signed metadata before expiry.

Installer configuration supplies --ring (default general); the only vocabulary
is fleet.yaml's dev/staging/general. Command's former ring names are retired.
The signed ring must match root-owned installed configuration. Only general has
partial rollout: SHA256(JSON [node_id,salt]), first eight bytes big-endian modulo
100, must be below rollout_pct. Keep salt fixed as percentage rises. Sequence
increases for every metadata change; equivocation and downgrades fail. Unchanged
components retain their digest/build; a new salt/sequence cannot reset quarantine.

## Poll, health and local recovery

Package timer: boot at 2–7 minutes, then 15–20 minutes. A durable 15–20 minute
floor precedes EVERY HTTP attempt, including failures/restarts. A truck offline
for a week fetches the latest release on return; no received Blue is needed.

poll stages the verified signed object into the root-private 0700
/var/lib/brrdfeeder-updater/pending_update.json mailbox (0600)
and invokes the host effector under a root-only lock. PathExists is retired and
never installed: no retained-mailbox activation loop. Both changed images finish
downloading and pass exact RepoDigests checks before either service is disturbed.
An existing staged release is processed before contacting the manifest server.
If a pull fails, only an exact locally cached RepoDigest can be used. Invalid,
expired or non-regular slots are cleared with a private rejection-reason receipt;
symlinks/FIFOs are never followed. No engine-writable mailbox is consumed.
Expiry-sensitive package and host-update decisions require a fresh explicit
os_clock_trusted=true report. An untrusted clock leaves a staged release intact.

Running zero-capability, networkless hold containers reference outgoing images
in the rootful engine/rootless console stores. They bind the static HOST binary
at /hold; scratch images need no utilities. Running anchors prevent image/system
prune from deleting the last-known-good images. An older anchor is removed only
after a subsequent success. Forced anchor/image deletion is outside the guarantee.

A fsynced private journal precedes atomic Quadlet rename + directory fsync.
Both components commit or both restore their exact previous pins. An independent
successful release leaves the unchanged component running; recovery may restart
both to establish a new measured health window.

Engine health requires two distinct post-switch status writes: correct node,
actual image digest, compiled build/revision, radio up, and required GPS/trusted
clock. Missing, invalid, future or age **greater than 3 × declared interval**
is unhealthy. Console must return HTTP 200 and render fresh healthy engine
status with the expected digest/revision. This proves process/sensor/console
health, NOT downstream ingestion.

Throughout the gate, both units' NRestarts AND both containers' RestartCount must
stay unchanged. Invocation/container IDs detect reset/replacement. Planned starts
reset the unit counter and must first be observed at zero. Any automatic restart
or unknown counter fails even if active/status freshness look healthy.

brrdfeeder-release-recover.service runs at boot without a network dependency.
An unconfirmed journal rolls back; a confirmed one finishes commit. Recovery
uses LOCAL images and host code, never pulls. Two failed applications quarantine
the digest pair. Interrupted switches do not consume failed-health attempts.
Boot recovery checks restored pins and running previous containers, not radio/GPS
or HTTP readiness. A missing user manager/start failure retains recovery_wait
for the next timer; it never permanently quarantines a good pair. Retries do not
restart an already-restored engine. rollback_failed is a retryable local receipt,
not proof of a defective candidate. Broken storage or a broken retained image cannot be
fixed by software rollback; no false recovery is reported.

The host writes update_outcome.json. D40 spools operational/Red reports and sends
when transport returns. A dead engine cannot deliver Red; the host receipt
survives. No new credentials or shell NATS publish are introduced.

## Separate host-updater releases

The host executable has its OWN install SHA256 pin. Neither workload image
contains it or supplies rollback code. A package manifest cannot replace it.
A separate timer/command poll-updater fetches <ring>/updater.json, never within
the engine/console transaction:

```sh
./brrdfeeder-release publish-updater --updater-only \
  --ring dev --architecture arm64 \
  --sha256 <standalone-executable-sha256> --build-seq 2 --sequence 1 \
  --salt updater-two --out updater.unsigned.json
```

Friday ruling: this command prepares UNSIGNED metadata only. Host-updater
signing/publication is blocked pending a separately validated S5 endpoint; no
caller loads the private key. The installed verifier/recovery remains tested.
Distinct signed domain cybrrd.host-updater.v1, explicit updater_only=true,
architecture, ring/cohort, sequence/build, expiry and SHA256 are all checked.
Build the example with -X main.updaterBuild=2; candidate identity must match.
Serve its bytes at /releases/v1/updater/<sha256>/linux-arm64/brrdfeeder-release.
No manifest URL can redirect the artifact downloader. Each failed executable
retains its attempt receipt; 32-entry history cap then requires operator review.

The current binary is retained as .previous with a local checksum. The host
transaction is fsynced before replacement; a bounded startup test must pass.
An independently installed Python3 supervisor restores the previous executable
after interruption or candidate startup failure, without containers or network.
This small supervisor is NOT self-replaced by updater.json; changing this
recovery root requires a reviewed installer/reinstall.

## Installation, evidence and limits

First move onto D44: local uninstall + one-liner with approved D44 engine/console
images and independently pinned host artifact. No automatic migration. Installer
requires canonical D17 status and D40 upward spool but does not provision server
grants or streams. Those remain operator-gated.

release_currency.json feeds Silver heartbeat: last valid check, sequence seen,
running sequence, actual update outcome. Telemetry exclusion/30-day disenrollment
are not implemented by this packet.

Run go test -race ./..., go vet ./..., python3 launcher_test.py.
governance/reviews/d44-revival/run-go-proofs.py retains positive and failing
mutation receipts from isolated copies. SIGKILL tests kill real processes around
durable writes but simulate Podman/systemd/sensors: NOT physical power-cut or Pi
proof. The revival report records the executed denominator and outstanding gates.

## Trust boundaries and Friday rollout

First install is TLS-anchored: bootstrap, installer pin and host-binary pin arrive
over get.cybrrd.com. A compromised distributor could replace the entire chain.
Signature authority describes post-install updates, not an independently signed
first-install bootstrap. The host status clock-trust flag is also self-reported.
Heartbeat seen/outcome/check_at are shape-validated updater reports; root can
forge them. Running sequence is additionally bound to the engine's running digest.

S5 `/sign/release` validates this exact schema/canonical bytes, source allowlist,
optional bearer and cosign on both fixed-repository digest targets before signing.
Key slot 0 is unchanged; its private key stays on S5. Publishing unsigned local
builds is insufficient: the existing trusted-CI cosign gate must also pass.
See governance/reviews/d44-rework/FRIDAY.md for isolated /dev install, N→N+1,
same-digest ring promotion and the separately approved public switch.
