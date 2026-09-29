#!/usr/bin/env bash
# Dependency provisioning ONLY. Tests themselves run offline.
set -euo pipefail
bash .github/scripts/setup-actionlint.sh
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  podman=4.9.3+ds1-1ubuntu0.2 uidmap=1:4.13+dfsg1-4ubuntu3.2 build-essential=12.10ubuntu1 \
  pkg-config=1.8.1-2build1 clang=1:18.0-59~exp2 libclang-dev=1:18.0-59~exp2 \
  libpcap0.8-dev=1.10.4-4.1ubuntu3.1 udev=255.4-1ubuntu8.17
python3 -m pip install pip==25.2 PyYAML==6.0.2
case "$(uname -m)" in
  aarch64) rust_arch=aarch64-unknown-linux-gnu; rust_sha=e3853c5a252fca15252d07cb23a1bdd9377a8c6f3efa01531109281ae47f841c ;;
  x86_64) rust_arch=x86_64-unknown-linux-gnu; rust_sha=20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c ;;
  *) exit 2 ;;
esac
rust_installer=$(mktemp)
trap 'rm -f "$rust_installer"' EXIT
curl --fail --show-error --location --max-time 120 \
  "https://static.rust-lang.org/rustup/archive/1.28.2/$rust_arch/rustup-init" -o "$rust_installer"
printf '%s  %s\n' "$rust_sha" "$rust_installer" | sha256sum -c -
chmod 0700 "$rust_installer"
"$rust_installer" -y --profile minimal --default-toolchain 1.88.0 --no-modify-path
export PATH="$HOME/.cargo/bin:$PATH"
printf '%s\n' "$HOME/.cargo/bin" >> "$GITHUB_PATH"
rustup toolchain install 1.88.0 --profile minimal --component clippy,rustfmt
cargo fetch --locked --manifest-path Component/aviary/Cargo.toml
podman pull docker.io/library/nats@sha256:e4bf19f15fd3218814a4e3c9e0064e1334bd8aa20d5984b9f1a0afd084f8cc00
podman pull docker.io/library/debian:13-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132
podman build --pull=never -f Component/brrdfeeder/install/tests/Containerfile \
  -t localhost/brrd-contract-os:20260929 Component/brrdfeeder/install/tests
