<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Wi-Fi capture adapter identity (0.9.1 proposal)

This follow-on must not land before the v0.9.0 tag. It closes the case where a
USB capture adapter returns under a different interface name. It does not change
RID/F3411, frame contents, decoding, filtering, publication or device credentials.

## Configuration and compatibility

Existing `capture.interface: wlan1` remains an exact interface-name selection.
An upgrade or installer rerun never rewrites that choice or falls back to another
adapter. Explicit first-install `--interface` keeps that behavior too.

New automatic first installs use:

```yaml
capture:
  adapter_usb_id: auto
  hunter:
    enabled: true
```

The installer still requires exactly one eligible adapter at first install;
absence is an actionable install refusal, not a capture-free success. Automatic
selection considers USB Wi-Fi adapters with a bound driver and a wiphy that
advertises monitor-mode support. It never picks the first interface in a list.
Explicit `--interface` is the escape hatch when the operator intends a particular
interface. A manually configured `adapter_usb_id: "0e8d:7961"` restricts the
automatic selector to that USB model. A matching model still needs a working
driver and monitor capability, and two matching adapters remain ambiguous.

Exactly one of `interface` or `adapter_usb_id` is required. Both, neither, an
empty value or a malformed selector is a configuration error. There is no
implicit conversion of old configuration. Opting an existing install into USB
selection is an explicit edit replacing `interface`, with a separate restart.
Automatic mode allows a different supported USB model to replace the old one;
the pinned USB-model mode does not silently broaden its selection.

## Selection and recovery

The engine reads `/sys/class/net` and canonical device ancestry. A candidate must
have a wireless phy index, positive ifindex, a USB-interface driver link and a
USB-device ancestor whose idVendor/idProduct agree with its kernel uevent.
The binding includes that canonical USB path, driver, USB ID, phy, interface name
and current ifindex. A name alone, driver name alone or an old ifindex is never
authority in USB mode. Runtime uses sysfs/uevent observations, not a new root udev
service, a naming rule or a config-writing daemon.

Monitor support comes from read-only nl80211 GET_WIPHY, not an assumption that
an interface's current managed mode means monitor mode is unsupported. Missing,
incomplete or failed observations cannot establish eligibility. Built-in/non-USB
radios are not automatic candidates. Before monitor/channel administration, the
chosen binding must still match sysfs and must not carry an IPv4 or IPv6 default
route. Ambiguity is not resolved by excluding an uplink or keeping a prior pick:
two eligible adapters are a refusal with the observed choices and the explicit
configuration remedy.

The capture outer loop selects before opening, revalidates after establishing
monitor mode and retries failures. It also checks selection every two seconds
while a pcap handle is open, including when no RID frames arrive. Removal,
driver/identity replacement, a new name/index or added ambiguity invalidates the
old binding, prevents further radio administration, requests capture reopen and
returns to selection. Startup with an absent adapter uses the same retry path;
it does not depend on having captured a first packet. No config write, install
rerun or engine restart is needed for automatic recovery.

The watchdog and channel scheduler share the selected binding; they never
independently adopt a different adapter. Channel administration revalidates the
binding and route guard immediately before its netlink call. A missing binding
means wait, not use a cached index. A replacement also resets a parked schedule
so its channel is established on the new radio. As with all userspace device
control, physical removal can race the following kernel call; kernel errors
cause retry, never a search for another interface by index.

Radio Up still requires a real captured packet; finding an adapter, setting
monitor mode or opening libpcap establishes only Recovering. Waiting, unsupported
driver/capability, ambiguity and uplink conflicts have a clear journal message
and an additive **local status-only** selection reason. Existing heartbeat,
telemetry, audit and RID fields retain their meaning and contents. Legacy name
mode retains its prior status shape. No health result is invented by a mock.

## Implementation boundaries

Keep the USB selector and sysfs observer in an engine-local module with injected
sysfs root, capability and route observations for tests. Add read-only nl80211
capability parsing with hand-encoded message tests. Use existing native monitor
control, pcap reopen, liveness, route guard and channel scheduler paths. Keep the
installer change limited to fresh automatic Wi-Fi selection/config generation
and local reason display; do not change Bluetooth ownership or existing configs.
Recompute all three installer bootstrap checksums after any installer edit.

## Hardware-free acceptance and refusal controls

Use temporary fake sysfs symlink trees and USB uevents plus mocked capability,
route and administration calls; no host radio, enrollment or network effects.
Exercise absent → add → remove → renamed/reindexed add; different-model automatic
replacement; pinned-model refusal; missing driver; monitor unsupported/unknown;
two capable USB adapters including an added second adapter during capture;
multiple interfaces on one phy; uplink refusal for both IP families; canonical
USB ancestry; changed driver/path with reused index; old explicit name preserved;
and parked-channel reset. Assert that refusal never calls radio administration.
Prove that opening alone does not produce Up and that reasons reach local status
without altering the existing heartbeat object.

Negative controls must make tests fail if ambiguity is replaced by first-match,
USB identity/driver/capability checks are bypassed, explicit-name mode falls back
to USB selection, a stale binding is administered, active selection is not
rechecked, or an installer rerun rewrites existing config. Existing public
offline installer/engine gates remain required; a failed gate stops release work.

## Later native Raspberry Pi procedure (not run by this proposal)

Use a separately approved disposable Pi installation and candidate image bound
to its exact source/build/digest, with GPS and a separate network uplink intact.
The reference capture adapter is MediaTek `0e8d:7961`, normally `wlan1`.

1. Record image/source identity, fresh status and service invocation; inspect
   only public adapter attributes and logs, never config credentials or secrets.
   Confirm the capture adapter carries neither IPv4 nor IPv6 default route.
2. Fresh install without `--interface`: exactly one capable USB adapter yields
   `adapter_usb_id: auto`. Explicit-interface install and a rerun of an existing
   name-based install preserve their exact name and config bytes.
3. With automatic mode, unplug before engine start. The service stays alive,
   local status gives a waiting reason and radio is not Up. Plug the MediaTek:
   it enters monitor mode, channel scheduling starts and genuine received frames
   establish Up. Record bounded detection/recovery time without restarting.
4. Unplug/replug; then use a separately prepared renamed network interface or
   a replacement adapter that naturally receives another name. Verify fresh
   ifindex/USB/driver observations, reopen, channel retune and real frame flow.
   Repeat with the parked schedule to catch stale parked-channel state.
5. Add two capable USB adapters. Within the rescan interval capture stops with
   an ambiguity reason and neither is newly administered. Remove one and verify
   autonomous recovery. Also exercise two interfaces on one capable phy.
6. Present a non-monitor-capable or driver-unbound USB device, then a working
   adapter. Record the clear refusal/recovery reason; built-in Wi-Fi and the
   network uplink must remain unchanged. Test a pinned `0e8d:7961` config with
   a different model: it waits for the requested identity rather than adopting.
7. On an isolated test network only, give the candidate a default route (each
   IP family separately); verify refusal before mode/channel administration and
   retained uplink connectivity. Restore the dedicated-radio setup afterward.
8. Compare complete captured RID fixtures before/after and inspect existing
   heartbeat/audit fields. Record no field removal, masking, hashing, relabeling
   or withholding. BLE/GPS and legacy same-name recovery are regression checks.

Stop on any discrepancy and retain source/digest, invocation-scoped logs,
public adapter observations and fresh local status. Native acceptance is a later
gate; hardware-free tests are not a substitute for driver/pcap/container proof.
