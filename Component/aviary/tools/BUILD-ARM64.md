<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# ARM64 engine archive build

Run as the **dedicated unprivileged build identity on an approved sandbox**
(currently worldport's `brrdbuild`), from a full CYB1 checkout. **Resonance is
out of bounds for build work, including cleanup.** the development team/the release approver deploy separately.
The script does not SSH, push an image, install packages, register binfmt,
change a Quadlet or start capture. Its networkless `verify-blue` smoke executes
the engine's argument-validation path only. `--check` only reads prerequisites.

## Normal route

On amd64, a host administrator must provision an enabled `qemu-aarch64`
binfmt handler with **F** (fix-binary: interpreter remains accessible across
container mount namespaces). The builder is native amd64 and cross-compiles;
the final Debian ARM64 stage's `RUN apt-get`, `useradd` and `setcap` still need
emulation. A builder-only success is **not** a complete engine image.

One-liner for the release approver to review/run on lamplab (not run by this packet):

```sh
sudo apt-get update && sudo apt-get install -y qemu-user-static binfmt-support && sudo update-binfmts --enable qemu-aarch64
```

Then, from a full checkout with committed aviary sources:

```sh
bash Component/aviary/tools/build-arm64.sh --check
bash Component/aviary/tools/build-arm64.sh candidate-<shortsha> /data/brrdbuild/engine-artifacts
```

The full build creates a **new image store, runroot, TMPDIR and cache directory
for every invocation**, under the output directory. It explicitly pulls both
`FROM` references by digest from the committed Containerfile, then builds with
`--pull=never`. Even the `uname -m` preflight uses the exact pinned runtime base,
not a tag. This clean-state requirement belongs to the **build procedure**;
digest-pinned FROM lines alone do not prevent cached-tag base-name metadata from
changing an image digest. The default/shared image store is never pruned or built in.

The build first runs `uname -m` inside the pinned ARM64 Debian container
with networking disabled to catch an unusable emulator before compiling.
It exports **only committed** `HEAD:Component/aviary` as the context: no local
target cache, credentials, config edits or other untracked files are copied.
It builds the **final** stage of `engine/Containerfile` (no `--target builder`),
checks ARM64 architecture, revision/sequence labels and file capabilities,
asserts the four scrub targets are absent, preserves `/etc/ld.so.cache`, and
invokes the real ARM64 loader plus the engine's offline `verify-blue` path
(expected exit 2 and the exact missing-file-argument diagnostic),
and saves:

```text
localhost/brrdfeeder-engine:<tag>
brrdfeeder-engine-<tag>.tar           (OCI archive)
brrdfeeder-engine-<tag>.tar.sha256
```

It refuses shallow history, dirty/untracked aviary sources, unpinned bases,
non-rootless builds and existing output artifacts. Failed build/save leaves no
final tar. State is **retained on failure and success** under the printed
`build-<tag>.<unique>/` path: context, private store/caches, consumed-base inspection,
image manifest digest, runtime smoke and package inventory. Repeated tags in
different isolated stores do not overwrite each other. The helper prints the
private store address; the resulting image is not loaded into the default store.
Build RUNs request four CPUs / 16 GiB / no additional swap; this is not a claim
that wrapper-service accounting covers every separate Buildah RUN cgroup.
Budget roughly 25 GB retained per cold build on the measured sandbox; archive
and prune decisions are separate, never implicit. A native ARM64 builder
needs no binfmt handler; unsupported builder architectures fail explicitly.

Metadata: full `GIT_SHA=git rev-parse HEAD`, branch/tag (or explicit detached
SHA) `GIT_REF`, UTC **commit timestamp** `BUILD_DATE`, and
`BUILD_SEQ=1000 + git rev-list --count HEAD`. The fixed 1000 offset reserves
private-history release numbers: a one-commit public snapshot builds as 1001,
above the field's 618–824 floors. Never lower the offset, override it from the
environment, rewrite published release history or build a shallow checkout.
Component/host-updater downgrade checks are unchanged. Container creation time is also the
commit timestamp; Cargo uses `--locked`. Comparisons must use the same commit,
ref, history count, builder platform/tool versions and consumed package set.
`--timestamp` clamps layer mtimes but cannot remove dates embedded inside files.
K28 found another such date: `useradd` records the password-change day in
`/etc/shadow`. The helper now overrides any ambient `SOURCE_DATE_EPOCH` with
`git show -s --format=%ct HEAD` and passes it as an explicit build argument.
The Containerfile requires a numeric value and supplies it to `useradd`.
This is build-only state, not a runtime ENV setting. A post-build guard checks
only the synthetic account's numeric day against `commit_epoch / 86400`; mismatch
refuses the archive. A test must vary dates, not merely repeat within one day:
caller epoch changes must not change an identical source's image, while a changed
source commit/date must be reflected rather than frozen to a hard-coded epoch.
The runtime APT RUN therefore removes exactly the three empirically varying logs
(`/var/log/apt/history.log`, `/var/log/apt/term.log`, `/var/log/dpkg.log`) and
`/var/cache/ldconfig/aux-cache` **before committing that layer**. It retains the
loader's `/etc/ld.so.cache` and dpkg package database. Runtime package inventories
are emitted outside the artifact for later SBOM/signing, not described as an
already signed SBOM. No arbitrary logs are deleted by a wildcard.

Three identical clean builds establish the tested repetitions, not a guarantee
about future apt repositories. Base digests are pinned; subsequent APT package
selection is still mutable. Package snapshot pinning remains a separate decision
if those consumed inputs drift. These archives are
**not cosign-signed CI publications**, and must not bypass signed Blue policy
or anti-downgrade checks. Sequence is the requested commit count, not a globally
monotonic total ordering across independent branches.

After authorized transfer to the selected node, the script prints the load
command and exact Quadlet line. Operator checks the checksum first:

```sh
sha256sum -c brrdfeeder-engine-<tag>.tar.sha256
sudo podman load -i brrdfeeder-engine-<tag>.tar
# Requested candidate Quadlet line, not applied by the build script:
Image=localhost/brrdfeeder-engine:<tag>
```

Record the current `Image=` verbatim and current running image ID before any
separately authorized deployment. Follow the node's runbook and rollback checks;
this build guide is not authority to edit a unit or restart a device.

## Historical D3 CI readiness (not the dedicated sandbox route above)

The following is the D3 assessment, not a claim that the subsequently established
dedicated sandbox lacks ARM64 execution. D36 does not enable or qualify the CI
workflow. Sandbox builds and generic runner jobs remain different execution seats.

The saved runner reference (2026-07-22, updates through 2026-09-15) describes
`resonance-runner`: rootless act_runner via rootless Podman; generic
`ubuntu-latest` jobs use `catthehacker/ubuntu:act-22.04`. Its setup-qemu/buildx
privileged registration path is explicitly documented as unavailable. The
pack-stack inventory likewise records a rootless runner, not a privileged
ARM64 builder. **No current live runner check was made: no resonance contact.**

Therefore `.gitea/workflows/build-engine-arm64.yml` is a **dormant, manual-only
variant**, not an enabled promise on the existing runner. The special
`brrd-arm64-builder` label intentionally does not match generic `ubuntu-latest`.
An administrator must first provide a trusted build seat with native local
Podman, full Git/Bash/tar tooling, working subordinate IDs/storage, and either
native ARM64 or host-provisioned binfmt visible to it. A host-executor label on
a dedicated approved build account is one possible route; do not expose the
host container socket to untrusted PR jobs. Label/config/security changes need
separate approval. The workflow itself never registers binfmt or requests a
privileged container, and has **no PR or push trigger**.

Checkout is full-history, without persisted credentials; archive upload uses
the Gitea-compatible v3 artifact protocol. Runner/action compatibility, actual
archive upload and runtime execution must be proven by the development team on that provisioned
seat before calling this CI route ready. Current readiness: **not available**.
No attempt was made to change the current runner or its labels.

## Emergency route (operator-only; not executed here)

Use only if native cross-compilation works but runtime binfmt remains absent,
and an explicitly authorized ARM64 node can assemble the runtime. This is the
2026-09-17 test-node-3 method, **not** the preferred repeatable final-stage build.

1. On lamplab, export the committed aviary tree into a fresh temporary context
   (as the normal script does). Record SHA, ref, commit date and rev-list count.
   Cross-build only the native builder; no ARM64 program runs on lamplab:

   ```sh
   podman build --platform linux/arm64 --target builder \
     --build-arg BUILDPLATFORM=linux/amd64 \
     --build-arg TARGETPLATFORM=linux/arm64 --build-arg TARGETARCH=arm64 \
     -f <context>/engine/Containerfile \
     -t localhost/brrdfeeder-builder:<tag> <context>
   podman create --name brrd-extract-<tag> --entrypoint /bin/true localhost/brrdfeeder-builder:<tag>
   podman cp brrd-extract-<tag>:/engine ./brrdfeeder-engine
   podman rm brrd-extract-<tag>
   file ./brrdfeeder-engine
   sha256sum ./brrdfeeder-engine
   ```

   `create` does not execute the image. The extraction container is uniquely
   named and removed after copying; the builder image can remain cached.

2. After separate authorization, transfer the binary, hash and metadata to the
   node. Verify the hash and ELF architecture. Record the running container's
   exact immutable local image ID (not a mutable tag), its config and verbatim
   Quadlet rollback `Image=`. Confirm base architecture is ARM64. Do **not**
   use `podman commit` on a running container: that risks baking mounted secrets
   or mutable runtime state into an image.

3. In a minimal on-node build context containing just the verified binary and
   this Containerfile, build from that immutable base. Supply all metadata
   again; the builder-stage-only image does not carry new runtime provenance.

   ```dockerfile
   ARG BASE
   FROM ${BASE}
   USER 0
   COPY brrdfeeder-engine /usr/local/bin/brrdfeeder-engine
   RUN chmod 0755 /usr/local/bin/brrdfeeder-engine && setcap cap_net_admin,cap_net_raw,cap_sys_time=ep /usr/local/bin/brrdfeeder-engine
   ARG GIT_SHA
   ARG GIT_REF
   ARG BUILD_DATE
   ARG BUILD_SEQ
   LABEL org.opencontainers.image.revision="${GIT_SHA}" org.opencontainers.image.ref.name="${GIT_REF}" org.opencontainers.image.created="${BUILD_DATE}" com.macawi.brrdfeeder.build_seq="${BUILD_SEQ}"
   ENV BRRDFEEDER_GIT_SHA="${GIT_SHA}" BRRDFEEDER_BUILD_SEQ="${BUILD_SEQ}"
   USER 1001:20
   WORKDIR /etc/brrdfeeder
   ENTRYPOINT ["/usr/local/bin/brrdfeeder-engine"]
   ```

   ```sh
   sudo podman build --platform linux/arm64 --pull=never \
     --build-arg BASE=sha256:<recorded-running-image-id> \
     --build-arg GIT_SHA=<fullsha> --build-arg GIT_REF=<ref> \
     --build-arg BUILD_DATE=<UTC-commit-time> --build-arg BUILD_SEQ=<rev-list-count> \
     -t localhost/brrdfeeder-engine:<tag> <minimal-context>
   ```

4. Inspect architecture, labels, runtime env, entrypoint, uid/workdir and file
   capabilities without starting capture. The old base retains its dependency
   versions: explicitly check new binary library requirements against it. Then
   follow the separately authorized deploy/rollback runbook. The old exact image
   must remain available; neither this guide nor D3 authorizes a node swap.
