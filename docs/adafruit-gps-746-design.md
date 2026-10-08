# Design note — Adafruit Ultimate GPS Breakout (#746) support

**Status:** design for the packet (`PACKET-GPS-ADAFRUIT-ULTIMATE-746-2026-10-06.md (internal queue; path elided)`,
ADDENDUM 1 released to the development seat 2026-10-08). Target: next release after 0.8.29. Repo: public
`cybrrd/brrdfeeder`. Facts below are cited to datasheet/source/repo; anything only measurable
on the device is marked **UNVERIFIED (device)**. The verifier audits; the head coach merges; the director + the head coach live-test
on brrdg3s3.

---

## 1. Facts to establish — answered from datasheet/source

### 1.1 Module and interface

| Question | Answer | Source |
|---|---|---|
| Which module/revision? | Adafruit product 746 "Ultimate GPS Breakout" — 2012–2024 revisions carried the **GlobalTop PA1616S** (MediaTek MT3339 core); current stock may carry PA1616D or the PA1616 "v3" variant. The differences (LNA presence, TCXO) do not change the NMEA/PMTK interface. | Adafruit product page 746 + its "Technical Details" and schematic PDFs; GlobalTop PA1616 datasheet family. Exact revision on brrdg3s3: **UNVERIFIED (device)** — the director can read the module's firmware sentence (`$PMTK...`, or Adafruit's `PMTK_Q` responses) or the board silkscreen when convenient; the software below does not depend on the revision. |
| Interface | **TTL UART, default 9600 baud, NMEA 0183** (2.8–5 V logic tolerant via onboard regulator/level shifts). PMTK command set (proprietary NMEA-like `$PMTK...` sentences with checksum) for configuration. | PA1616/MT3339 datasheets; Adafruit tutorial 746 (Ultimate GPS) — wiring and PMTK examples. |
| How is it wired on brrdg3s3? | **USB via Silicon Labs CP2102N bridge `10c4:ea60`** → `/dev/ttyUSB0` (by-id serial `8a843057d49df01188d7f692bb936ffa`). NOT the GPIO UART. No PPS wired (`/dev/pps*` absent). Pi serial console is on serial0 and unaffected. | Packet §"Observed on brrdg3s3" (the director, read-only 2026-10-06). |

### 1.2 PPS output

- The breakout routes the module's **1 PPS output to a labelled PPS pin** (100 ms pulse/s, per
  the PA1616/MT3339 datasheet's 1PPS specification). It is TTL-level, NOT RS-232.
- Kernel path: `dtoverlay=pps-gpio,gpiopin=<N>` (or `pps-gpio` with the new
  `gpio` property naming) → `/dev/pps0`; chrony `refclock SHM 0` via `ppsld`/`gpsd`, or
  simplest on Pi OS: chrony's own `refclock PPS /dev/pps0` (chrony ≥ 4.x supports the PPS
  driver directly with `lock`/`prefer` on a co-existing SOCK/SHM GPS source).
  Reference: kernel `Documentation/devicetree/bindings/pps/pps-gpio.yaml`; chrony docs
  (refclock PPS). **Which GPIO/chrony combination lands on brrdg3s3: UNVERIFIED (device)** —
  the wire isn't run yet; this design treats PPS as a purely optional, installer-gated add-on
  (§3.3).
