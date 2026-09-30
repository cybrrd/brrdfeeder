#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Build committed source only. No registry push, device access or deployment.
set -euo pipefail

die() { printf 'build-arm64: %s\n' "$*" >&2; exit 1; }
usage() {
  printf 'Usage: %s [--check] [TAG [OUTPUT_DIR]]\n' "$0"
  printf '%s\n' '--check performs read-only prerequisites checks; it does not pull/build/run images.'
}
check_only=false
if [[ ${1:-} == --help ]]; then usage; exit 0; fi
if [[ ${1:-} == --check ]]; then check_only=true; shift; fi
[[ $# -le 2 ]] || { usage >&2; exit 2; }
for tool in git podman tar mktemp sha256sum awk; do command -v "$tool" >/dev/null || die "missing command: $tool"; done
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo=$(git -C "$script_dir" rev-parse --show-toplevel)
[[ $(git -C "$repo" rev-parse --is-shallow-repository) == false ]] || die 'shallow checkout: fetch full history before computing BUILD_SEQ'
sha=$(git -C "$repo" rev-parse HEAD)
ref=$(git -C "$repo" symbolic-ref --quiet --short HEAD || git -C "$repo" describe --tags --exact-match HEAD 2>/dev/null || printf 'detached-%s' "${sha:0:12}")
# WHAT: reserve the first 1000 sequence values for pre-snapshot private history.
# WHY: public history starts at one commit; 1001 must exceed field floors <=824.
# This release-lineage constant must NEVER be lowered or environment-overridden.
readonly BUILD_SEQ_OFFSET=1000
commit_count=$(git -C "$repo" rev-list --count HEAD)
seq=$((BUILD_SEQ_OFFSET + commit_count))
# Commit time makes provenance stable on a rerun of the same commit.
epoch=$(git -C "$repo" show -s --format=%ct HEAD)
# Never accept an ambient/caller date as source provenance. Pass explicitly
# into the Containerfile too: exporting alone does not populate its RUN env.
export SOURCE_DATE_EPOCH="$epoch"
build_date=$(date -u -d "@$epoch" +%Y-%m-%dT%H:%M:%SZ)
tag=${1:-arm64-${sha:0:12}}
[[ $tag =~ ^[a-zA-Z0-9_][a-zA-Z0-9_.-]{0,127}$ ]] || die 'invalid image tag (1–128 ASCII tag characters)'
image="localhost/brrdfeeder-engine:$tag"
arch=$(uname -m)
case "$arch" in
  aarch64|arm64) build_platform=linux/arm64 ;;
  x86_64|amd64)
    build_platform=linux/amd64
    # Override is for the hermetic test fixture, never required operationally.
    binfmt_dir=${BRRD_BINFMT_DIR:-/proc/sys/fs/binfmt_misc}
    if [[ ! -r $binfmt_dir/status || ! -r $binfmt_dir/qemu-aarch64 ]] ||
       ! grep -qx enabled "$binfmt_dir/status" ||
       ! grep -qx enabled "$binfmt_dir/qemu-aarch64" ||
       ! grep -q '^flags:.*F' "$binfmt_dir/qemu-aarch64"; then
      printf '%s\n' 'build-arm64: ARM64 binfmt unavailable/disabled or missing F (fix-binary) flag.' \
        'The native builder stage can cross-compile, but ARM64 runtime RUN instructions cannot execute.' \
        'Ask the host administrator to run (this script never installs/registers anything):' \
        '  sudo apt-get update && sudo apt-get install -y qemu-user-static binfmt-support && sudo update-binfmts --enable qemu-aarch64' \
        'Then rerun --check and confirm /proc/sys/fs/binfmt_misc/qemu-aarch64 is enabled with flags containing F.' >&2
      exit 3
    fi ;;
  *) die "unsupported local builder architecture: $arch" ;;
