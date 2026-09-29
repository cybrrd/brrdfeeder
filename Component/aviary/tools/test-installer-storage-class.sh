#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
set -euo pipefail

if [[ ${D8_INSTALLER_TEST_CONTAINER:-0} != 1 || $EUID -ne 0 ]]; then
  printf 'STOP run only in the documented throwaway root container\n' >&2
  exit 2
fi

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
bootstrap="${script_dir}/../deploy/bootstrap"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

getent group operator >/dev/null || groupadd operator
useradd -m -g operator operator
export BRRDFEEDER_LEGACY_USER=operator
usermod -a -G dialout operator
install -D -m 0755 /bin/true /home/operator/brrdfeeder-src/engine/target/release/engine
sed 's/storage_class: "ephemeral"/storage_class: "ephemreal"/' \
  "$bootstrap/config.yaml.mobile.template" > /home/operator/config.yaml

set +e
output=$(cd "$bootstrap" && bash brrdfeeder-install.sh --dry-run 2>&1)
rc=$?
set -e
[[ $rc -ne 0 ]]
grep -Fq "node.storage_class must be ephemeral|persistent, got 'ephemreal'" <<<"$output"
if grep -q 'Step 1 — udev rules' <<<"$output"; then
  printf 'FAIL invalid storage class reached a mutating installer step\n' >&2
  exit 1
fi

cp "$bootstrap/config.yaml.mobile.template" /home/operator/config.yaml
valid_output=$(cd "$bootstrap" && bash brrdfeeder-install.sh --dry-run 2>&1)
grep -Fq 'install -o root -g root -m 0600 /dev/null /var/lib/brrdfeeder/blackbox/.flush.lock' <<<"$valid_output"

printf 'PASS installer storage_class allowlist: typo=ephemreal named=yes abort_before_mutation=yes valid_lock=0600-in-dir\n'