- Honest limit (packet says to record it): PPS strengthens **host time** (the `host_rx`
  clock-error estimate in wire v5's observation record); it does **not** improve the radio
  receive timestamp (`radio_ts_us`, the mt7921 TSF — a different clock domain entirely; see
  the TSF probe handoff).

### 1.3 External antenna

- The breakout has a **u.FL connector with automatic active-antenna switch-over**; the module
  supplies **antenna bias (3.0–3.3 V typ., ≤ 5 mA budget per the PA1616 datasheet's VANT
  spec)** through the u.FL path. | datasheets above.
- **Marine/ship antennas are commonly 5 V (10–30 mA) bias.** Connecting a 5 V antenna to the
  3.3 V module bias either underperforms or, with a passive-antenna assumption broken, fails.
  **A bias-tee / external 5 V injector with DC-block on the module side is REQUIRED before
  the ship antenna connects.** Exact ship-antenna model/voltage: **UNVERIFIED (device)** —
  the head coach has the antenna; the installer must NOT assume, and this design adds no antenna
  automation (out of software scope — it stays a hardware-notes item).
- Field risk (recorded per ADDENDUM 1): the 2026-10-07 brrdg3s3 hub drop is suspected ESD
  via the then-ungrounded GPS bulkhead coax; the head coach is grounding it + adding a surge protector.
  **Hardware note, not a software finding** — it goes in the measurement plan's preconditions
  and the eventual hardware-list entry, not in installer logic.

### 1.4 What the software already has (build on, don't redo)

| Piece | State at 0.8.29 (main `83e57fa`) | Citation |
|---|---|---|
| Crash-loop fix | DONE in 0.8.28: absent GPS → the runtime transport grants `/dev/null` (termios rejects), engine keeps running with preserved position + GPS-fault state; heartbeat surfaces `gps.state != healthy`; the fleet-health Red (CYB1 #110) now pages on it | `docs/gps-runtime.md`; `Component/brrdfeeder/install/gps-runtime.py` (`prepare`/`should_restart`); engine `sensor_gps.rs` retry loop |
| udev rule | u-blox only: `SUBSYSTEM=="tty", ATTRS{idVendor}=="1546", ATTRS{idProduct}=="01a5–01a9"` → `/dev/cybrrd_gps`; the preflight WARNS on other USB serial IDs ("not a supported GPS … may be non-GPS hardware") — the honest boundary already exists | `brrdfeeder-install.sh:1115-1116, 3329-3330, 2895-2935` |
| Engine NMEA | The reader is generic NMEA 0183 (GGA/RMC/GSA via the `nmea` crate) — named `run_ublox_gps` historically but **nothing u-blox-specific**; baud is config (`sensors.gps.baud`, default 9600 = the Adafruit default too) | `sensor_gps.rs:15, 273-320`; `node_config.rs:325-395` |
| Interim rule on brrdg3s3 | `/etc/udev/rules.d/97-cybrrd-gps-local-adafruit.rules` (node-local, not product) — my work replaces it with the product mechanism | ADDENDUM 1 |

## 2. Requirement 1 — recognition + `/dev/cybrrd_gps` mapping

**Design: VID:PID match restricted by explicit opt-in, recorded in config, rendered into the
udev rule. Never "every CP210x is a GPS"** (CP2102N is the single most common USB-serial
bridge on earth — Arduinos, consoles, SDRs).

1. New config key `sensors.gps.usb_id: "10c4:ea60"` (optional, string, validated
   `^[0-9a-f]{4}:[0-9a-f]{4}$`). The installer renders the GPS udev rule as:
   built-in u-blox entries (unchanged) **plus one operator-declared entry per configured
   usb_id**, with an ENV marker `CYBRRD_GPS_DECLARED="1"` and a comment line naming the
   declaring key. Multiple GPS bridges → first-declared wins is NOT silent: the installer
   refuses a second candidate and names both by-id paths (the same discipline as the capture
   interface).
2. Opt-in flow at install/re-run (**as amended by the director's answer, 2026-10-08T22:34Z**): the
   existing preflight loop already enumerates USB serial candidates with their VID:PID
   (`brrdfeeder-install.sh:2895-2935`). Extend it:
   - **Interactive:** when a candidate is `10c4:ea60` **and** no u-blox is present **and**
     no `usb_id` is configured, the installer prints the recognition notice — "Silicon Labs
     CP2102N bridge detected (10c4:ea60). The Adafruit Ultimate GPS (#746) uses this
     bridge, but so do many other devices. Map it as this feeder's GPS? [y/N]" — default
     **No**, read through the installer's existing terminal discipline (the
     `BRRDFEEDER_TTY_FD` / `/dev/tty` path used by uninstall confirmation), a pipe install
     WITH a tty prompts on the tty, **no tty behaves as No** (device claims must be
     deliberate; `-y` never auto-accepts).
   - **Headless:** explicit flag `--gps-usb-id 10c4:ea60` (validated) is the only
     non-interactive opt-in; it implies the NMEA confirm below.
   - **NMEA confirm before mapping:** after any opt-in, a short PASSIVE read on the
     candidate port at 9600 baud (≈5 s; look for ≥2 checksum-valid `$GP`/`$GN` sentences)
     validates the claim. Validated → `sensors.gps.usb_id` is written and the udev entry
     rendered. Not validated → the installer records "opted in, no NMEA seen on <id>",
     renders nothing, and the node simply runs the 0.8.28 degrade path (preserved position
     + GPS fault). **Never writes to the port** during the probe.
   - **Pre-existing rules are never deleted or overwritten by us:** if another rules file
     already claims the same VID:PID → SYMLINK (e.g. brrdg3s3's interim
     `97-cybrrd-gps-local-adafruit.rules`), the installer REPORTS it (file + its effect)
     and says the operator may remove it; we neither remove nor overwrite. the director removes
     the interim rule by hand during the live test.
   - **Re-runs and auto-update:** an existing `sensors.gps.usb_id` is honored without
     prompting (idempotent re-render). The 0.8.29 auto-update path replaces only the
     engine container image — `/etc/brrdfeeder/config.yaml` and udev rules are untouched,
     so the recorded `usb_id` and its rule survive updates by construction (verified: the
     updater script never writes under `/etc/brrdfeeder` or `/etc/udev`).
3. **Optional NMEA corroboration at seed time (not claim time):** the GPS seed
   (`gps-seed.py`) already frames checksum-valid NMEA and reports `gps-waiting` with
   satellites/HDOP diagnostics; for a `usb_id`-declared device its first-fix wait doubles as
   an NMEA probe — a bridge wired to a non-NMEA talker surfaces as the existing
   "receiver silent for 15 seconds; reopening" state, and the status line names the declared
   USB ID so the operator can retract the claim. No new probing code touches the port.
4. **UART path (explicit opt-in, host config asked-never-changed):** new installer flag
   `--gps-uart /dev/ttyAMA0` (or `/dev/serial0`). Choosing it: (a) prints that Pi OS's
   serial console on the UART must be disabled (`console=serial0,115200` removed from
   /boot/firmware/cmdline.txt, `enable_uart=1` in config.txt) and **asks** before changing
   either — the head coach's explicit opt-in step per the packet boundary; (b) writes
   `sensors.gps.device: /dev/ttyAMA0` and renders a udev rule matching that kernel name
   (`KERNEL=="ttyAMA0"`); (c) the runtime transport (`gps-runtime.py`) already follows the
   configured device path — no change needed there.

## 3. Requirement 2 — engine MTK NMEA handling

The parser side needs nothing new (§1.4). What's missing is optional PMTK initialization:

- New config block, all optional, default off:
  ```yaml
  sensors:
    gps:
      init:
        pmtk: true            # gate for everything below; false = send nothing
        update_rate_hz: 10     # PMTK_API_SET_NMEA_OUTPUT ... frequency (1..10)
        nmea_set: "gga,rmc,gsa"  # comma list; empty = leave module default
        sbas: true             # PMK_API_SET_SBAS_ENABLE
  ```
- At device open (after termios, before the read loop) the reader sends the enabled PMTK
  sentences with correct NMEA checksums, **logs each sentence sent and the module's
  ack/nack ($PMTK001) verdict**, and never retries harder than once per open. Rationale for
  defaults: the PA1619-family default output (9600 baud, 1 Hz, GGA/RMC/GSA/GSV/VTG) already
  satisfies the engine; the block exists so the comparison test (§5) can pin identical
  settings on both receivers, and so a chatty default can be trimmed. Baud changes
  (`PMTK_SET_NMEA_BAUDRATE`) are deliberately **out of scope** — a wrong command bricks the
  port until power cycle; operators who need it set `sensors.gps.baud` to match the module's
  existing rate instead.
- The 128-byte line cap in `gps-seed.py` and the engine's reader both already tolerate
  PMTK ack sentences (unknown sentence types are skipped by the `nmea` crate / seed's
  `$`-framing); tests confirm no regression.

## 4. Requirement 3 — optional PPS

- Installer gains `--gps-pps <gpio>`: prints the overlay line
  (`dtoverlay=pps-gpio,gpiopin=<N>`), the chrony snippet (`refclock PPS /dev/pps0 ...`),
  and **asks** before touching `/boot/firmware/config.txt` or the chrony drop-in — the same
  ask-never-silently rule. Default: not offered unless the flag is passed.
- Engine/heartbeat: `clock` block in the heartbeat gains optional fields
  `pps_source: "/dev/pps0" | null`, `pps_locked: bool|null`, `clock_offset_ms: f32|null`,
  populated from the host's chrony tracking via the status writer's existing host-side
  channel (the engine stays containerized; chrony is host-side — the installer's PPS step
  adds a tiny `brrdfeeder-pps-status` libexec that reads `chronyc tracking -o json` /
  `chronyc sources` ONCE per status interval and drops it into the status file the console
  already reads; the engine heartbeat repeats what the status file carries, never invents
  it). This is the wire-v5 "clock source and estimated error" feed named by the packet —
  shipped behind the flag, absent when PPS is not configured (absence stays absent).

## 5. Requirement 4 — measurement plan (the director runs on brrdg3s3; I do not touch nodes)

Preconditions (hardware notes): bulkhead coax grounded + surge protector fitted (the head coach, in
progress); **ship antenna bias voltage confirmed with a multimeter BEFORE connecting**; if
5 V — bias-tee + DC-block installed; both receivers' PMTK/u-blox config pinned to the same
update rate + sentence set.

1. **Parallel baseline (1 site-day):** u-blox 7 (current) and Adafruit #746 both on
   brrdg3s3's roof vantage, both via their own USB ports; capture per-fix:
   time-to-first-fix (cold + warm), sats used (GSA), HDOP (GGA), position scatter (the
   heartbeat/status stream already carries all three at 1 Hz; `gps.state` transitions give
   TTFF). DuckDB over the status JSON is the director's existing pattern.
2. **Antenna A/B (1 more site-day each):** ship antenna on the Adafruit (via the grounded
   bulkhead), same vantage: repeat the same metrics; record the ESD/grounding state in the
   log — the 2026-10-07 hub drop makes the first hours after connection a watched window.
3. **24-h soak (best receiver):** position scatter (CEP 50/95), sat/HDOP duty cycle, hub
   stability (`journalctl -k | grep -i usb` watch).
4. Report: numbers only where measured; UNVERIFIED markers survive into the hardware-list
   PR (requirement 5 — get.cybrrd.com updates only after this exists; NOT in my PR).

## 6. What deliberately does NOT change

- The u-blox default path (VID/PID set, recognition messages) — additive only.
- The runtime transport, crash-loop semantics, gps-seed's non-probing posture.
- No hardware-list/get.cybrrd.com edits (gated on the measurement).
- No serial-console/overlay/config.txt changes without the explicit ask.
- No PMTK baud command (bricking risk); no antenna bias automation.

## 7. Open questions (none blocking the build)

1. Ship-antenna model + bias voltage (the head coach, before the antenna A/B) — measurement-plan input.
2. Board revision on brrdg3s3 (curiosity + hardware note; software is revision-agnostic).
3. Whether the PPS status helper should also feed command's Silver view (out of scope here;
   the fleet-health work reads gps vitals, not chrony — noted for a later cycle).
