#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
# Embedded in the verified installer; executed only for bootstrap install mode.
# Root-only preparation belongs after checksum verification and inside logging.
bootstrap_say() { log_event NOTICE "$*"; }
bootstrap_die() { bootstrap_say "FATAL: $*"; exit 1; }
bootstrap_note() { bootstrap_say "$*"; export BRRDFEEDER_BOOTSTRAP_NOTES="${BRRDFEEDER_BOOTSTRAP_NOTES:-}; $*"; }
[[ $EUID == 0 ]] || bootstrap_die "Root preparation requires sudo; use curl -fsSL https://get.cybrrd.com | bash"
arch="$(uname -m)"
case "$arch" in
  aarch64|arm64) ;;
  *) bootstrap_die "this release is built for arm64 (Raspberry Pi 4/5). This machine is $arch.
       Installing the wrong architecture is a slower failure than refusing now." ;;
esac

for tool in curl sha256sum ip; do
  command -v "$tool" >/dev/null || bootstrap_die "$tool is required but not installed."
done

bootstrap_say "architecture : $arch"
bootstrap_say "engine/console pins: supplied by checksum-verified bootstrap"

# ── Console listen address ──────────────────────────────────────────────────
# The installer REQUIRES --console-listen as a literal IP:port and refuses a
# hostname, a wildcard or a shell expression. That is deliberate and correct:
# binding the status console to 0.0.0.0 would expose it beyond the owner's LAN,
# and this page is unauthenticated by design because it serves a home or
# small-business network.
#
# But a customer should not have to find their own LAN address to install
# software. So derive it — and derive it CONSERVATIVELY, refusing anything that
# would put the console somewhere surprising. An auto-detected listen address is
# only defensible if it fails rather than guesses.
#
# `ip route get 1.1.1.1` yields the source address of the interface carrying the
# default route. That is structurally the right answer: it cannot select the
# Wi-Fi capture adapter (a monitor-mode interface has no IP at all), and on our
# own sensors it correctly returns the LAN address rather than the tailnet one.
CONSOLE_PORT=8080

listen_supplied=0
for arg in "$@"; do
  case "$arg" in --console-listen|--console-listen=*) listen_supplied=1 ;; esac
done

if [ "$listen_supplied" -eq 1 ]; then
  bootstrap_note "console addr : supplied by operator"
else
  route_line="$(ip -4 route get 1.1.1.1 2>/dev/null | head -1)"
  # Field-walk rather than a regex. `sed 's/.*dev \(...\)/'` is greedy and matches
  # inside an interface name containing "dev" — caught in testing with a fixture
  # device named `testdev`, which parsed as "src". Real names are fine, but a latent
  # trap in an installer is still a trap.
  lan_ip="$(printf '%s\n' "$route_line" | awk '{for(i=1;i<NF;i++) if($i=="src"){print $(i+1); exit}}')"
  lan_dev="$(printf '%s\n' "$route_line" | awk '{for(i=1;i<NF;i++) if($i=="dev"){print $(i+1); exit}}')"

  [ -n "$lan_ip" ] || bootstrap_die "could not determine this machine's LAN address.
       The console needs a literal address to listen on. Re-run supplying it:
         curl -fsSL https://get.cybrrd.com | bash -s -- --console-listen <your-LAN-IP>:$CONSOLE_PORT"

  # Refuse addresses that would put the console somewhere the owner does not expect.
  o1="${lan_ip%%.*}"; rest="${lan_ip#*.}"; o2="${rest%%.*}"
  reason=""
  case "$lan_ip" in
    127.*)     reason="loopback — the console would be unreachable from any other device on your network" ;;
    169.254.*) reason="link-local — this machine did not get a DHCP lease; fix networking first" ;;
    0.0.0.0)   reason="wildcard — refusing to expose the console on every interface" ;;
  esac
  # 100.64.0.0/10 — CGNAT, which is also the Tailscale range. A console bound
  # there is reachable over the overlay and NOT from the owner's own LAN: the
  # opposite of what this page is for, and a surprise nobody would debug quickly.
  if [ -z "$reason" ] && [ "$o1" = "100" ] && [ "$o2" -ge 64 ] 2>/dev/null && [ "$o2" -le 127 ] 2>/dev/null; then
    reason="a CGNAT/Tailscale address — the console would be reachable over the overlay but NOT from your own LAN"
  fi
  [ -z "$reason" ] || bootstrap_die "declining to auto-select $lan_ip: $reason.
       Supply the address you want explicitly:
         curl -fsSL https://get.cybrrd.com | bash -s -- --console-listen <your-LAN-IP>:$CONSOLE_PORT"

  bootstrap_note "console addr : $lan_ip:$CONSOLE_PORT  (auto-detected on ${lan_dev:-?})"
  set -- --console-listen "$lan_ip:$CONSOLE_PORT" "$@"
fi

