<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Signed releases and independent host convergence

AGPL-3.0-or-later. Nodes autonomously PULL. Command and Blue no longer initiate
software updates. Blue's crypto, pinned Ed25519 public key and applied-build
watermark remain; verify-blue is a diagnostic, not an update effector.

## Package publisher and hosting

The standalone installer artifact is built by [build-host.sh](build-host.sh),
which requires **Go 1.27.0**, disables module networking, builds Linux/ARM64
twice from separate source paths and compares bytes against the committed
installer helper pin. Its linker flag defines the helper build identity;
do not substitute a version guessed from an image tag. Ship the Go license.
The digest-pinned [Containerfile](Containerfile) is a separate build entry point;
do not assume its output equals the installer-pinned helper without reproducing
and comparing it. A failed pin comparison is a stop, not permission to rewrite
installer/bootstrap pins outside the reviewed release workflow.

```sh
# From this directory, with the required compiler already available:
./build-host.sh /path/to/new-artifact-directory
```

The helper's `publish` subcommand is a **low-level signer client**. It validates
manifest inputs, calls an explicitly configured HTTPS S5 endpoint and checks
the pinned signature and exact canonical echo. It is not a completed-release
source verifier, protected promotion approval, durable sequence allocator or
hosting deployer. Production publication needs those separate reviewed controls;
do not use this subcommand to bypass them. No private signing key is accepted.
Source availability is **not evidence that S5 is deployed**, its current CI
signature policy works, or any feed has been published. Each needs separately
authorized live verification. See the [release workflow](../../../.github/RELEASING.md).

An approved hosting operator publishes verified metadata atomically at
https://get.cybrrd.com/releases/v1/{dev,staging,general}/release.json.
Signing, serving, installation and ring promotion are distinct approvals.
Retain immutable signed history and an independent per-ring publication ledger;
never restore an older sequence as a website rollback. Verify anonymous served
bytes and cache policy after publication. **Signature, not transport, is authority.** HTTPS
distributes signed bytes; another static mirror cannot authorize a new release.
Redirects, URL credentials and oversized documents are refused.

cybrrd.release.v1 binds BOTH digest/build/revision triples, ring,
configured-ring:<ring> audience, sequence, rollout, salt, publication and expiry.
Release.canonical defines the signature array. Unknown/duplicate fields fail.
Expiry defaults to 30 days; refresh signed metadata before expiry.

Installer configuration supplies --ring (default general); the only vocabulary
is dev/staging/general. Fleet inventory is not installed configuration or a trigger.
The signed ring must match root-owned installed configuration. Only general has
partial rollout: SHA256(JSON [node_id,salt]), first eight bytes big-endian modulo
100, must be below rollout_pct. Keep salt fixed as percentage rises. Sequence
increases for every metadata change; equivocation and downgrades fail. Unchanged
components retain their digest/build; a new salt/sequence cannot reset quarantine.

## Poll, health and local recovery

Package timer: normally boot at 2–7 minutes, then 15–20 minutes after service
completion (systemd scheduling can add delay). A durable 15–20 minute
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

After downloading, both local images (including an unchanged console) must bind
the signed digest, OCI revision and component build label. Fresh clock trust and
manifest expiry are rechecked after downloads and again before journaling;
backward wall-clock movement defers the update. A slow manifest fetch must also
finish with trusted time before it can advance `last_release_check_at`.

Before downloads and before the transaction, read-only filesystem checks require
16 MiB and 64 free inodes on the private state, public projection, Quadlet and
actual rootful/rootless Podman graph stores. This is a metadata reserve, **not**
a promise the images will fit. Failed uncached pulls leave the old pair running.
Pin-write failures attempt paired local recovery without consuming a bad-image
attempt; failed recovery writes retain the journal for a later retry. Command
output is capped at 1 MiB while streaming; overflow cancels the command.

Each completed `poll`/`apply` invocation under the host lock writes a bounded
private `attempt.json` and support-visible `release_attempt.json`. Fields include
attempt times, mode/phase/error code, HTTP status, whether network was attempted,
compiled helper build and a verified console tuple when available. They contain
no URLs, response bodies or raw error text. These receipts do not alter the
existing currency/journal schema, and are not a new Silver/Red delivery claim.
`last_release_check_at` still means a valid signed check, not an attempted fetch.
A crash before completion can leave the previous attempt receipt; the durable
transaction journal is recovery authority. Receipt-write failures are reported,
never treated as durable evidence, and never gate local rollback.

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

The engine and console each have a health gate of up to 180 seconds. This is
not a single 180-second end-to-end promise: image transfers happen first,
individual external commands have a five-minute bound, and the package service
has a 20-minute start timeout. Record the installed unit configuration and
actual timing; do not diagnose an expected retry/poll floor as an absent update.

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

This command prepares UNSIGNED metadata only. Host-updater
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

**Classify the installed baseline first.** A compatible installed helper in the
current image namespaces can converge through its signed feed without reinstall.
A legacy installation or a required newer helper may need a one-time approved
join. Confirm actual helper SHA/build, image identities, ring, key, timers,
floors, recovery launcher, status and server grants before deciding.

Before an approved uninstall/reinstall, preserve nonsecret config and diagnostic
receipts, coordinate old enrollment-credential revocation/re-enrollment and owner
consent, and ensure exact approved bootstrap/installer/helper/images, upward
grants, local rollback capacity and a support path are ready. Never erase floors,
rewrite identities or reinstall to force a downgrade. The installer does not
provision server grants/streams; an engine update cannot replace the host helper.
Re-running the installer retains existing valid pins/helper, not an implicit
upgrade. See [proof and joining prerequisites](PROOF.md).

release_currency.json feeds Silver heartbeat: last valid check, sequence seen,
running sequence, actual update outcome. Telemetry exclusion/30-day disenrollment
are not implemented by this packet.

Run `go test -race ./...`, `go vet ./...`, `python3 launcher_test.py` and
`python3 tests/test_docs.py` from this directory.
[tests/run-go-proofs.py](tests/run-go-proofs.py) and
[tests/run-rework-proofs.py](tests/run-rework-proofs.py) retain positive and failing
mutation receipts from isolated copies. Set `D44_EVIDENCE_DIR` to an isolated
output directory; run only with the declared toolchain/test prerequisites.
SIGKILL tests kill real processes around
durable writes but simulate Podman/systemd/sensors: NOT physical power-cut or Pi
proof. Report the exact source head, executed denominator and remaining native
gates rather than treating these fixtures as field acceptance.

## Trust boundaries and rollout

First install is TLS-anchored: bootstrap, installer pin and host-binary pin arrive
over get.cybrrd.com. A compromised distributor could replace the entire chain.
Signature authority describes post-install updates, not an independently signed
first-install bootstrap. The host status clock-trust flag is also self-reported.
Heartbeat seen/outcome/check_at are shape-validated updater reports; root can
forge them. Running sequence is additionally bound to the engine's running digest.

The intended S5 `/sign/release` deployment must validate this exact schema and
canonical bytes, source authorization and both fixed-repository image signatures
and identities before signing. Its seed stays on S5; slot 0 is unchanged. Prove
current keyless CI signature/certificate compatibility before relying on a new
deployment; an unsigned local build is insufficient. Protected source receipt,
promotion approval, sequence ledger and independent output verification belong
on the internal publisher, never on untrusted public PR jobs.

An isolated dev proof precedes staging/general. Promote the same proved good
digests without rebuilding, with a new ring-specific signature and sequence
under separate approval. The public bootstrap switch is last, after proof and
review; a GitHub tag or published release alone does not authorize it.
