#!/usr/bin/env bash
# Unprivileged build/SBOM only. No credentials, pushes or signatures here.
set -euo pipefail
python3 .github/scripts/release-metadata.py context
: "${RUNNER_TEMP:?}" "${COMPONENT:?}"
case "$COMPONENT" in engine|console) ;; *) exit 2;; esac
out="$RUNNER_TEMP/release/$COMPONENT"
mkdir -p "$out"
case "$COMPONENT" in
  engine)
    tag="release-$GITHUB_SHA"
    build_out="$RUNNER_TEMP/build-engine"
    bash Component/aviary/tools/build-arm64.sh "$tag" "$build_out"
    (cd "$build_out" && sha256sum -c "brrdfeeder-engine-$tag.tar.sha256")
    mv "$build_out/brrdfeeder-engine-$tag.tar" "$out/image.oci.tar" ;;
  console)
    case "$(uname -m)" in aarch64|arm64) platform=linux/arm64;; x86_64|amd64) platform=linux/amd64;; *) exit 2;; esac
    base=$(awk '$1=="FROM" && $2=="--platform=$BUILDPLATFORM" {print $3}' Component/brrdhouse/Containerfile)
    [[ $base =~ @sha256:[a-f0-9]{64}$ ]]
    podman pull --platform "$platform" "$base"
    sh Component/brrdhouse/build.sh arm64
    mv Component/brrdhouse/brrdhouse-arm64.oci.tar "$out/image.oci.tar" ;;
esac
syft "oci-archive:$out/image.oci.tar" -o cyclonedx-json > "$out/sbom.cdx.json"
python3 .github/scripts/release-metadata.py create "$COMPONENT" "$out"