esac
podman info >/dev/null || die 'local Podman unavailable (check user runtime/storage permissions)'
printf 'GIT_SHA=%s\nGIT_REF=%s\nBUILD_DATE=%s\nBUILD_SEQ=%s\nBUILDPLATFORM=%s\nSOURCE_DATE_EPOCH=%s\n' "$sha" "$ref" "$build_date" "$seq" "$build_platform" "$epoch"
if "$check_only"; then printf 'Prerequisites pass; execution/build NOT tested by --check.\n'; exit 0; fi
[[ -z $(git -C "$repo" status --porcelain --untracked-files=all -- Component/aviary) ]] || die 'commit or remove local aviary changes before building; provenance must identify the exact source'
output_dir=${2:-$PWD}
mkdir -p -- "$output_dir"
output_dir=$(cd -- "$output_dir" && pwd)
archive="$output_dir/brrdfeeder-engine-$tag.tar"
[[ ! -e $archive && ! -e $archive.sha256 ]] || die "refusing to overwrite archive or checksum: $archive"
# D36: never borrow a cached tag's base-name metadata or a previous RUN cache.
# Keep the entire isolated store on disk for audit/failure diagnosis. No prune.
build_tmp=$(mktemp -d "$output_dir/build-$tag.XXXXXXXX")
printf 'Build state (retained on success/failure): %s\n' "$build_tmp"
mkdir "$build_tmp/context" "$build_tmp/storage" "$build_tmp/runroot" \
  "$build_tmp/podman-tmp" "$build_tmp/tmp" "$build_tmp/cache"
export TMPDIR="$build_tmp/tmp" XDG_CACHE_HOME="$build_tmp/cache"
podman() {
  command podman --root "$build_tmp/storage" --runroot "$build_tmp/runroot" \
    --tmpdir "$build_tmp/podman-tmp" "$@"
}
[[ $(podman info --format '{{.Host.Security.Rootless}}') == true ]] || die 'isolated builds require rootless Podman'
[[ $(podman info --format '{{.Store.GraphRoot}}') == "$build_tmp/storage" ]] || die 'unexpected image store'
[[ -z $(podman images --all --quiet) ]] || die 'new image store is not empty'
podman info --format json > "$build_tmp/environment.json"
podman images --all --format json > "$build_tmp/images-before.json"
git -C "$repo" archive HEAD:Component/aviary | tar -x -C "$build_tmp/context"
containerfile="$build_tmp/context/engine/Containerfile"
builder_base=$(awk '$1 == "FROM" && $2 == "--platform=$BUILDPLATFORM" {print $3}' "$containerfile")
runtime_base=$(awk '$1 == "FROM" && $2 == "--platform=linux/arm64" {print $3}' "$containerfile")
for base in "$builder_base" "$runtime_base"; do
  [[ $base =~ ^[^[:space:]]+@sha256:[a-f0-9]{64}$ ]] || die "base must be digest-pinned: $base"
done
podman pull --platform "$build_platform" "$builder_base"
podman pull --platform linux/arm64 "$runtime_base"
podman image inspect "$builder_base" > "$build_tmp/builder-base.json"
podman image inspect "$runtime_base" > "$build_tmp/runtime-base.json"
# Catch inaccessible emulators before compilation, using the exact runtime base.
actual_arch=$(podman run --rm --network=none --pull=never --platform linux/arm64 \
  --entrypoint /bin/uname "$runtime_base" -m)
[[ $actual_arch == aarch64 ]] || die "ARM64 runtime probe failed: $actual_arch"
podman build --platform linux/arm64 --timestamp "$epoch" --pull=never \
  --layers --rm=false --jobs=1 --cpu-period=100000 --cpu-quota=400000 --memory=16g --memory-swap=16g \
  --build-arg "BUILDPLATFORM=$build_platform" --build-arg TARGETPLATFORM=linux/arm64 --build-arg TARGETARCH=arm64 \
  --build-arg "GIT_SHA=$sha" --build-arg "GIT_REF=$ref" \
  --build-arg "BUILD_DATE=$build_date" --build-arg "BUILD_SEQ=$seq" \
  --build-arg "SOURCE_DATE_EPOCH=$epoch" \
  -f "$containerfile" -t "$image" "$build_tmp/context"
