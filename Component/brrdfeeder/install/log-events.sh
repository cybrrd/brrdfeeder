# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# shellcheck shell=bash
# Embedded before flag parsing; private records are consumed by install-log.py.
# These are action-level commands. Read-only predicates/captured queries remain
# ordinary shell code; their visible output is still captured by the supervisor.
LOG_PHASE=pre-flight
log_event() { printf '\036%s\t%s\t%s\n' "$BRRDFEEDER_LOG_TOKEN" "$1" "$2"; }
log_secret() {
  local encoded
  encoded=$(printf '%s' "$2" | base64 -w0)
  log_event SECRET "$1"$'\t'"$encoded"
}
run_step() {
  local step=$1 display rc
  shift
  printf -v display '%q ' "$@"
  log_event BEGIN "$step"$'\t'"$display"
  if "$@"; then rc=0; else rc=$?; fi
  # The supervisor recognizes this nonce even after output without a final LF.
  log_event END "$rc"
  return "$rc"
}
export -f log_event log_secret run_step
export BRRDFEEDER_LOG_TOKEN LOG_PHASE
