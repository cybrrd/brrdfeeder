# aviary — the cyBRRD sensor codebase (one codebase, four build-time profiles)
**Platform:** cyBRRD · **Index:** C1AVI — hosts C1BRD (profile `opensource`, fielded) · C1SEN + C1WDN (profile `enterprise`, design) · sensor-side C1HMB (profile `hummingbrrd`, design) · **Family:** A · **Status:** active
**Source of record:** CYB1 (this directory) since 2026-09-13; provenance in BUILD_INFO.md · supersedes gitea `cy/aviary` @ 309c0d0 (to be archived read-only, R3) and github `macawi-ai/BRRDfeeder` @ ef0eec2 (ADR 0001)
**Roster:** design/cybrrd-component-roster.md · **The product is called BRRDfeeder** (R8) — its public cut lives at `Component/brrdfeeder/`
# aviary

## Fleet installer refresh

The [bootstrap procedure](deploy/bootstrap/README.md) refreshes an existing
rootful Quadlet while preserving its image. Pass each node's config explicitly:
cathartes `/etc/brrdfeeder/config.yaml`, robin
`/home/synth/brrdfeeder/config.yaml`, cardinal `/home/synth/config.yaml`.
After authorized kit scp/extraction, run `--config <path> --verify`, then
`--dry-run`, review the diff, and apply. Apply records
`/var/lib/brrdfeeder-deploy/<ts>/installer/` and exits 3 (restart required)
unless `--restart-engine` is supplied. Require engine active, fresh
`nats] connected`, and this node's heartbeat within 180 seconds; otherwise use
the receipt's `ROLLBACK.sh` and repeat the health gate. Bare-metal user-service
installation is deprecated; image swaps remain separate `roll-engine.sh` work.

**The consolidated cyBRRD sensor lineage — one codebase, four profiles.**

`flock` enrolls the birds. **aviary defines them.**

> This repository is **private**. It holds all profiles, including the commercial ones.
> The open-source edition is a *cut* of this repository, published outward.
> See [ADR 0001](docs/decisions/0001-consolidated-lineage.md) §4 — the direction is one-way.

## What this is

A single codebase producing profile-specialised builds of the cyBRRD drone-detection
platform. The reference is **SOHO** — the most capable single-unit profile — and every other
tier is a **subtraction** from it:

```
SOHO (reference)      Wi-Fi 2.4/5/6 · BLE 5.4 · PCIe MT7925 · eMMC · triband · curated GNSS
   ├── Enterprise   = SOHO + MPR + specialised sensors + ECOLOGEE SUBSTR8   (the only superset)
   ├── Opensource   = SOHO − BLE − PCIe − eMMC        (USB Wi-Fi, USB GNSS, microSD)
   └── hummingBRRD  = SOHO − everything but 2.4 GHz Wi-Fi   (ESP32-S3, phone-paired)
```

The governing principle: **you can subtract from a superset reliably; you cannot extrapolate
a superset from a subset.**

## Honest status

| Profile | Status | Backed by |
|---|---|---|
| **Opensource** | **fielded** | Live on cardinal, robin, cathartes-aura. Flight-validated RF→edge→NATS→JetStream. |
| SOHO | **design** | Decided, not built. Hardware gated (see below). |
| Enterprise | design | Warden/Sentinel split defined; MPR designed. |
| hummingBRRD | design | Not started. |

**The reference profile is not yet a build.** As of 2026-07-19: eMMC and programmer ordered
but not arrived; the MT7925 has **no driver bound** on kestrel; the triband antenna is in
design; the GNSS part is **not yet selected**. This README will not claim otherwise, and
`capabilities.toml` marks those capabilities `planned` rather than `implemented`.

## Capability claims are contracts, not prose

[`capabilities.toml`](capabilities.toml) is **the** single source of truth for what each
profile does and does not do. Any public claim contradicting it is a **build defect**, not a
docs nit. Requirements live in `requirements/` and are bound to refuting tests.

Statuses are **per profile**. That is what makes `bluetooth = "none"` on the opensource cut
enforceable by the build rather than by memory.

The `contract-trace` gate (`engine/tests/contract_trace.rs`) enforces four rules — including
an explicit **anti-vacuity guard**, added because the schema-1 gate, run against schema 2,
found zero capability statuses and *passed while checking nothing*. A test that cannot fail
is not a test.

Refutation-checked in both directions: omitting a profile from a status table fails RULE 3;
marking `bluetooth` **`implemented`** on the opensource profile fails RULE 2 **by name**.

## Lineage

Supersedes `github.com/cybrrd/brrdfeeder` (63 commits, 2026-04-03 → 2026-07-19, final
`ef0eec2`). Predecessors are **archived, not deleted** — they remain readable as evidence.
Full disposition table and carry-forward list: [ADR 0001](docs/decisions/0001-consolidated-lineage.md).

## Layout

```
docs/decisions/     architecture decision records
docs/findings/      empirically-earned engineering findings (measured, not inferred)
requirements/       governed requirements ledger (EARS), bound to refuting tests
profiles/           per-profile build configuration
capabilities.toml   THE capability manifest — single source of truth
```

## The flock

cardinal · robin · cathartes-aura — fielded opensource nodes, cooperative detection.
**kestrel** — the SOHO reference candidate. **Sýc** (Tengmalm's Owl, *Sýc rousný*) — the
void-listener: bearings on aircraft that transmit no Remote ID and no Bluetooth. Named for
the owl whose asymmetric ear placement yields a three-dimensional fix on prey that never
announces itself.
