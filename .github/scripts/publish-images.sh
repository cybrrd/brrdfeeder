#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Only the environment: release job calls this. Never rebuild downloaded input.
set -euo pipefail
[[ ${RELEASE_APPROVAL_CONFIGURED:-} == true ]] || { echo 'Release approval setup not acknowledged' >&2; exit 2; }
: "${RUNNER_TEMP:?}" "${REGISTRY_TOKEN:?}" "${GITHUB_ACTOR:?}" "${GITHUB_OUTPUT:?}" "${GITHUB_STEP_SUMMARY:?}"
out="$RUNNER_TEMP/release"
# Assignment preserves a failing verifier exit code (not process substitution).
records=$(python3 .github/scripts/release-metadata.py verify "$out")
export DOCKER_CONFIG="$RUNNER_TEMP/registry-auth"
mkdir -m 0700 -p "$DOCKER_CONFIG"
# The job supplies this same DOCKER_CONFIG to provenance actions and removes
# config.json in its final always() step, including on signing/attestation failure.
printf '%s' "$REGISTRY_TOKEN" | skopeo login --authfile "$DOCKER_CONFIG/config.json" \
  --username "$GITHUB_ACTOR" --password-stdin ghcr.io
unset REGISTRY_TOKEN
while IFS=$'\t' read -r component image digest; do
  skopeo copy --preserve-digests --authfile "$DOCKER_CONFIG/config.json" \
    "oci-archive:$out/$component/image.oci.tar" "docker://$image:$GITHUB_REF_NAME"
  remote_digest=$(skopeo inspect --authfile "$DOCKER_CONFIG/config.json" --format '{{.Digest}}' "docker://$image:$GITHUB_REF_NAME")
  [[ $remote_digest == "$digest" ]] || { echo 'Published digest mismatch' >&2; exit 2; }
  cosign sign --yes "$image@$digest"
  printf '%s-digest=%s\n' "$component" "$digest" >> "$GITHUB_OUTPUT"
  printf '%s@%s\n' "$image" "$digest" > "$out/$component/image.txt"
  printf '### %s\n`%s@%s`\n' "$component" "$image" "$digest" >> "$GITHUB_STEP_SUMMARY"
done <<< "$records"
