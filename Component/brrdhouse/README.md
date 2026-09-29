# BRRDhouse — read-only console

D43 implements ADR 0007's public LAN status page. Plain HTTP, no login, actions,
accounts, credentials, enrolment or updates. Go `net/http`, embedded
`html/template`, htmx 2.0.10 and SSE extension 2.2.4. Assets are local; there are
no runtime Go dependencies, CDN calls or broker connections. Tier 1 reads D17's
schema-1 `status.json`; Tier 3 local NATS remains the target. No Tier 2 API.

The package's GPS pre-start helper also writes `startup.json` in the same public
directory. This is **not an engine heartbeat**: `gps-missing`, `gps-waiting` and
`gps-busy` (and the brief `gps-fix` handoff) always render offline with owner-facing guidance. Its fixed five-second
interval has a three-interval freshness budget. A stale, invalid or future-dated
record remains offline, even if an old engine report is still fresh. The helper
removes it before launching the engine. Neither file exposes account identifiers;
the console still has only the existing read-only mount and no control interface.
The startup view also projects numeric receiver observations: satellites used/in
view, fix quality/mode, maximum/average SNR and last valid NMEA age at report time.
Missing/expired observations are unknown, not zero. Complete GSV cycles supply
SNR; absent SNR samples are not averaged as zero. The waiter expires observations
after 15 seconds, and the console suppresses the entire view if the record is stale.
These fields require this updated console image; the previous image ignores them.

## Build and publish handoff

From a rootless build account on an amd64 build host, from this directory:

```sh
podman pull docker.io/library/golang@sha256:966278043a40889499db9b0cd196fc789c37c385d41bd9a10cb1e7764af60cdc
sh build.sh arm64
```

The pinned Go 1.27.1 amd64 builder cross-compiles a static ARM64 binary. Podman
5.4.2 `--timestamp 0`, no build cache, disabled build networking, fixed compiler,
trimmed paths, no binary VCS metadata/build ID and an explicit revision label make
the OCI manifest reproducible for identical input files. Runtime is scratch,
UID/GID 65532. Reproduction requires an amd64 build host and the pinned builder
in local storage; ARM64 runtime requires an ARM64 host or registered emulation.
`sh build.sh amd64` builds the native sandbox smoke-test variant.

No repository URL is required to build or start the console. The helper derives
the full revision from Git HEAD and refuses uncommitted console files. The
`SOURCE_REVISION` build argument supplies only the
`org.opencontainers.image.revision` OCI provenance label; it is not compiled
into the binary or displayed on the page. Direct Containerfile builds can omit
the label; use the helper for release provenance. There is no page source link,
repository link, revision link or missing-source message. Selection of a public
repository URL is no longer an image-build/release blocker for this change.

Only Cy publishes, using the existing publication process and credential:

```sh
skopeo copy --preserve-digests --authfile /path/to/publish-auth.json oci-archive:brrdhouse-arm64.oci.tar docker://ghcr.io/cybrrd/brrdhouse:d43-arm64
```

That command is a handoff, not an executed step. Signing, release promotion and
release pin selection remain release work. Supply the resulting manifest digest,
not a mutable tag, through the standard installer's --console-image parameter. The report records the produced digest and archive.
Podman storage uses uncompressed layers; the OCI archive uses compressed layers
and therefore has a different manifest digest. The archive's digest is the
publication identity. `--preserve-digests` prevents silent conversion on publish.

## Standard package installation

BRRDhouse is intrinsic to the BRRDfeeder package. Use the
[customer installer](../brrdfeeder/install/README.md), once, with separately
approved engine and console digest pins, capture/location inputs, and the
intended LAN listener. There is **no separate console installation**.

The installer creates the locked `brrdfeeder` service identity and a separate,
locked non-login `brrdhouse` account for the rootless console. It installs and
executes the exact shipped `deploy/provision-status.sh` automatically; an
equality test prevents its embedded copy from drifting. It configures
`node.status_file: /var/lib/brrdfeeder-status/status.json`, mounts that dedicated
directory RW into the non-root engine and RO at the same path into the console,
installs both Quadlets, enables console-user linger, and starts both services.
`deploy/brrdhouse.container` is a reference contract, not a separate installation
procedure. No manual provisioning, config/mount edit or console activation is needed.

The directory belongs to the resolved engine service UID/GID with mode **0755**.
D17 atomically replaces status with **0644** files owned by that identity.
The console cannot access the engine's credential/config/private-state directories.
Neither engine-root execution nor 0777/group-writable status storage is used.
The provisioner rejects unsafe identities, symlink paths and unexpected contents.

The effective status interval defaults to 30 seconds. The installer enables D17;
starting a console alone does not enable the writer. The rootless console grants
zero capabilities, no new privileges, a read-only root and no writable tmpfs,
with a 64-process limit and a 96 MiB memory limit only when the installer confirms
memory control is delegated to its user service. Otherwise no memory cap is applied
and the install log says why; all other hardening remains. It receives no Podman/D-Bus/agent socket,
hardware device, account file or registry credential.

