<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Bluetooth hotplug behavior and native acceptance

Absent `sensors.rid_ble` means enabled-auto with targeted rfkill unblock.
Existing explicit sections retain their defaults: omitted `enabled` and omitted
`unblock_rfkill` are false. Explicit `enabled: false` is deliberate off. An
engine-only update never opts an existing section into rfkill unblock. A host
installer rerun retains its existing migration policy of adding missing
`unblock_rfkill: true` to an enabled section; explicit false remains false.
A fresh install with one supported adapter still pins its USB ID. An install
without one leaves intent implicit. Hotplug and the engine never rewrite config.
The helper records the previous bluetoothd state before claiming it; this host
cannot serve Bluetooth keyboards/audio while RID.BLE owns Bluetooth.

The engine rescans HCI sysfs about every five seconds, uses a read-only
HCIGETDEVINFO snapshot for addresses, and binds the selected controller's USER
channel exclusively. Bind/setup/loss errors become waiting with a reason. Health
requires actual advertisements, not just a successful takeover. An unsupported
adapter is named with the recommended IDs; ambiguous matches refuse. A missing
pin names present hardware and suggests `sensors.rid_ble.adapter: {}` or a new pin.

The HCI ADD rules match only 0bda:876e / 0bda:a728. A system timer repeats the
oneshot helper every five seconds to cover delayed setup/missed events and
observe an operator unmask/start. Helpers serialize apply/restore with a root-only
runtime lock. A saved receipt plus running/unmasked bluetoothd latches a conflict;
the engine yields, and automatic helpers never re-mask. Explicit helper `apply`
reclaims after operator review, or set `enabled: false` for prior-state restoration.
Selection is validated before installer ownership changes and revalidated after
stopping the engine. A failed installer claim restores prior state and removes a
receipt created by that attempt only after successful Bluetooth restoration.
Failed rollback and incomplete hotplug claims retain receipts for explicit recovery.

## Known limits

- Both engine and host installer/Quadlet changes are needed. Engine-only updates
  do not install ownership automation on older hosts. Existing read-only config
  binds require engine restart after manual pin/config changes. Explicit off with
  a receipt is handled by the helper's stop/restore/restart path. Without a receipt,
  the helper leaves services unchanged and prints the required restart command:
  `sudo systemctl restart brrdfeeder-engine.service`. Run it after saving OFF so
  the running engine reloads the config and status reports Disabled.
- Five seconds is a polling interval, not a hard activation deadline: driver setup,
  helper execution and incoming advertisers add latency. A healthy idle receiver
  becomes degraded after its configured quiet window. No advertisement is not
  evidence of a working RF path.
- A brief manual service change entirely between observations is not guaranteed
  to be detected. A detected conflict remains latched until explicit reclaim or
  restore (runtime state resets on reboot; persisted unmask is observed again).
- Multiple matching supported adapters refuse; a BD_ADDR pin can select one.
  No automatic rebinding of a missing pin. Unsupported controllers never get
  claimed. USB hardware/firmware and radio behavior require native Pi proof.
- Same-name Wi-Fi replug and GPS recovery are existing behaviors. Wi-Fi replacement
  under a different interface name and multiple-GPS ambiguity remain outside scope.
- No telemetry/F3411 schemas or audit event types change; reasons are local status.

## Native Pi procedure (disposable test node only)

Use reviewed candidate engine and installer builds together; retain their hashes,
OS/kernel version, install output, status snapshots and journals. Record the engine
`systemctl show brrdfeeder-engine -p InvocationID -p MainPID` before each plug test.
Use `sudo brrdfeeder status`, `/var/lib/brrdfeeder-status/status.json`, console radio
view and `journalctl -u brrdfeeder-ble-takeover -u brrdfeeder-engine` for evidence.

1. Install without a supported adapter and with bluetooth.service running/unmasked.
   Verify the automatic-activation message, absent key, no receipt and unchanged
   bluetoothd. Capture the config SHA256.
2. Plug one 0bda:876e (repeat with 0bda:a728). Observe udev/oneshot invocation, receipt
   written before stop/disable/mask, and engine binding without InvocationID/PID
   change. Confirm config hash unchanged. With a known legal RID transmitter or
   BLE advertiser, require Healthy plus increasing frames; do not accept bind alone.
3. Unplug: require waiting/impaired coverage and cleared selected-adapter inventory.
   Replug: require automatic Healthy/frames recovery and the same engine invocation.
4. Remove supported hardware; insert an unsupported HCI adapter. Require its actual
   VID:PID and 0bda:876e / 0bda:a728 recommendation. It must not be claimed. Plug a
   supported replacement and verify recovery.
5. Set `sensors.rid_ble.enabled: false`. Observe receipt-based restoration and engine
   restart; verify Disabled locally. Unplug/replug twice: no claim/mask and BLE stays
   off. Check Wi-Fi/GPS still operate. Also repeat from step 1's running auto-on,
   no-adapter, no-receipt state: save `sensors.rid_ble.enabled: false`, then run
   `sudo systemctl restart brrdfeeder-engine.service`. Require a new InvocationID,
   fresh local status showing Disabled, no receipt, and unchanged bluetoothd.
   Replug twice and require Disabled with no ownership claim. Merely editing the
   file while the old engine keeps waiting is not a pass.
6. Enable auto selection (`adapter: {}`), restart the engine to load it, and plug
   two supported adapters. Require refusal and no guessed receiver. Remove one and
   require recovery. Pin the missing model while the other is present: require
   both identities and the suggested config fix. Test an explicit BD_ADDR selection.
7. While active, manually `sudo systemctl unmask bluetooth.service` and start it.
   Within two polling intervals require a conflict and USER ownership released;
   verify no automatic stop/mask on repeated hotplug. Explicitly reclaim with
   `sudo /usr/local/libexec/brrdfeeder-bluetooth apply`, then start the engine and
   verify recovery. Repeat unmask-only observation. Preserve original receipt.
8. Replug GPS and the Wi-Fi capture adapter under the same interface name. Require
   existing GPS/fix and Wi-Fi capture recovery. Record failures separately.
9. Reboot with adapter present; require automatic recovery. On the disposable node,
   uninstall and verify prior bluetoothd state restored and no takeover unit/timer,
   package udev rules or `/run/brrdfeeder-bluetooth` residue remains.

Record actual timings, frame evidence and refusals. Native proof is outstanding
until these steps run on the candidate; hardware-free tests do not replace it.
