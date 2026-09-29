#!/usr/bin/env bash
# Bench-only arrival gate for a candidate RID.BLE USB controller.
set -euo pipefail

dry_run=0
bench_attested=0
hci_index=
lsusb_file=
dmesg_file=
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
probe="${script_dir}/../../../notes/rid-ble-field-2026-09-17/hci-user-probe.py"

usage() {
  cat <<'EOF'
Usage: bt-dongle-gate.sh --bench [--hci-index N] [--probe PATH]
       bt-dongle-gate.sh --dry-run

Run only on lamplab or a physically isolated bench Pi, never a fleet node.
--bench is the operator attestation. The controller must be administratively
DOWN and free from bluetoothd before the USER-channel probe.
EOF
}

stop() { printf 'STOP %s\n' "$*" >&2; exit 2; }
reject() { printf 'REJECT %s\n' "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) dry_run=1; shift ;;
    --bench) bench_attested=1; shift ;;
    --hci-index) hci_index=${2:?missing HCI index}; shift 2 ;;
    --probe) probe=${2:?missing probe path}; shift 2 ;;
    --lsusb-file) lsusb_file=${2:?missing lsusb fixture}; shift 2 ;;
    --dmesg-file) dmesg_file=${2:?missing dmesg fixture}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) stop "unknown option: $1" ;;
  esac
done

if [[ $dry_run -eq 1 ]]; then
  printf '%s\n' \
    'CHECK bench attestation and refuse an active fleet engine' \
    'CHECK lsusb Realtek PID: 876e/8771 proceed; a728 reject; unknown stop' \
    'CHECK dmesg btusb/btrtl loaded rtl8761bu firmware' \
    'CHECK choose one HCI index and run hci-user-probe.py via USER channel' \
    'CHECK require coded_phy=True, extended_adv=True, and PROBE OK' \
    'STOP dry-run does not touch the adapter and does not certify PASS'
  exit 2
fi

[[ $bench_attested -eq 1 ]] || stop "--bench attestation required; fleet-node execution is forbidden"
if command -v systemctl >/dev/null 2>&1 && systemctl is-active --quiet brrdfeeder-engine.service 2>/dev/null; then
  reject "brrdfeeder-engine.service is active; this appears to be a fleet node"
fi
if pgrep -f 'brrdfeeder-src/engine/target/release/engine' >/dev/null 2>&1; then
  reject "BRRDfeeder engine process is running; arrival gate is bench-only"
fi
if [[ ${BT_GATE_ALLOW_NONROOT_TEST:-0} != 1 && $EUID -ne 0 ]]; then
  stop "run as root for HCI_CHANNEL_USER"
fi

if [[ -n $lsusb_file ]]; then
  [[ -r $lsusb_file ]] || stop "unreadable lsusb fixture: $lsusb_file"
  lsusb_output=$(<"$lsusb_file")
else
  command -v lsusb >/dev/null 2>&1 || stop "lsusb is not installed"
  lsusb_output=$(lsusb -d 0bda: 2>/dev/null || true)
fi

mapfile -t realtek_ids < <(printf '%s\n' "$lsusb_output" | LC_ALL=C grep -Eio '0bda:[0-9a-f]{4}' | tr '[:upper:]' '[:lower:]')
[[ ${#realtek_ids[@]} -gt 0 ]] || reject "no Realtek USB device found"

accepted=()
for usb_id in "${realtek_ids[@]}"; do
  pid=${usb_id#0bda:}
  case "$pid" in
    876e|8771) accepted+=("$usb_id") ;;
    a728)
      reject "0bda:a728 has kernel-confirmed broken LE extended scan (5ead2063611a; narrowed by ca0583c24661)"
      ;;
    *)
      printf '%s\n' '--- dmesg btusb/btrtl lines ---' >&2
      if [[ -n $dmesg_file && -r $dmesg_file ]]; then
        grep -Ei 'btusb|btrtl|rtl.*bluetooth|bluetooth.*rtl' "$dmesg_file" >&2 || true
      else
        dmesg 2>&1 | grep -Ei 'btusb|btrtl|rtl.*bluetooth|bluetooth.*rtl' >&2 || true
      fi
      stop "unclassified Realtek PID $usb_id"
      ;;
  esac
done
[[ ${#accepted[@]} -eq 1 ]] || stop "expected exactly one accepted Realtek dongle; found ${#accepted[@]}"
printf 'CHECK USB %s accepted RTL8761BU-family PID\n' "${accepted[0]}"

if [[ -n $dmesg_file ]]; then
  [[ -r $dmesg_file ]] || stop "unreadable dmesg fixture: $dmesg_file"
  dmesg_output=$(<"$dmesg_file")
else
  dmesg_output=$(dmesg 2>&1 || true)
fi
printf '%s\n' "$dmesg_output" | grep -Eiq 'rtl8761bu(_fw)?(\.bin)?' || \
  reject "kernel log does not show expected rtl8761bu firmware load"
printf '%s\n' 'CHECK kernel loaded rtl8761bu firmware'

if [[ -z $hci_index ]]; then
  mapfile -t hci_devices < <(find /sys/class/bluetooth -maxdepth 1 -type l -name 'hci[0-9]*' -printf '%f\n' 2>/dev/null | sort)
  [[ ${#hci_devices[@]} -eq 1 ]] || stop "choose the candidate explicitly with --hci-index N"
  hci_index=${hci_devices[0]#hci}
fi
[[ $hci_index =~ ^[0-9]+$ ]] || stop "invalid HCI index: $hci_index"
[[ -r $probe ]] || stop "probe not readable: $probe"

set +e
probe_output=$(python3 "$probe" "$hci_index" 2>&1)
probe_rc=$?
set -e
printf '%s\n' "$probe_output"
[[ $probe_rc -eq 0 ]] || reject "hci-user-probe.py exited $probe_rc"
printf '%s\n' "$probe_output" | grep -q 'coded_phy=True' || reject "controller does not report LE Coded PHY"
printf '%s\n' "$probe_output" | grep -q 'extended_adv=True' || reject "controller does not report extended advertising"
printf '%s\n' "$probe_output" | grep -q '^PROBE OK$' || reject "probe did not reach PROBE OK"

printf 'PASS %s hci%s rtl8761bu USER-channel Coded PHY + extended advertising\n' "${accepted[0]}" "$hci_index"
