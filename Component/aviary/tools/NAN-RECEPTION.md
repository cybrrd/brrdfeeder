<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->

# Wi-Fi NAN reception evidence and bench recipe

`router::ingest_frame` admits ODID NAN Service Discovery Action frames, checks
all NAN attribute boundaries, selects the ODID service descriptor, and passes
counter-stripped bytes to the same strict Message Pack decoder as Beacon RID.
Relevant corpus IDs: BWF0010, BWF0020, BWF0030, BWF0036, BWF0040, BWF0050,
BWF0060, BWF0100. Sync-beacon separation is covered by BWF0034 fixtures.
These are software checks, not RF conformance evidence for BWF0032, BWF0070,
BWF0080, or BWF0090.

Only the standard publish/service-info layout (`service_control = 0x10`) is
supported. Unknown attributes and unrelated services are skipped, all outer
lengths are checked, and duplicate ODID service descriptors are rejected as
ambiguous for a one-observation API. The descriptor's service-info length must
match exactly; the strict decoder imposes size 25, counts 1–9, and exact length.
The capture adapter removes FCS only when radiotap declares it, and rejects
NAN frames marked as having a bad FCS. Encrypted/fragmented Action frames are
not parsed as complete SDFs.

The Beacon walker also implements the review follow-up: an ODID IE with a
malformed RID body is skipped when its outer IE length still identifies the
next boundary. A later valid ODID IE may then produce the observation. An outer
IE that overruns the frame still stops the walk; no boundary is guessed.

## Identity and wire provenance

`mac_address` remains the received Addr2. It is not a stable aircraft key:
NAN source MACs may rotate. The engine's NAN dedup key uses `drone_id` resolved
from Message Pack identity (serial, else registration); other transports keep
their existing `(drone_id, MAC)` key. Two serials remain distinct even when
they share one source or cluster address. As elsewhere in RID, an unauthenticated
broadcast identifier is a correlation hint, not proof of physical identity.

Identity-less NAN observations still reach raw frame output but bypass dedup
and the aircraft observation cache. They are not merged into an `UNKNOWN`
aircraft and are not associated with a later identity by MAC. Downstream
consumers must likewise use pack identity for NAN correlation and preserve
source-address observations as provenance.

`wifi_bssid` is an additive optional wire-v4 string with the observed Addr3 in
canonical hex form. It is populated only for NAN, so Beacon and BLE JSON are
unchanged. A BSSID such as `50:6f:9a:01:00:ff` identifies a shared cluster, not
an aircraft. Receive channel/frequency fields are deferred to the channel
strategy slice, where radiotap measurement can be carried distinctly from
the receiver's configured channel. No wire version bump is needed.

## Independent implementation oracle

Source: [OpenDroneID C library at the pinned commit](https://github.com/opendroneid/opendroneid-core-c/tree/6484f26545d4f012682524e2d843fab0fbdc0b34).
Upstream commit: `6484f26545d4f012682524e2d843fab0fbdc0b34`.
Upstream license: Apache-2.0. No upstream source is copied into the engine.

`nan-oracle.c` is a test-only driver that links externally against the reference
library. For each count 1–9 and counters `00`, `EF`, `F0`, and `FF`, it calls
`odid_wifi_build_message_pack_nan_action_frame`, then independently decodes
those bytes with `odid_wifi_receive_message_pack_nan_action_frame`. It emits
frame hex and the reference decoder's fields. A real reference sync-beacon
builder also supplies the negative fixture; only its clock bytes are zeroed
for repeatability. The generated synthetic fixture has its own license sidecar.

With this repository as `$PUBLIC` and a checkout of the upstream commit as
`$ORACLE`, reproduce outside the shipped build:

```sh
test "$(git -C "$ORACLE" rev-parse HEAD)" = 6484f26545d4f012682524e2d843fab0fbdc0b34
cc -std=gnu11 -Wall -Wextra -I "$ORACLE/libopendroneid" \
  "$PUBLIC/Component/aviary/tools/nan-oracle.c" \
  "$ORACLE/libopendroneid/opendroneid.c" "$ORACLE/libopendroneid/wifi.c" \
  -lm -o nan-oracle
./nan-oracle > regenerated.json
cmp regenerated.json "$PUBLIC/Component/aviary/cybrrd-rid-protocol/tests/fixtures/nan-oracle.json"
cargo +1.88.0 test --locked --manifest-path "$PUBLIC/Component/aviary/Cargo.toml" \
  -p cybrrd-rid-protocol --test wifi_nan
```

The Rust test compares every emitted reference field, including serial,
aircraft/operator position and altitude, status, operator ID, self ID, and
authentication metadata. Its separate handwritten sweep covers all 2,304
counter/count combinations. Negative controls have paired valid inputs and
service-ID/counter-boundary mutation receipts in review evidence.

## Later RF bench

An available stimulus is [ArduRemoteID](https://github.com/ArduPilot/ArduRemoteID)
on a supported ESP32-S3/C3 board. Before the bench, pin its firmware commit and
record its actual NAN configuration and programmed test identity. On an isolated
bench, use synthetic stationary identity/location data and enable NAN on
channel 6. Capture monitor-mode PCAP with radiotap from the bench receiver,
plus raw `wifi_nan` output, for at least a full 256-counter cycle.

Verify the captured category/action, service ID, source Addr2, cluster Addr3,
pack count, and counter progression against the output. Confirm the same
identity under a changed source MAC does not split a track, and a second
identity in the same cluster remains separate. Include sync beacons and
unrelated public-action traffic as negatives. Retain firmware/config hashes,
PCAP hash, receiver build, channel/frequency measurement, loss counts, and field
comparison. No bench hardware or radio configuration was changed for this PR.

Status is implemented with synthetic and independent-oracle evidence. Real
NAN transmitter reception, interoperability under RF loss, and channel/timing
coverage remain unproven.
