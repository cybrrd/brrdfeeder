# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# GENERATED Self-Update general candidate; the release approver publishes after review.
#!/usr/bin/env bash
# bootstrap.sh — the single command.
#
#   curl -fsSL https://get.cybrrd.com | bash
#
# Served as https://get.cybrrd.com/install.sh from the `world` Caddy edge.
#
# WHAT THIS IS: a thin, auditable wrapper. It does NOT install anything itself. It
# fetches the real installer, VERIFIES IT BY CHECKSUM, and runs it with the approved
# release digests already filled in — so the operator never pastes a hash, the way
# rustup and the Claude Code CLI never make you paste one.
#
# WHY A CHECKSUM: `curl | bash` places the whole of your trust in one TLS connection.
# This script pins the SHA-256 of the installer it expects, so a compromised or
# truncated download fails loudly instead of executing. That is the one property that
# makes this pattern defensible, and it is why the installer is fetched to a file and
# hashed BEFORE it is run, never piped straight into a shell.
#
# WHY DIGESTS, NOT TAGS: the installer refuses image tags by design — "Fresh install
# requires --image ...@sha256:<approved-release-digest>; tags are not accepted." A tag
# can be repointed after you have audited it; a digest cannot. This file carries the
# approved digests, so cutting a release is a one-line change here.
#
# FAILS CLOSED: if a digest below is still a placeholder, this script refuses to run
# rather than installing something unpinned. An installer that degrades to "best
# effort" is how a customer ends up on an image nobody approved.
set -euo pipefail

# ── Approved release ────────────────────────────────────────────────────────
# Engine: published and verified anonymously 2026-09-22 — the registry resolves this
# digest without credentials, arch arm64/linux, label revision=02c1d811.
ENGINE_IMAGE="ghcr.io/cybrrd/brrdfeeder@sha256:3360f8d6ae29a4daddbe3a40697a410f9c7fed62fff186199eaa513f1dc52e0f"

# Console (BRRDhouse) — the management/status container. The release approver's ruling 2026-09-22: it is
# INTRINSIC to the BRRDfeeder package, not a separate install. GPS waiting-state
# image built offline 2026-09-23, arm64/linux. The release approver MUST publish and verify anonymous
# digest access before deploying this bootstrap; a local build is not publication.
CONSOLE_IMAGE="ghcr.io/cybrrd/brrdhouse@sha256:fffd150942cde14c3474536562c844badb3cfcc8e7f275b160399d1f8820708b"

# The installer this bootstrap fetches, and the hash it must have.
INSTALLER_URL="${BRRDFEEDER_INSTALLER_URL:-https://get.cybrrd.com/dev/brrdfeeder-install.sh}"
INSTALLER_SHA256="f53966d298c0fe4cd0668139f9f8aed4c8d11e62ab3e6b39926698fed0f35c14"

# ── RELEASE CHECKLIST — do these IN THIS ORDER when cutting a release ───────
#  1. Publish both images to ghcr and verify each resolves BY DIGEST anonymously,
#     from outside any credential. That is the position a customer is in; a package
#     that pulls for you and 403s for them fails halfway through an install.
#  2. Re-derive INSTALLER_SHA256 from the installer AS MERGED TO main, not from a
#     feature branch. The value below binds this one-liner candidate and must
#     be checked again after merge. If the merge changes one byte, this hash is wrong and
#     the bootstrap will refuse — loudly, which is correct, but check it rather
#     than discover it.
#  3. Copy this file to world:/etc/world/get/install.sh AND the installer to
#     world:/etc/world/get/brrdfeeder-install.sh. Both, or the fetch 404s.
#  4. Verify from a machine with no credentials:
#       curl -fsSL https://get.cybrrd.com | head -20
#     Plain curl must return this script. The browser request must return the page:
#       curl -fsSL -H 'Accept: text/html' https://get.cybrrd.com | head
#  5. The console listen address is AUTO-DERIVED (see below) because the installer
#     requires it and a customer should not have to look up their own LAN IP. If
#     the installer ever gains further REQUIRED fresh-install flags, this bootstrap
#     must supply or derive them too — otherwise the advertised one-liner breaks
#     silently for new users while continuing to work for anyone re-running.
BANNER="BRRDfeeder — installer"
BOOT_MODE=install
case "${1:-}" in
  uninstall|--uninstall) BOOT_MODE=uninstall ;;
  status) BOOT_MODE=status ;;
  support-bundle|--support-bundle) BOOT_MODE=support-bundle ;;
