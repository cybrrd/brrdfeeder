#!/usr/bin/env bash
# board-discovery.sh: READ-ONLY survey of an ARM64 board for BRRDfeeder suitability.
# WHAT: records the board, OS, kernel, Podman, and whether the ALFA Wi-Fi, the Realtek RID.BLE dongle and the u-blox GPS are
#       usable (driver bound, firmware present, monitor mode supported). Changes nothing: no installs, no interface up/down,
#       no HCI commands (never probe a Bluetooth controller another program may own).
# WHY:  a board is "supported" only after evidence. This is step 1 of that evidence (governance/checklists/2026-09-24-board-discovery.md).
# USAGE: bash board-discovery.sh          (sudo is optional; with it, the kernel log section is complete)
# OUTPUT: ~/brrdfeeder-board-discovery-<host>-<utc>.txt, also printed.
set -u
OUT="$HOME/brrdfeeder-board-discovery-$(hostname)-$(date -u +%Y%m%dT%H%M%SZ).txt"
exec > >(tee "$OUT") 2>&1
sec() { printf '\n==== %s ====\n' "$1"; }
run() { printf '$ %s\n' "$*"; "$@" 2>&1 || printf '(exit %s)\n' "$?"; }
have() { command -v "$1" >/dev/null 2>&1; }
verdict() { printf 'VERDICT %-22s %s\n' "$1" "$2"; }
V=()

sec "board"
model=$(tr -d '\0' </proc/device-tree/model 2>/dev/null || echo unknown); echo "model: $model"
echo "compatible: $(tr '\0' ' ' </proc/device-tree/compatible 2>/dev/null || echo unknown)"
echo "arch: $(uname -m)  kernel: $(uname -r)"
grep -E '^(PRETTY_NAME|ID|VERSION_ID|VERSION_CODENAME)=' /etc/os-release
have lscpu && lscpu | grep -E 'Model name|^CPU\(s\)|Vendor ID'
grep -E 'MemTotal' /proc/meminfo
[ "$(uname -m)" = aarch64 ] && V+=("arch|PASS aarch64") || V+=("arch|FAIL $(uname -m) (images are arm64)")

sec "storage"
run findmnt -no SOURCE,FSTYPE,OPTIONS /
run lsblk -d -o NAME,TRAN,ROTA,SIZE,MODEL

sec "container runtime"
if have podman; then
  pv=$(podman --version | awk '{print $3}'); echo "podman $pv"
  [ "${pv%%.*}" -ge 5 ] 2>/dev/null && V+=("podman|PASS $pv") || V+=("podman|FAIL $pv (need >= 5 for Quadlet as installed)")
else echo "podman: not installed"; V+=("podman|INFO not installed (installer installs it; the distro version must be >= 5)"); fi
have apt-cache && apt-cache policy podman 2>/dev/null | sed -n 1,3p
echo "systemd: $(systemctl --version | head -1)"
echo "cgroup fs: $(stat -fc %T /sys/fs/cgroup)"   # cgroup2fs expected
[ "$(stat -fc %T /sys/fs/cgroup)" = cgroup2fs ] && V+=("cgroup|PASS v2") || V+=("cgroup|FAIL not cgroup v2")

