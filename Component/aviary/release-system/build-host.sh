#!/usr/bin/env bash
# Reproducible ARM64 artifact only: no installer execution, signing or publishing.
set -euo pipefail
[[ $# == 1 ]] || { echo 'usage: build-host.sh OUTPUT_DIR' >&2; exit 2; }
source_dir=$(cd -- "$(dirname -- "$0")" && pwd)
mkdir -p -- "$1"
out=$(cd -- "$1" && pwd)
[[ $(go env GOVERSION) == go1.27.0 ]] || { echo 'Go 1.27.0 required' >&2; exit 2; }
export CGO_ENABLED=0 GOOS=linux GOARCH=arm64 GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
build() {
  go build -a -mod=readonly -trimpath -buildvcs=false \
    -ldflags='-s -w -buildid= -X main.updaterBuild=1' -o "$1" .
}
(cd "$source_dir" && build "$out/brrdfeeder-release-arm64")
cp -R "$source_dir" "$tmp/source"
(cd "$tmp/source" && build "$tmp/second")
cmp "$out/brrdfeeder-release-arm64" "$tmp/second"
pin=$(sed -n 's/^readonly RELEASE_HELPER_SHA256="\([a-f0-9]*\)"/\1/p' "$source_dir/../../brrdfeeder/install/brrdfeeder-install.sh")
[[ $pin =~ ^[a-f0-9]{64}$ ]]
printf '%s  %s\n' "$pin" "$out/brrdfeeder-release-arm64" | sha256sum -c -
(cd "$out" && sha256sum brrdfeeder-release-arm64 > brrdfeeder-release-arm64.sha256)
cp "$(go env GOROOT)/LICENSE" "$out/GO-LICENSE"
