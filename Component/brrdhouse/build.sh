#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
set -eu
cd "$(dirname "$0")"
arch=${1:-arm64}
case "$arch" in arm64|amd64) ;; *) echo 'usage: sh build.sh [arm64|amd64]' >&2; exit 2;; esac
case "$(uname -m)" in
  aarch64|arm64) build_platform=linux/arm64 ;;
  x86_64|amd64) build_platform=linux/amd64 ;;
  *) echo 'unsupported native compiler architecture' >&2; exit 2 ;;
esac
tag="ghcr.io/cybrrd/brrdhouse:d44-$arch"
# Build provenance stays in the OCI label, never in the console page.
revision=$(git rev-parse --verify HEAD)
if [ -n "$(git status --porcelain --untracked-files=all -- .)" ]; then
  echo 'Refusing revision label for uncommitted console files; commit them first.' >&2
  exit 2
fi
if [ "$(git rev-parse --is-shallow-repository)" != false ]; then
  echo 'Refusing shallow checkout: fetch full history before computing BUILD_SEQ.' >&2
  exit 2
fi
# WHAT: reserve 1000 values for private-history releases before the public snapshot.
# WHY: its first commit yields 1001, above field floors <=824. NEVER lower this
# constant or accept a caller/environment override of the release lineage.
readonly BUILD_SEQ_OFFSET=1000
commit_count=$(git rev-list --count HEAD)
sequence=$((BUILD_SEQ_OFFSET + commit_count))
# WHAT: the deliberate drill health failure is keyed on the release identity
# (the engine product version, exactly as release-metadata.py asserts it
# against the tag) and reaches the console only as a runtime ENV baked here.
# WHY: fixture/test builds never pass this build-arg, so their consoles stay
# healthy and lifecycle tests exercise real rollback mechanics; no flag or
# setting can enable the failure in any other build. WHEN-to-change: when the
# drill retires, delete this derivation together with the ENV gate in main.go.
drill_unhealthy=0
engine_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' ../aviary/engine/Cargo.toml | head -n 1)
if [ "$engine_version" = "0.8.31-drill.1" ]; then
  drill_unhealthy=1
fi
# OCI created timestamps and all layer timestamps are fixed; no mutable labels,
# tags, downloads or dependency resolution are inputs to the build. The source
# revision and monotonic build sequence are explicit immutable build inputs.
podman build --no-cache --network=none --arch "$arch" --timestamp 0 \
  --build-arg "BUILDPLATFORM=$build_platform" \
  --build-arg "TARGETARCH=$arch" --build-arg "SOURCE_REVISION=$revision" \
  --build-arg "BUILD_SEQ=$sequence" \
  --build-arg "BRRDHOUSE_DRILL_UNHEALTHY=$drill_unhealthy" \
  --format oci -f Containerfile -t "$tag" .
podman image inspect "$tag" --format '{{.Digest}} {{.Architecture}} {{.Config.User}}'
podman save --format oci-archive -o "brrdhouse-$arch.oci.tar" "$tag"
printf 'OCI archive manifest (compressed transport digest):\n'
tar -xOf "brrdhouse-$arch.oci.tar" index.json
printf '\nPublish (the release approver only): skopeo copy --preserve-digests --authfile /path/to/publish-auth.json oci-archive:brrdhouse-%s.oci.tar docker://%s\n' "$arch" "$tag"
