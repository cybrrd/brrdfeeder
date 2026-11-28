#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
set -euo pipefail
: "${RUNNER_TEMP:?}" "${GITHUB_ENV:?}" "${GITHUB_PATH:?}"
case "$(uname -m)" in
  aarch64) rust_arch=aarch64-unknown-linux-gnu; rust_sha=e3853c5a252fca15252d07cb23a1bdd9377a8c6f3efa01531109281ae47f841c ;;
  x86_64) rust_arch=x86_64-unknown-linux-gnu; rust_sha=20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c ;;
  *) exit 2 ;;
esac
# Isolate hosted toolchains; leave pre-existing installations untouched.
rust_root=$(mktemp -d "$RUNNER_TEMP/hostutils-rust.XXXXXXXX")
export RUSTUP_HOME="$rust_root/rustup"
export CARGO_HOME="$rust_root/cargo"
printf 'RUSTUP_HOME=%s\nCARGO_HOME=%s\n' "$RUSTUP_HOME" "$CARGO_HOME" >> "$GITHUB_ENV"
printf 'CARGO_TARGET_DIR=%s\n' "$rust_root/target" >> "$GITHUB_ENV"
rust_installer_dir=$(mktemp -d "$RUNNER_TEMP/hostutils-installer.XXXXXXXX")
rust_installer="$rust_installer_dir/rustup-init"
trap 'rm -f "$rust_installer"; rmdir "$rust_installer_dir"' EXIT
curl --fail --show-error --location --max-time 120 \
  "https://static.rust-lang.org/rustup/archive/1.28.2/$rust_arch/rustup-init" -o "$rust_installer"
printf '%s  %s\n' "$rust_sha" "$rust_installer" | sha256sum -c -
chmod 0700 "$rust_installer"
"$rust_installer" -y --profile minimal --default-toolchain 1.100.0 --no-modify-path
export PATH="$CARGO_HOME/bin:$PATH"
printf '%s\n' "$CARGO_HOME/bin" >> "$GITHUB_PATH"
rustup toolchain install 1.100.0 --profile minimal --component clippy,rustfmt
