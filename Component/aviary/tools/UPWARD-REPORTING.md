<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->

# Upward reporting: initial distress and delivery status

Upward reporting is opt-in through the `upward` configuration block and needs
reviewed broker stream/subject grants. Operational (Silver) and Red have
independent queues, spools and connections. PubAck confirms broker acceptance,
not delivery to a downstream observer.

## Red boot grace

Initial unready sensors are a **deferred episode**, not an immediate alarm.
`upward.red_boot_grace_s` defaults to 300 seconds, is clamped to 0..3600, and
uses monotonic elapsed time from monitor startup. Zero restores immediate
edge-triggered emission. The monitor still starts before the required-GPS gate.

WHAT: delay monitor-generated distress emission after a fresh engine session.
WHY: allow GPS, clock and radio boot transients to settle. WHEN to tune: raise
for known slow-start sites; use zero for immediate diagnostics. DEPENDS: keep
this setting at least `sensors.gps.startup_grace_secs + 60`; raising GPS startup
grace above 240 seconds without raising Red grace violates that tuning floor.

Suppression is blanket: any monitor fault that begins and clears inside grace
does not alarm, including a real short-lived failure. Every still-true fault
emits once at the first monitor tick after expiry (normal tick: five seconds).
Suppressed faults do not enter the emitted-edge set. Clear-and-return after
expiry emits a new edge, and each engine instance starts its own grace.

Silver continues publishing observed health during grace; the legacy heartbeat
is unchanged. Unready is never relabeled healthy. Existing update-outcome Red
receipts are processed separately and are not delayed by the sensor-fault grace.

## Acknowledged does not mean disk-durable

Routine Operational reports wait for durable staging. Red retains its live
escape path when local storage fails, with an explicit disk fault (`fault=1`).
A real PubAck retires the delivered record but cannot claim to have repaired
that disk. Durable acknowledged delivery clears the previous fault, including
connect/publish failure; non-durable Red delivery retains the disk fault.
Historical failure counts remain available, and later failures re-arm status.

The R1 handoff corrected an unconditional fault clear, not grace timing. Its
broker regression already received exactly one Red PubAck; the failed assertion
was the still-broken disk status. That assertion remains unchanged.
