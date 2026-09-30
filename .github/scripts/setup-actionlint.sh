#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Official actionlint 1.7.12 checksums; provisioning, never part of offline tests.
set -euo pipefail
: "${RUNNER_TEMP:?}" "${GITHUB_PATH:?}"
case "$(uname -m)" in
  aarch64|arm64) arch=arm64; sha=325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6 ;;
  x86_64|amd64) arch=amd64; sha=8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8 ;;
  *) exit 2;;
esac
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl --fail --show-error --location --max-time 120 \
  "https://github.com/rhysd/actionlint/releases/download/v1.7.12/actionlint_1.7.12_linux_$arch.tar.gz" -o "$tmp/tool.tar.gz"
printf '%s  %s\n' "$sha" "$tmp/tool.tar.gz" | sha256sum -c -
mkdir -p "$RUNNER_TEMP/actionlint"
tar -xzf "$tmp/tool.tar.gz" -C "$RUNNER_TEMP/actionlint" actionlint
printf '%s\n' "$RUNNER_TEMP/actionlint" >> "$GITHUB_PATH"
