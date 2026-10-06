<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->

# Wi-Fi channel scheduler — operator guide

The receiver's objective is the **set** of aircraft in range, not a good track
for whichever aircraft spoke first. Two earlier designs each broke that goal in
opposite ways: dwelling to follow a heard drone made the node a single-aircraft
tracker; blind 200/400 ms rotation starved the social channels to ~10% of wall
time and turned ≥1 Hz Remote ID into ~4–5 s observation gaps (a receiver
artifact that was briefly misread as a transmitter cadence floor).

## Presets (`capture.hunter.preset`)

| Preset | Behaviour | Use when |
|---|---|---|
| `social` (**default**) | ch 6 and 149 dwell **1200 ms** each per visit; every other configured channel **220 ms**; ±15% jitter; sweep reshuffled each supercycle; social channels placed ~half a supercycle apart | The network-receiver default. Dense RID environments, NAN reception, social-channel Beacons |
| `park6` | Park the first social channel (6). No rotation, no retune churn | Dense single-band sites that accept blindness elsewhere (a second radio or BLE covers the rest) |
| `sweep` | Legacy 200/400 ms rotation values and priority list | Spectrum-witness missions where equal-time sampling is the goal |

## Parameters (WHAT / WHY / WHEN-to-tune / DEPENDS)

- `dwell_priority_ms` (social dwell, default 1200)
  - WHAT: dwell on each social channel per visit.
  - WHY: spans two NAN discovery windows (512 TU = 524.288 ms each) with margin,
    and gives P(catch) ≈ 70% per visit against a 1 Hz Beacon (`BUR0010`).
  - WHEN-to-tune: raise if heartbeat `dwell_share_pct`/`rid_hits_total` show
    NAN windows missed at visit edges; lower only if sweep starvation hurts
    more than social yield.
  - DEPENDS: `BWF0090` (social channels), `BWFB0030`.
- `dwell_default_ms` (sweep dwell, default 220)
  - WHAT: dwell on every non-social channel per visit.
  - WHY: 220 ms covers the 200 TU (204.8 ms) any-channel Beacon interval
    (`BWFB0040`/`BWFB0050`) with guard time.
  - WHEN-to-tune: lower trades discovery of conformant any-channel
    broadcasters for shorter cycles.
- `jitter_fraction` (default 0.15): anti-phase-lock. The social floor
  (1000 ms) is applied after jitter, so the ≥1 s Beacon guarantee and one
  full NAN window + margin always survive.
- `jitter_seed` (default fixed): deterministic schedule — CI and fleet agree.
- `lock_on_duration_ms` (default **0** in every preset): opt-in
  **single-target audit** tool only. When > 0, a decoded drone frame defers
  rotation for at most this long (early release after `lock_on_min_ms` once
  identity + position are observed). Never enable it on a network receiver:
  a chatty identity-only emitter can hold the channel indefinitely.
- `channel_set`, `priority_channels`: explicit overrides always win over
  preset defaults (back-compat); the engine logs which fields were explicit
  at startup.

## Channel-set hygiene (2026-10-06)

- **Duplicates** in `channel_set` are deduplicated at plan resolution (first
  occurrence wins) and the startup log prints a notice, e.g.
  `[hunter] channel_set deduplicated: dropped duplicate channels [11]`.
  Dedupe with a notice rather than refusal: a duplicated entry functions
  today (it silently doubles that channel's dwell share — the bug), so the
  fix removes the double share without boot-failing a working setup over an
  operator typo.
- **DFS channels (52–144, UNII-2) and channels outside the supported
  regulatory range** (2.4 GHz 1–14, 5 GHz 36–48/149–165) are **refused at
  plan level**: the engine prints one actionable line per channel, e.g.
  `[hunter-plan] refusing channel 52: DFS channel (UNII-2, 52-144): radar
  clearance is unsupported`, then fails closed **before any radio
  administration**. This replaces the previous behavior — scheduling a dead
  visit the kernel refuses every capture cycle. The default 12-channel set
  is unaffected (no DFS, no duplicates, no notices).
- `priority_channels` entries outside the resolved `channel_set` are ignored
  (unchanged); run the hygiene rules on `channel_set` itself.

## Measuring it (C4)

- Every Wi-Fi observation on the wire carries `rx_channel` and
  `rx_channel_source` (`radiotap` when the driver reports the channel,
  `configured` when declared from the hunter; absent = unknown — never
  invented).
- The heartbeat `hunter` block reports per-channel `dwell_ms_total`,
  `rid_hits_total`, `dwell_share_pct`, plus `retune` (`last_ms`, `max_ms`,
  `count`) — measured `set_channel` round-trips that replace the cited
  8.5 ms same-silicon proxy in the model.

## Device boundary (C5)

The engine administers only the designated capture adapter — channel setting
in this slice — and **fails closed at startup** if that adapter carries the
host default route (it is the uplink, not a capture radio). The built-in
Wi-Fi/BT, routes, DNS, and firewall are never touched.

## Field validation plan (run later on a real node)

1. **Park vs social vs sweep** on one node with a known broadcaster (lyreBRRD
   tester or an own flight), 10 minutes each: expected Green inter-observation
   gaps ≈1 s (park6), ≈2 s (social on its channel), 4–9 s (sweep) — from the
   heartbeat `dwell_share_pct` + `rid_hits_total` and frame timestamps.
2. **NAN window coverage**: with a NAN emitter on ch 6, confirm every social
   visit captures ≥2 discovery windows (per-visit SDF count from the wire).
3. **Retune truth**: collect 24 h of `hunter.retune` on each driver in the
   fleet (mt7921u, rtw88_8812au); replace the 8.5 ms proxy if p99 differs.
4. **No-lock confirmation**: while ≥2 drones broadcast, verify in the
   heartbeat that every configured channel is still visited each supercycle
   (`dwell_ms_total` grows on all channels) — the aircraft-independence proof.
