#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
set -euo pipefail
mode=${1:-}
case "$mode" in build|publish) ;; *) echo 'usage: setup-release.sh build|publish' >&2; exit 2;; esac
sudo apt-get update
sudo apt-get install -y --no-install-recommends skopeo=1.13.3+ds1-2ubuntu0.24.04.3
if [[ $mode == build ]]; then
  sudo apt-get install -y --no-install-recommends podman=4.9.3+ds1-1ubuntu0.2 uidmap=1:4.13+dfsg1-4ubuntu3.2
fi
case "$(uname -m)" in
  aarch64) arch=arm64
    cosign_sha=426193b4c5da4d4d643e822f48fe0cc8a476ca1782a272704831f5a0cef716d7
    syft_sha=c46d5e4c28e12aa4c5becfaa343ef1c7f89045b6b895f2c21d471c62db09c706
    gh_sha=b1a0c0a0fcf18524e36996caddc92a062355ed014defc836203fe20fba75a38e ;;
  x86_64) arch=amd64
    cosign_sha=c3b4f5410e608af03a5eb0aaac84a4313d8da131248e08ff1759ac70c79d1644
    syft_sha=caeedb81fb0491615f1ebd1761e4145d41ee86dd2cc7bf80669f9f5ad9d6133d
    gh_sha=ca6e7641214fbd0e21429cec4b64a7ba626fd946d8f9d6d191467545b092015e ;;
  *) exit 2 ;;
esac
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fetch() { curl --fail --show-error --location --max-time 120 "$1" -o "$2"; }
if [[ $mode == build ]]; then
  fetch "https://github.com/anchore/syft/releases/download/v1.52.0/syft_1.52.0_linux_$arch.tar.gz" "$tmp/syft.tar.gz"
  printf '%s  %s\n' "$syft_sha" "$tmp/syft.tar.gz" | sha256sum -c -
  tar -xzf "$tmp/syft.tar.gz" -C "$tmp" syft
  sudo install -m 0755 "$tmp/syft" /usr/local/bin/
else
  fetch "https://github.com/sigstore/cosign/releases/download/v2.6.5/cosign-linux-$arch" "$tmp/cosign"
  fetch "https://github.com/cli/cli/releases/download/v2.83.2/gh_2.83.2_linux_$arch.tar.gz" "$tmp/gh.tar.gz"
  printf '%s  %s\n' "$cosign_sha" "$tmp/cosign" "$gh_sha" "$tmp/gh.tar.gz" | sha256sum -c -
  tar -xzf "$tmp/gh.tar.gz" -C "$tmp" "gh_2.83.2_linux_$arch/bin/gh"
  sudo install -m 0755 "$tmp/cosign" "$tmp/gh_2.83.2_linux_$arch/bin/gh" /usr/local/bin/
fi
