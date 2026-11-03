<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# GPS absence, first-fix waiting and recovery

`sudo brrdfeeder status` and the local console distinguish:

- No configured GPS device: first-fix waiting, with USB serial adapter IDs seen.
- Device present, no measured fix: first-fix waiting, with satellites and HDOP
  when received. Unknown observations are not zeroes.
- Engine with a saved real position: position preserved, GPS not live. The
  heartbeat retains `configured_position` (`config_static`); it does not promote
  that position or an old fix to `current_position` or `gps_live`.

Without any saved location, the existing pre-start helper remains in one steady
wait until a measured fix arrives. It refreshes its local status every five
seconds and rate-limits journal reminders to once per minute. Plugging in the
GPS during this wait needs no manual restart. The engine has not started, so
there is no remote engine heartbeat for this first-ever-fix state. Starting an
engine with no position and defining its wire representation are out of scope.

With a saved location, the engine and its heartbeat/status writer remain alive
past the GPS startup grace interval. The grace interval now controls reminders
(minimum 60 seconds), not a fatal exit. Required-GPS operational publishing still
waits for both healthy GPS and trusted time; no time-trust rule is bypassed. The
console does not show a green current status when its clock is untrusted.

## Narrow container transport

The host helper prepares `/run/brrdfeeder-gps/device` before each container start.
When the configured device exists it creates a stable character inode with that
exact major/minor, owned by root:dialout with mode 0660. Removing the USB endpoint
cannot remove this inode between preparation and Podman's device lookup. When
absent, the helper uses a link to the non-removable `/dev/null`; serial setup
rejects it, and the GPS reader reports failure while retrying. No fake fixes are
created. The container retains its non-root identity, read-only filesystem and
existing capability allowlist. No host `/dev` mount or serial-device wildcard is
introduced.

The five-second host timer never restarts on absence, never interrupts a
first-fix pre-start wait, and never starts a deliberately stopped service. A new
present device identity requests one logged restart so Podman can attach that
exact device. Attempts are recorded before restart, with at most one per minute
and three per ten minutes. An unchanged identity is not retried indefinitely.
After flapping, a new stable identity can be attached when that budget reopens.
The helper neither reads the receiver nor sends receiver commands.

This choice avoids generation-time optional-device selection: Podman's
[`AddDevice` documentation](https://docs.podman.io/en/v5.4.2/markdown/podman-systemd.unit.5.html#adddevice)
and [generator implementation](https://github.com/containers/podman/blob/v5.4.2/pkg/systemd/quadlet/quadlet.go#L688-L698)
show that a `-`-prefixed device is omitted when missing at generation time.
The stable transport is prepared at service start instead.

## Receiver discovery and serial ownership

The installer maps u-blox `1546:01a5` through `1546:01a9` automatically. When
none is present and no saved or explicit USB ID selects another receiver, a
CP2102N `10c4:ea60` (used by Adafruit USB GPS hardware) is automatically checked
for NMEA. There is no GPS question or terminal timeout. A bridge ID alone does
not identify a GPS: a new mapping requires at least two checksum-valid `$GP` or
`$GN` sentences during a bounded five-second receive window at 9600 baud.
A fix is not required to identify NMEA; the normal first-fix gate still applies.

`--gps-usb-id VID:PID` still selects another USB serial receiver explicitly,
subject to the same ownership and NMEA checks. Confirmed IDs persist as
`sensors.gps.usb_id`; reruns use that ID without prompting. Other serial IDs are
reported without probing unless selected explicitly. Connect one GPS receiver;
multiple devices sharing a declared VID:PID are not individually enrolled.

Before opening the candidate and again before reading/configuring it, the probe
checks visible `/proc/*/fd` entries by character-device number, including aliases
and container device grants. A held port is skipped; incomplete process visibility
also skips it. No process is stopped and no device owner is displaced. These are
snapshot checks, not an exclusive lock against a privileged concurrent opener;
they rely on the installer's host process namespace. Rerun after releasing the
port or resolving visibility. An exact previously confirmed package-owned rule
is retained when a rerun cannot probe because the port is busy, ownership is
unknown, or the receiver is absent. No new mapping is created in those cases.

**Receive-only does not mean zero effect on the tty.** The probe opens with
`O_RDONLY | O_NOCTTY | O_NONBLOCK`, temporarily selects raw 9600 8N1 with local
receiver enabled, and attempts to restore the saved termios settings in a `finally` block
on success, timeout, read errors and interruption. Restoration is checked by
reading back the complete settings. A failed restore or verification returns
`restore-failed`, warns that the tty may remain modified, and does not authorize
a mapping even after valid NMEA. Check the receiver and restore its serial
settings before rerunning. It sends
no protocol bytes, flushes, or explicit modem-line commands. Driver-specific
open/close effects on physical modem lines still require hardware verification.
No PMTK initialization is performed by discovery.

No valid NMEA means no new mapping. Check USB power, UART wiring and 9600-baud
output, then rerun the installer. Antenna reception affects the subsequent fix;
it is not normally needed for the receiver to emit NMEA sentences.

The generated non-u-blox rule matches tty subsystem plus the confirmed VID:PID;
`CYBRRD_GPS_DECLARED=1` is assigned as a marker, not required as a prior property.
Third-party rules are reported and left untouched.

## Addition after installation

The GPS runtime service discovers a lone CP2102N `10c4:ea60` tty when the
configured `/dev/cybrrd_gps` mapping is absent. An ADD-only udev activation and
the five-second timer use the same oneshot service, so missed events, boot-time
devices and ports released by another owner converge through one serialized
path. A custom configured GPS path is an explicit binding and is never replaced
by this discovery.

The bridge ID is only a candidate. Before using it, runtime applies the same
receive-only NMEA, process-ownership and checked termios-restoration contract as
installation. Multiple candidates refuse without opening either port. A busy
port is never opened; incomplete process visibility also fails closed. Silent
receivers retry at most once per minute. `restore-failed` blocks that exact tty
identity until it is unplugged/re-enumerated so repeated probes cannot compound
an uncertain serial state.

Successful qualification is cached only for the exact tty identity under
`/run/brrdfeeder-gps`; it does not add a broad CP210x udev mapping or rewrite
configuration. The existing exact-device transport supplies that character
device at `/dev/cybrrd_gps` inside the container. If the engine is already
active, the existing bounded restart policy attaches it; a deliberately stopped
engine is never started. Replacement/re-enumerated bridges require fresh NMEA
confirmation. Native u-blox and previously installer-confirmed mappings still
win without entering this probe.

## Verification boundaries

Offline tests cover actual Podman start using the helper's absent transport,
real pseudo-terminal first-fix/unplug/replug, bounded restart decisions, late
CP2102N sysfs discovery, receive-only qualification, busy/unknown ownership,
restore-failed refusal, replacement requalification, truthful heartbeat fields
and local status rendering. Device-node creation for a present receiver is
unit-tested; physical USB re-enumeration and downstream map display still need
the field acceptance check. No test contacts a deployed node.
