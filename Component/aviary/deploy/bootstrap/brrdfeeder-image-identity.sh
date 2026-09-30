#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Host-side Quadlet lifecycle helper. No tag inspection, runtime socket mount,
# credentials, last-known fallback, or authority granted to the engine.
set -euo pipefail
export LC_ALL=C
mode=${1:-}
state_dir=${2:-}
cidfile=${3:-}
invocation=${INVOCATION_ID:-}
[[ $invocation =~ ^[a-f0-9]{32}$ ]] || { echo '[identity] invalid invocation; unknown' >&2; exit 1; }
[[ $state_dir == /* && -d $state_dir && ! -L $state_dir ]] || exit 1
[[ $(stat -c %u -- "$state_dir") == "$EUID" ]] || exit 1
permissions=$(stat -c %a -- "$state_dir")
(( (8#$permissions & 0022) == 0 )) || exit 1

write_record() {
    local state=$1 cid=${2:-} digest=${3:-} tmp
    tmp=$(mktemp "$state_dir/.identity.XXXXXXXX")
    chmod 0644 "$tmp"
    printf '{"schema_version":1,"invocation_id":"%s","state":"%s"' "$invocation" "$state" > "$tmp"
    if [[ $state == known ]]; then
        printf ',"container_id":"%s","image_digest":"%s"' "$cid" "$digest" >> "$tmp"
    fi
    printf '}\n' >> "$tmp"
    mv -fT -- "$tmp" "$state_dir/identity.json"
}

case "$mode" in
    prepare) write_record pending ;;
    resolve)
        # Invalidate FIRST, but do not finish the engine's wait before inspect.
        # On any failure publish a terminal unknown; no known record survives.
        write_record pending
        trap 'rc=$?; if (( rc != 0 )); then write_record unverified || true; fi' EXIT
        [[ -f $cidfile && ! -L $cidfile ]] || exit 1
        cid=$(<"$cidfile")
        [[ $cid =~ ^[a-f0-9]{64}$ ]] || exit 1
        # Literal format, validated CID, timeout; never inspect a moving tag.
        inspected=$(timeout 5 podman container inspect --format '{{.Id}} {{.ImageDigest}} {{.State.Running}}' "$cid") || {
            echo '[identity] WARNING reason=running_identity_unverified; container inspection failed' >&2
            exit 1
        }
        read -r actual digest running extra <<< "$inspected"
        [[ $actual == "$cid" && $digest =~ ^sha256:[a-f0-9]{64}$ && $running == true && -z $extra ]] || {
            echo '[identity] WARNING reason=running_identity_unverified; no running-container manifest digest' >&2
            exit 1
        }
        write_record known "$actual" "$digest"
        printf '[identity] container_id=%s image_digest=%s source=podman_container_inspect\n' "$actual" "$digest"
        ;;
    *) echo 'usage: brrdfeeder-image-identity prepare|resolve RUNTIME_DIR [CIDFILE]' >&2; exit 2 ;;
esac
