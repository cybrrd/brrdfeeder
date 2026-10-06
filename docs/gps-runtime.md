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

## Unsupported serial adapters

Automatic mapping remains restricted to u-blox `1546:01a5` through `1546:01a9`.
CP210x (including `10c4:ea60`), FTDI and other USB serial IDs do not identify a
GPS receiver. They are reported, never automatically claimed or probed.

Use a supported receiver, or have an operator explicitly review the receiver,
its serial identity, ownership and `sensors.gps.device`/baud configuration before
opting in. This release does not add an Adafruit support claim, broad CP210x/FTDI
udev rule, PMTK commands, PPS configuration or chrony discipline changes.

## Verification boundaries

Offline tests cover actual Podman start using the helper's absent transport,
real pseudo-terminal first-fix/unplug/replug, bounded restart decisions, truthful
heartbeat fields and local status rendering. Device-node creation for a present
receiver is unit-tested; physical USB re-enumeration and downstream map display
still need the field acceptance check. No test contacts a deployed node.