# ── Fresh install, abandoned template, or rerun? ────────────────────────────
# The installer has three states and treats first-install flags differently in
# each. Getting this wrong fatals on the installer's own line 340 ("Existing
# config is preserved; omit first-install --interface/--latitude/--longitude").
#
#   no config.yaml            fresh install  -> derive interface/listener; GPS seeds position
#   config.yaml with EDIT-ME- abandoned run  -> it is the installer's OWN unedited
#                                               template (the installer itself gates
#                                               on that token); remove it so the
#                                               installer regenerates it WITH the
#                                               flags, then proceed as fresh
#   config.yaml, no EDIT-ME-  real config    -> a rerun; pass NO first-install flags
#
# The second state is exactly what a customer hits after a first attempt stops
# early — as brrdg3s2 did on 2026-09-22 — and without this they would be told to
# hand-edit a YAML file, which is the thing the one-liner exists to prevent.
install_mode=fresh
if [ -e "$CONFIG_PATH" ]; then
  if grep -qE 'EDIT-ME-[A-Za-z]' "$CONFIG_PATH" 2>/dev/null; then
    bootstrap_note "config       : abandoned template from an earlier run — removing so this run completes it"
    [[ ! -L $CONFIG_PATH && ! -L /etc/brrdfeeder ]] || bootstrap_die "Refusing symlinked config template"
    run_step bootstrap/recover-template rm -f "$CONFIG_PATH"
    install_mode=fresh
  else
    bootstrap_note "config       : existing (rerun) — first-install parameters will not be passed"
    install_mode=rerun
  fi
fi

# ── Capture interface ───────────────────────────────────────────────────────
# The installer needs --interface on a fresh install. A customer cannot reasonably
# be expected to know their adapter is "wlan1". But it IS deterministic: Remote ID
# capture requires monitor mode, and on a Raspberry Pi exactly one adapter can do
# it — the external one. The Pi's built-in Wi-Fi (brcmfmac) cannot. So: find the
# phys that support monitor mode. Exactly one is the answer; zero means the
# external adapter is not plugged in, which is the single most common way a new
# install will fail and deserves a plain message rather than a cryptic one later.
iface_supplied=0; lat_supplied=0; lon_supplied=0
if [ "$install_mode" = "rerun" ]; then
  # A rerun must not carry first-install flags (installer line 340). If the
  # operator passed any, that is their explicit choice and the installer will
  # say so; the bootstrap adds none of its own.
  iface_supplied=1
fi
for arg in "$@"; do
  case "$arg" in
    --interface|--interface=*) iface_supplied=1 ;;
    --latitude|--latitude=*)   lat_supplied=1 ;;
    --longitude|--longitude=*) lon_supplied=1 ;;
  esac
done

if [ "$install_mode" = "rerun" ]; then
  bootstrap_note "capture iface: existing configuration preserved"
elif [ "$iface_supplied" -eq 1 ]; then
  bootstrap_note "capture iface: supplied by operator"
else
  if ! command -v iw >/dev/null; then
    bootstrap_say "installing iw (needed to identify the capture adapter)…"
    run_step bootstrap/install-iw apt-get install -y --no-install-recommends iw \
      || bootstrap_die "could not install iw. Supply the capture interface explicitly:
         curl -fsSL https://get.cybrrd.com | bash -s -- --interface <iface> ..."
  fi
  mon_ifaces=""
  for phy in /sys/class/ieee80211/*; do
    [ -e "$phy" ] || continue
    pname="$(basename "$phy")"
    if iw phy "$pname" info 2>/dev/null | awk '/Supported interface modes/,/Band /' | grep -q '\* monitor'; then
      dev="$(ls "$phy/device/net" 2>/dev/null | head -1)"
      [ -n "$dev" ] && mon_ifaces="$mon_ifaces $dev"
    fi
  done
  mon_ifaces="${mon_ifaces# }"
  case "$(printf '%s' "$mon_ifaces" | wc -w)" in
    0) bootstrap_die "no Wi-Fi adapter that supports monitor mode was found.
       Remote ID capture needs one, and the Raspberry Pi's built-in Wi-Fi cannot
       do it. Is the external adapter (ALFA AWUS036AXM/AXML) plugged in?
       If it is and this still fails, supply it explicitly:
         curl -fsSL https://get.cybrrd.com | bash -s -- --interface <iface> ..." ;;
    1) bootstrap_note "capture iface: $mon_ifaces  (the only monitor-capable adapter)"
       set -- --interface "$mon_ifaces" "$@" ;;
    *) bootstrap_die "more than one monitor-capable adapter found ($mon_ifaces).
       Not guessing which one is the capture radio. Supply it explicitly:
         curl -fsSL https://get.cybrrd.com | bash -s -- --interface <iface> ..." ;;
  esac
fi

# ── Sensor position ─────────────────────────────────────────────────────────
# The service waits for GPS, never the installer or the owner. A paired explicit
# expert override is still accepted; existing configuration is never reseeded.
if [ "$lat_supplied" -ne "$lon_supplied" ]; then
  bootstrap_die "supply --latitude and --longitude together, or neither."
elif [ "$install_mode" = "rerun" ]; then
  bootstrap_note "position     : existing configuration preserved; service seeds only if absent"
elif [ "$lat_supplied" -eq 1 ]; then
  bootstrap_note "position     : optional expert override supplied"
else
  bootstrap_note "position     : GPS service will wait for a measured fix; install will not wait or prompt"
fi
bootstrap_say ""