esac

# ── Run ID and the bootstrap's own log (INSTALL-LOG-SPEC §B, §G) ────────────
# One 8-hex run_id is generated HERE and handed to the installer, so the
# bootstrap log and the install log share it. It is the first and last thing
# printed to the terminal: support asks "what run ID is on your screen?" and
# matches it to the file. The bootstrap keeps its own log because its refusals
# (wrong arch, no adapter, checksum mismatch) happen BEFORE the installer exists,
# and a refused user still needs something to send.
RUN_ID="$(head -c4 /dev/urandom 2>/dev/null | od -An -tx1 | tr -d ' \n')"
[ -n "$RUN_ID" ] || RUN_ID="$(date +%s | tail -c 9)"
BLOG=""                 # opened as the invoking user, before elevation
NOTES=""                # decisions, handed to the installer verbatim

log() { [ -n "$BLOG" ] && printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >> "$BLOG" 2>/dev/null || true; }
die() {
  printf '\n[FATAL] %s\n' "$*" >&2
  log "[FAIL] $*"
  printf '\nBRRDfeeder %s failed at bootstrap.\n' "$BOOT_MODE" >&2
  printf 'Support: curl -fsSL https://get.cybrrd.com | bash -s support-bundle\n' >&2
  printf 'Log: %s  (run %s)\n' "${BLOG:-unavailable; retain this terminal output}" "$RUN_ID" >&2
  exit 1
}
say()  { printf '  %s\n' "$*"; log "[INFO] $*"; }
note() { say "$*"; NOTES="${NOTES}${NOTES:+; }$*"; }   # a decision: printed, logged, AND handed down

printf '\n%s   (run %s)\n\n' "$BANNER" "$RUN_ID"

# ── Refuse to proceed on anything unpinned ──────────────────────────────────
if [[ $BOOT_MODE == install ]]; then
case "$ENGINE_IMAGE" in
  *"@sha256:"*) ;;
  *) die "engine image is not digest-pinned. This build of the bootstrap is not releasable." ;;
esac
case "$CONSOLE_IMAGE" in
  __CONSOLE_IMAGE_UNSET__)
    die "the console image digest has not been set in this bootstrap.
       The console is part of the standard package, not a separate install, so
       refusing to continue rather than installing half of it.
       If you are cutting a release: publish the console image and replace
       CONSOLE_IMAGE with its ghcr digest." ;;
  *"@sha256:"*) ;;
  *) die "console image is not digest-pinned. Refusing." ;;
esac
fi
[ "$INSTALLER_SHA256" != "__INSTALLER_SHA256_UNSET__" ] \
  || die "no expected checksum for the installer. Refusing to fetch and run
       unverified code. Set INSTALLER_SHA256 when cutting the release."

# ── Preconditions, checked before anything is downloaded ────────────────────

# As the invoking user, open the log. Never fail the install because a log could not be
# opened; never silently run without one either — say where it went.
if [[ $EUID == 0 && ! -L /var/log/brrdfeeder ]] && mkdir -p /var/log/brrdfeeder 2>/dev/null && [ -w /var/log/brrdfeeder ]; then
  BLOG="/var/log/brrdfeeder/bootstrap-$(date -u +%Y%m%dT%H%M%SZ)-$RUN_ID.log"