The listener is explicit and non-wildcard. Exact allowed HTTP authorities are
generated from the supplied literal LAN address and unit hostname (with ports).
Unknown Host, cross-site Fetch Metadata, foreign Origin and write methods are
rejected; forwarded headers are ignored. Do not expose this HTTP service on the
WAN. Loopback is permitted for local testing but does not provide LAN access.

The directory mount is essential: the engine writes using rename and unlinks on
graceful shutdown. Each request/SSE tick reopens the file and fails closed for
missing, unreadable, malformed, oversized, nonregular or symlink input; unknown
schema, invalid/zero/overflow interval, future time or explicitly untrusted
engine clock also means offline. Age **greater than 3 × the reported effective
interval** means offline; equality is still within budget. A new tick arrives
every second. SIGKILL can leave a recent report displayed until the budget
expires, plus one refresh tick and scheduling delay. Liveness is inferred, not
probed. The clocks must be correct; absent optional clock-trust metadata is not
itself evidence that the engine clock is wrong.

Offline snapshots suppress live subsystem/software details. Bluetooth `state`
is the engine's current RID.BLE health. `inventory.rid_ble.current_rfkill` is
re-observed after the unblock attempt and on every engine status snapshot;
its two block flags and observation time must be complete and fresh before
the console calls it blocked or clear. A current block raises attention even
if health has not yet changed from Healthy. Failed reception also raises attention.
The older `inventory.rid_ble.rfkill` field is preflight **history only** and
never raises a current block warning. New consoles paired with older engines
show current Bluetooth state/block as unknown, not falsely healthy or blocked.
An unreadable, missing or replaced kernel switch is unknown; another adapter's
state cannot substitute. No new credentials, diagnostic text or sysfs paths are
published. These additive schema-1 fields require rebuilding **both engine and
console images**, then the normal reviewed release/pin selection; changing the
console alone cannot make an old engine supply current observations. This fix
does not change installer pins or authorize publishing/deployment.
Missing values are unknown. The public projection omits
node/account/enrolment IDs, positions and raw diagnostics. D17's GPS path is
`inventory.gps.device`; unknown `path`/`gps_device` keys are deliberately omitted.
Every displayed source string is escaped as text, and CR/NUL bytes are stripped at the
projection so a hostile value cannot alter SSE event framing. No source text becomes a
URL, script, CSS, template HTML, command or trusted htmx attribute.

If SSE fails or no message arrives for four seconds, the browser hides the
snapshot and shows status unknown. A sleeping tab checks again on return. With
JavaScript disabled, the page describes itself as a snapshot requiring reload.
HTTP is unauthenticated: LAN readers see device inventory. Host validation is
the primary DNS-rebinding mitigation; do not expose this page to the internet.

## Verification and licensing

Run `go test -race -count=1 -v ./...` and `go vet ./...` inside the pinned builder
container. Set `D43_EVIDENCE` to collect the four rendered proofs. The review
harness inverts/restores the freshness comparison, records the expected failing
stale assertion and renders the stale mutant as healthy.

AGPL-3.0-or-later applies to this console from day one; see `LICENSE`. htmx core
and SSE retain their upstream licenses in `licenses/`. Pinned asset checksums
are in `vendor.sha256` (a final newline is normalized when vendoring). Runtime
images neither bundle nor serve `/source.tar.gz`; that path returns 404. The
console retains its existing license notice and `/LICENSE` link, but does not
feature source/repository/revision disclosures on the website page, per Cy's
amended item 4. Corresponding source is offered under the included license;
release licensing agreements and accompanying documentation must state how to
obtain the exact corresponding source, including build and dependency/license
files. Keep those source-offer details in the licensing documentation, not the
console UI. Removing the page link does not change LICENSE or its obligations.
Rebuild the console image for this change;
registry/image names, service accounts and installer pins are not renamed here.

Second cut: TLS-authenticated administration, claim-code/password recovery,
unit-agent actions, binding, signed updates/rollback and native physical Pi
acceptance. This page does not complete the management console.

## GPS reception rating

Both startup and running POSITION cards use `gps.go`'s single policy. Defaults:
Good = confirmed fix AND at least 6 used satellites AND HDOP ≤2; Marginal =
confirmed fix AND at least 4 used satellites AND HDOP ≤5; Poor = another confirmed
fix (including missing quality measurements); No fix = no confirmed fix.
HDOP is geometry, not a measured accuracy or safety guarantee. Missing values are
not zero. Stale reports do not receive a reception rating.

Operators can tune the console's `Exec` arguments with `--gps-good-min-sats`,
`--gps-good-max-hdop`, `--gps-marginal-min-sats`, `--gps-marginal-max-hdop`.
Invalid/inverted/non-finite thresholds refuse startup. No write/admin UI is added.
The waiter has no HDOP field yet: a transient confirmed waiter fix therefore
cannot be Good; it remains engine-offline. The engine does not publish satellites
in view or per-satellite SNR; those are explicitly marked not reported.

Whenever reception is not Good, the card explains what the service is doing and
shows: GPS satellites are mostly in the southern sky. Put the GPS where it can see
south and overhead. North-facing windows, metal roofs and tree cover can prevent
a fix. This placement advice reflects the northern-hemisphere installation in
this packet; a clear overhead view remains important everywhere.