sec "usb devices"
have lsusb && { run lsusb; run lsusb -t; } || echo "lsusb missing (package usbutils); sysfs list:"
for d in /sys/bus/usb/devices/*; do [ -r "$d/idVendor" ] && printf '%s %s:%s %s\n' "$(basename "$d")" "$(cat "$d/idVendor")" "$(cat "$d/idProduct")" "$(cat "$d/product" 2>/dev/null)"; done

usbid() { local p; p=$(readlink -f "$1"); while [ "$p" != / ]; do [ -r "$p/idVendor" ] && { echo "$(cat "$p/idVendor"):$(cat "$p/idProduct")"; return; }; p=$(dirname "$p"); done; echo "not-usb"; }

sec "wi-fi phys (capture adapter = ALFA, MediaTek)"
alfa=0; monphys=0
for phy in /sys/class/ieee80211/*; do
  [ -e "$phy" ] || { echo "no wireless phys"; break; }
  p=$(basename "$phy"); dev=$(ls "$phy/device/net" 2>/dev/null | head -1); drv=$(basename "$(readlink -f "$phy/device/driver")" 2>/dev/null)
  id=$(usbid "$phy/device"); mon=no
  have iw && iw phy "$p" info 2>/dev/null | awk '/Supported interface modes/,/Band /' | grep -q '\* monitor' && mon=yes
  echo "$p iface=${dev:-?} driver=${drv:-?} usb=$id monitor=$mon"
  [ "$mon" = yes ] && monphys=$((monphys+1))
  case "$id" in 0e8d:*) [ "$mon" = yes ] && alfa=$((alfa+1));; esac
done
have iw || echo "iw missing: monitor support NOT evaluated (package iw)"
for m in mt7921u mt76x2u; do printf '%s module: ' "$m"; modinfo -F filename "$m" 2>/dev/null || echo "NOT AVAILABLE in this kernel"; done
ls -1 /lib/firmware/mediatek/ 2>/dev/null | grep -i -E 'mt7961|mt7662|WIFI_RAM' | sed 's/^/firmware: /'
[ "$alfa" -ge 1 ] && V+=("alfa-wifi|PASS $alfa MediaTek USB phy with monitor mode") || V+=("alfa-wifi|FAIL no MediaTek USB phy with monitor mode (plugged in? driver? firmware?)")
[ "$monphys" -gt 1 ] && V+=("wifi-autodetect|WARN $monphys capture-capable Wi-Fi adapters: select one with --interface")

sec "bluetooth controllers (RID.BLE = Realtek 0bda:876e)"
rtl=0
for h in /sys/class/bluetooth/hci*; do
  [ -e "$h" ] || { echo "no HCI controllers"; break; }
  id=$(usbid "$h/device"); drv=$(basename "$(readlink -f "$h/device/driver")" 2>/dev/null)
  echo "$(basename "$h") usb=$id driver=${drv:-?} $( [ -e "$h/device" ] && echo)"
  [ "$id" = 0bda:876e ] && rtl=$((rtl+1))
done
ls -1 /lib/firmware/rtl_bt/ 2>/dev/null | grep -i 8761b | sed 's/^/firmware: /' || true
have rfkill && run rfkill list
[ "$rtl" -eq 1 ] && V+=("rid-ble|PASS Realtek 0bda:876e present") || V+=("rid-ble|FAIL Realtek 0bda:876e count=$rtl")
[ -n "$(ls /lib/firmware/rtl_bt/ 2>/dev/null | grep -i 8761b)" ] && V+=("rtl-firmware|PASS rtl8761b* present") || V+=("rtl-firmware|FAIL rtl_bt/rtl8761b* missing (firmware-realtek)")

sec "gps (u-blox, USB serial)"
ls -l /dev/serial/by-id/ 2>/dev/null || echo "no /dev/serial/by-id"
gps=0; for t in /sys/class/tty/ttyACM* /sys/class/tty/ttyUSB*; do [ -e "$t" ] || continue; id=$(usbid "$t/device"); echo "$(basename "$t") usb=$id"; case "$id" in 1546:*) gps=$((gps+1));; esac; done
[ "$gps" -ge 1 ] && V+=("gps|PASS u-blox serial present") || V+=("gps|FAIL no u-blox (1546:*) serial device")

sec "power / thermal / kernel log"
for z in /sys/class/thermal/thermal_zone*; do [ -r "$z/temp" ] && echo "$(cat "$z/type"): $(( $(cat "$z/temp")/1000 ))C"; done
if have vcgencmd && have python3; then
  python3 - <<'POWER_PY'
import os, signal, subprocess, tempfile, time
# A regular anonymous output file cannot keep the survey's tee pipe open.
# Do not use subprocess.run/communicate: their kill cleanup can wait forever.
with tempfile.TemporaryFile() as output:
    process = subprocess.Popen(['vcgencmd', 'get_throttled'], stdin=subprocess.DEVNULL,
                               stdout=output, stderr=subprocess.DEVNULL, start_new_session=True)
    deadline = time.monotonic()+5
    while process.poll() is None and time.monotonic() < deadline:
        time.sleep(.05)
    if process.poll() is None:
        try: os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError: pass
        print('WARNING: power check: unknown (firmware did not answer). Reboot the Pi before retrying.')
    elif process.returncode:
        print('WARNING: power check: unknown (firmware query failed). Reboot the Pi, then retry.')
    else:
        output.seek(0)
        print(output.read(256).decode('ascii', 'replace').strip())
POWER_PY
elif have vcgencmd; then
  echo 'WARNING: power check: unknown (Python 3 is needed for a bounded firmware query).'
fi
if dmesg >/dev/null 2>&1; then K="dmesg"; else K="sudo -n dmesg"; fi
$K 2>/dev/null | grep -i -E 'mt7921|mt76|btrtl|rtl8761|firmware|over-current|undervolt|under-voltage|usb .*reset|disconnect' | tail -40 \
  || echo "(kernel log unreadable without sudo; re-run with sudo for this section)"

sec "SUMMARY"
for v in "${V[@]}"; do verdict "${v%%|*}" "${v#*|}"; done
echo; echo "saved: $OUT"
