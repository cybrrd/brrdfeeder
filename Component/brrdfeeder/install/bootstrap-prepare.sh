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
# Fresh automatic installs persist USB selection intent, not an observed name.
# Explicit --interface and every existing config retain exact-name behavior.
# Zero or multiple capable USB interfaces is a refusal, never a first match.
capture_auto_selected=0
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
  if ! mon_iface=$(python3 - <<'CAPTURE_USB_EOF'
#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Read-only fresh-install USB Wi-Fi selection. Existing config bypasses this."""
from pathlib import Path
import re
import subprocess
import sys


def event(path):
    return dict(line.split('=', 1) for line in path.read_text().splitlines() if '=' in line)


def monitor(phy):
    result = subprocess.run(['iw', 'phy', phy, 'info'], check=True, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
    modes = result.stdout.split('Supported interface modes:', 1)
    if len(modes) != 2:
        raise ValueError('wireless driver did not report supported interface modes')
    return bool(re.search(r'^\s*\* monitor\s*$', modes[1].split('Band ', 1)[0], re.M))


def choose(root=Path('/sys'), capability=monitor):
    devices = (root/'devices').resolve(strict=True)
    candidates, reasons = [], []
    for net in sorted((root/'class/net').iterdir()):
        if not (net/'phy80211').exists():
            continue
        if not re.fullmatch(r'[A-Za-z0-9_][A-Za-z0-9_.:-]{0,14}', net.name):
            raise ValueError('invalid Wi-Fi interface name')
        device = (net/'device').resolve(strict=True)
        if not device.is_relative_to(devices):
            raise ValueError('Wi-Fi device escapes sysfs devices')
        usb = next((p for p in (device, *device.parents)
                    if p.is_relative_to(devices) and ((p/'idVendor').exists() or (p/'idProduct').exists())), None)
        if usb is None:
            reasons.append(net.name+': not a USB adapter')
            continue
        vid, pid = (usb/'idVendor').read_text().strip(), (usb/'idProduct').read_text().strip()
        if not re.fullmatch(r'[0-9a-fA-F]{4}:[0-9a-fA-F]{4}', vid+':'+pid):
            raise ValueError('invalid USB identity')
        ue = event(usb/'uevent')
        product = ue.get('PRODUCT', '').split('/')
        if (ue.get('DEVTYPE') != 'usb_device' or len(product) != 3
                or [int(x, 16) for x in product[:2]] != [int(vid, 16), int(pid, 16)]):
            raise ValueError('USB identity and kernel uevent disagree')
        driver = (device/'driver').resolve(strict=True)
        if driver.parent != (root/'bus/usb/drivers').resolve(strict=True):
            raise ValueError('Wi-Fi USB driver is not bound')
        ue = event(device/'uevent')
        if ue.get('DEVTYPE') != 'usb_interface' or ue.get('DRIVER') != driver.name:
            raise ValueError('USB driver and kernel uevent disagree')
        phy = (net/'phy80211').resolve(strict=True).name
        if not re.fullmatch(r'phy[0-9]+', phy):
            raise ValueError('invalid wireless phy name')
        if capability(phy):
            candidates.append(net.name)
        else:
            reasons.append(net.name+': USB '+vid+':'+pid+' driver '+driver.name+' has no monitor-mode support')
    if len(candidates) > 1:
        raise ValueError('more than one monitor-capable USB Wi-Fi interface found ('+', '.join(candidates)+'); refusing to guess. Supply --interface explicitly')
    if not candidates:
        raise ValueError('no USB Wi-Fi adapter with a bound driver and monitor-mode support found. Plug in the external capture adapter (ALFA AWUS036AXM/AXML). '+ '; '.join(reasons))
    return candidates[0]


if __name__ == '__main__':
    try:
        print(choose())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit('USB Wi-Fi capture selection refused: '+str(error))
CAPTURE_USB_EOF
  ); then
    bootstrap_die "Could not select a dedicated USB capture adapter; see the reason above."
  fi
  capture_auto_selected=1
  bootstrap_note "capture iface: $mon_iface (USB automatic selection; renamed replacements recover automatically)"
  set -- --interface "$mon_iface" "$@"

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