else
  BLOG="$(mktemp "/tmp/brrdfeeder-bootstrap-$RUN_ID.XXXXXXXX.log")"
fi
: > "$BLOG" && chmod 0640 "$BLOG"
[[ $BLOG != /tmp/* ]] || log "Bootstrap log uses a temporary file before elevation: $BLOG"
{
  printf '==== BRRDFEEDER BOOTSTRAP LOG ====\n'
  printf 'run_id=%s\nstarted=%s\nbootstrap_sha256=%s\nengine_image=%s\nconsole_image=%s\ninstaller_url=%s\ninstaller_sha256_expected=%s\ninvoked_by_uid=%s sudo_user=%s\nargv=%s\n\n' \
    "$RUN_ID" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$(sha256sum "$0" 2>/dev/null | cut -d' ' -f1 || echo unknown)" \
    "$ENGINE_IMAGE" "$CONSOLE_IMAGE" "$INSTALLER_URL" "$INSTALLER_SHA256" "$EUID" "${SUDO_USER:-}" "$*"
} >> "$BLOG"

# ── Fetch, verify, THEN run ─────────────────────────────────────────────────
for tool in curl sha256sum; do
  command -v "$tool" >/dev/null || die "$tool is required to fetch the verified recovery installer."
done
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT
installer="$workdir/brrdfeeder-install.sh"

say "fetching installer from $INSTALLER_URL"
curl -fsSL --proto '=https' --tlsv1.2 -o "$installer" "$INSTALLER_URL" \
  || die "could not download the installer. Check network and DNS, then retry."

got="$(sha256sum "$installer" | cut -d' ' -f1)"
if [ "$got" != "$INSTALLER_SHA256" ]; then
  die "INSTALLER CHECKSUM MISMATCH — refusing to execute.
       expected $INSTALLER_SHA256
       got      $got
       Do not work around this. It means the file you received is not the file
       this release approved."
fi
note "checksum     : verified"
say ""

log "[HANDOFF] verified installer with run_id=$RUN_ID"
say "bootstrap log: $BLOG  (run $RUN_ID)"
export BRRDFEEDER_RUN_ID="$RUN_ID" BRRDFEEDER_BOOTSTRAP_NOTES="$NOTES" BRRDFEEDER_BOOTSTRAP_LOG="$BLOG"
export BRRDFEEDER_BOOTSTRAP_PREPARE=1
if [[ $BOOT_MODE == install ]]; then
  set -- --image "$ENGINE_IMAGE" --console-image "$CONSOLE_IMAGE" --ring general "$@"
fi
# Elevate only AFTER verification, with stdin connected to the real terminal.
# With sudo use_pty, forwarding the curl pipe strands all later keyboard reads.
# Root never executes a caller-writable pathname. The inline code, expected
# hash and argv cross sudo as arguments, not via a mutable control file.
root_handoff='
set -euo pipefail
umask 077
source_path=$1
expected=$2
shift 2
[[ $EUID == 0 && $expected =~ ^[0-9a-f]{64}$ ]] || exit 1
private_dir=$(/usr/bin/mktemp -d /run/brrdfeeder-bootstrap.XXXXXXXX)
trap '\''/bin/rm -rf -- "$private_dir"'\'' EXIT
/usr/bin/install -m 0600 -- "$source_path" "$private_dir/installer.sh"
actual=$(/usr/bin/sha256sum "$private_dir/installer.sh")
if [[ ${actual%% *} != "$expected" ]]; then
  printf "%s\n" "ROOT INSTALLER CHECKSUM MISMATCH — refusing to execute." >&2
  printf "Log: %s (run %s)\n" "$BRRDFEEDER_BOOTSTRAP_LOG" "$BRRDFEEDER_RUN_ID" >&2
  exit 1
fi
/bin/bash "$private_dir/installer.sh" "$@"
'
rc=0
if (( EUID == 0 )); then
  /bin/bash -c "$root_handoff" brrdfeeder-root-handoff "$installer" "$INSTALLER_SHA256" "$@" || rc=$?
else
  command -v sudo >/dev/null || die "sudo is not installed. Ask the administrator to install sudo and grant this account access, then retry: curl -fsSL https://get.cybrrd.com | bash"
  sudo_args=()
  sudo_ready=0
  # Debian verifypw=all may require a password for -v even when commands
  # are permitted by NOPASSWD:ALL. Probe execution before validation.
  if sudo -n true </dev/null 2>/dev/null; then
    sudo_ready=1
    sudo_args=(-n)
  fi
  if { exec {handoff_tty}</dev/tty; } 2>/dev/null && [[ -t $handoff_tty ]]; then
    # Debian sudo use_pty distinguishes the /dev/tty alias (device 5:0) from
    # the concrete controlling terminal. In the real-PTY proof the alias alone
    # still loses input. Resolve the kernel-reported terminal, never stdin's pipe.
    command -v ps >/dev/null || die "ps is required to attach sudo to your terminal. Ask the administrator to install procps, then re-run the one-liner."
    terminal_name="$(LC_ALL=C ps -o tty= -p "$$")" || die "cannot identify the controlling terminal; retry from a normal terminal session."
    terminal_name="${terminal_name//[[:space:]]/}"
    [[ $terminal_name =~ ^(pts/[0-9]+|tty[A-Za-z0-9]+)$ ]] && [[ -c /dev/$terminal_name ]] \
      || die "cannot safely identify the controlling terminal; use a normal terminal session, or --yes with noninteractive sudo for automation."
    exec {handoff_tty}<&-
    { exec {handoff_tty}<"/dev/$terminal_name"; } 2>/dev/null \
      || die "cannot open your controlling terminal for sudo; retry from a normal terminal session."
    [[ -t $handoff_tty ]] || die "sudo input is not a terminal; refusing to strand a prompt."
    if [[ $sudo_ready -eq 0 ]]; then
      sudo -v <&"$handoff_tty" || die "sudo authorization failed. Ask the administrator to grant this account sudo access, then retry: curl -fsSL https://get.cybrrd.com | bash"
    fi
  else
    if [[ -n ${handoff_tty:-} ]]; then exec {handoff_tty}<&-; unset handoff_tty; fi
    # Never attempt a password prompt through the curl pipe.
    sudo_args=(-n)
    [[ $sudo_ready -eq 1 ]] || die "Sudo could not run a noninteractive command, and no terminal is available for a password prompt. Re-run from a terminal, or ask the administrator to review sudo access; automated uninstall also requires --yes."
  fi
  # Explicit handoff only: no sudo -E, no broad caller environment preservation.
  handoff_args=(env
    "BRRDFEEDER_RUN_ID=$RUN_ID" "BRRDFEEDER_BOOTSTRAP_NOTES=$NOTES"
    "BRRDFEEDER_BOOTSTRAP_LOG=$BLOG" "BRRDFEEDER_BOOTSTRAP_PREPARE=1"
    /bin/bash -c "$root_handoff" brrdfeeder-root-handoff "$installer" "$INSTALLER_SHA256" "$@")
  sudo -n -l -- "${handoff_args[@]}" >/dev/null 2>&1 \
    || die "sudo authorization failed for the verified installer. Ask the administrator to grant this account sudo access, then retry: curl -fsSL https://get.cybrrd.com | bash"
  command_args=(sudo "${sudo_args[@]}" -- "${handoff_args[@]}")
  if [[ -n ${handoff_tty:-} ]]; then
    "${command_args[@]}" <&"$handoff_tty" || rc=$?
    exec {handoff_tty}<&-
  else
    "${command_args[@]}" </dev/null || rc=$?
  fi
fi
# Do not exec: the private verified download is cleaned by the EXIT trap.
exit "$rc"