[[ $(podman image inspect --format '{{.Architecture}}' "$image") == arm64 ]] || die 'result is not arm64'
[[ $(podman image inspect --format '{{index .Labels "org.opencontainers.image.revision"}}' "$image") == "$sha" ]] || die 'revision label mismatch'
[[ $(podman image inspect --format '{{index .Labels "com.macawi.brrdfeeder.build_seq"}}' "$image") == "$seq" ]] || die 'build sequence label mismatch'
podman image inspect "$image" > "$build_tmp/image-inspect.json"
podman image inspect --format '{{.Digest}}' "$image" > "$build_tmp/image-digest.txt"
podman run --rm --network=none --read-only --cap-drop=all --security-opt=no-new-privileges \
  --entrypoint /bin/sh "$image" -ec '
    test -x /usr/local/bin/brrdfeeder-engine
    test "$(uname -m)" = aarch64
    getcap /usr/local/bin/brrdfeeder-engine | grep "cap_net_admin,cap_net_raw,cap_sys_time=ep"
    for path in /var/log/apt/history.log /var/log/apt/term.log /var/log/dpkg.log /var/cache/ldconfig/aux-cache; do
      test ! -e "$path"
    done
    test -s /etc/ld.so.cache
    /lib/ld-linux-aarch64.so.1 --list /usr/local/bin/brrdfeeder-engine
    result=0
    output=$(/lib/ld-linux-aarch64.so.1 /usr/local/bin/brrdfeeder-engine verify-blue 2>&1) || result=$?
    printf "%s\nengine probe exit=%s\n" "$output" "$result"
    test "$result" = 2
    printf "%s\n" "$output" | grep -F "verify-blue: missing <file> argument"
    test ! -e /var/cache/ldconfig/aux-cache
    sha256sum /usr/local/bin/brrdfeeder-engine
  ' > "$build_tmp/runtime-smoke.txt" 2>&1
# Read only the synthetic locked image account's numeric day; never print the
# password field or read a host credentials file. Container root is rootless.
podman run --rm --network=none --read-only --user 0 --cap-drop=all --security-opt=no-new-privileges \
  --entrypoint awk "$image" -F: '$1 == "brrdfeeder" {print $3; found=1} END {if (!found) exit 1}' \
  /etc/shadow > "$build_tmp/shadow-lastchg.txt"
[[ $(<"$build_tmp/shadow-lastchg.txt") == "$((epoch / 86400))" ]] || die 'image account date is not the source commit day'
podman run --rm --network=none --read-only --cap-drop=all --security-opt=no-new-privileges \
  --entrypoint dpkg-query "$image" -W > "$build_tmp/runtime-packages.txt"
podman save --format oci-archive --uncompressed --output "$build_tmp/image.tar" "$image"
# No partial final artifact if build/verification/save fails; don't clobber races.
mv -n -- "$build_tmp/image.tar" "$archive"
[[ ! -e $build_tmp/image.tar ]] || die "archive appeared concurrently: $archive"
(cd -- "$output_dir" && sha256sum "brrdfeeder-engine-$tag.tar" > "brrdfeeder-engine-$tag.tar.sha256")
printf '\nArchive: %s\n' "$archive"
printf 'Image digest: %s\n' "$(<"$build_tmp/image-digest.txt")"
printf 'Private image store: %s\n' "$build_tmp/storage"
printf 'After transferring archive + checksum to the chosen node, verify the checksum, then:\n'
printf 'sudo podman load -i %q\n' "brrdfeeder-engine-$tag.tar"
printf 'Image=%s\n' "$image"
printf 'No image pushed, Quadlet edited, service restarted, or node contacted.\n'
