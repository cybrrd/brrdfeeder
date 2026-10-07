<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Engine memory bound and Silver observations (0.8.29)

The shipping installer renders a rootful system Quadlet with explicit
`CgroupsMode=split`, `[Service] MemoryMax=256M`, `MemorySwapMax=0`, and
`OOMPolicy=kill`. The service bound includes the engine, conmon, and charged
container tmpfs. It does not cap unrelated host services. 256 MiB leaves headroom
above the measured 12–14 MB engine RSS, roughly 5 MB conmon, and the 50 MiB capture
tmpfs. This is a starting safety bound, not a measured workload maximum.

This deliberately uses systemd resource control in **[Service]**, never
`[Container]`. Podman's split mode keeps the payload below the service's cgroup;
using a separate Podman scope would make a launcher-only limit ineffective.
See the [Podman Quadlet cgroup contract](https://docs.podman.io/en/v5.4.2/markdown/podman-systemd.unit.5.html#cgroupsmode)
and [systemd OOMPolicy](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html#OOMPolicy=).
The rootless console keeps its existing conditional 96 MiB Podman limit: lack
of delegation must not prevent its startup.

An OOM kill cannot gracefully drain the killed process. “Clean restart” here
means systemd kills the group, runs the receipt/Quadlet cleanup hooks, then starts
a new invocation after five seconds. Three starts in 300 seconds hit the start
limit and leave the engine failed instead of looping indefinitely. Journald
records the OOM result and restart decision. The host helper logs an explicit
event and atomically fsyncs the count in root-owned
`/var/lib/brrdfeeder-memory/events.json`, once per invocation. Ordinary exits,
signals, timeouts and a duplicate stop hook do not increment it. Invalid state
is reported as unavailable; it is never silently reset. The receipt survives
engine restarts and host reboots; uninstall deliberately removes it.

`memory_cap_events` counts systemd-confirmed OOM deaths while this service is
bounded. It does **not** prove the cap, rather than host-wide memory pressure,
caused each death. It is not a count of `memory.events:max` allocation retries.
The local allocation-fault test separately establishes cap enforcement.

## Configuration and recovery

A root operator may create a service drop-in at
`/etc/systemd/system/brrdfeeder-engine.service.d/memory.conf`:

```ini
[Service]
MemoryMax=384M
```

Reload systemd and restart the engine only during an approved maintenance
window. Inspect the generated service, effective `MemoryMax`, cgroup containment,
and journal after changing it. Do not remove the limit to conceal a leak. After
diagnosing repeated OOMs, `systemctl reset-failed brrdfeeder-engine.service` and
an explicit start release the start-limit latch. A custom drop-in remains
operator-owned; remove/reconcile it before the conservative uninstaller, which
refuses unexpected overrides. No new installer/bootstrap/product version is
introduced by this PR, and no bootstrap installer hash is regenerated here.
The 0.8.29 integration must regenerate its pins only after the held 0.8.28
installer has shipped and the stack has been integrated.

## Additive heartbeat contract

`heartbeat.memory` is optional in local status, and `memory` is optional in the
NATS heartbeat. Existing schema/version fields are unchanged. The object schema
is [memory.schema.json](memory.schema.json); every property is independently
optional. Values are unsigned integers; absent is unknown, not zero or `null`.
Consumers must ignore unknown properties. These are observations, not release
authorization, attestation, or a health verdict.

| Property | Meaning |
| --- | --- |
| `engine_rss_bytes` | Engine process VmRSS × 1024 from its own proc status. |
| `engine_cgroup_current_bytes` | This process's v2 payload cgroup `memory.current`; includes charged cache/tmpfs, not just RSS. |
| `engine_cgroup_peak_bytes` | Payload cgroup `memory.peak` when the kernel exposes it; scope resets with the cgroup. |
| `console_rss_bytes` | Sum of readable `brrdhouse` executable RSS in the dedicated account's exact `brrdhouse.service` cgroup. |
| `host_mem_total_bytes` | Host MemTotal × 1024, measured outside the container. |
| `host_mem_available_bytes` | Host MemAvailable × 1024, measured outside the container. |
| `volatile_journal_bytes` | Allocated blocks × 512 for regular `.journal` / `.journal~` files under `/run/log/journal`; no journal contents, symlinks, or persistent journal counted. |
| `memory_cap_events` | Durable historical service OOM count, with the causality limitation above. |

The host observer is a bounded, network-free oneshot on a 30-second inactive
timer. Its root-owned RAM snapshot is boot-ID-bound and expires after 90 seconds
of CLOCK_BOOTTIME (including suspend); wall-clock adjustments cannot revive it.
The stop hook also refreshes it. Engine and console receive only this snapshot
directory read-only: no host proc tree, journal contents, Podman socket or
credentials. A missing/corrupt/expired snapshot omits host metrics while engine
RSS/cgroup sampling remains independent. Engine-local status and heartbeat use
the same assembled payload; OOM-count changes trigger a local status refresh.

The console may show the independently fresh host OOM count even when engine
status is stale or absent. This never makes the engine “running”. A nonzero count
remains a historical attention item; no alert-clearing or restart control is
added. Host measurements are sampled asynchronously and are not a transactional
system-wide memory snapshot.

## Verification and outstanding native gate

Behavioral RED and acceptance were committed before implementation. Run:

```sh
python3 Component/aviary/deploy/memory/test_memory.py
python3 Component/aviary/deploy/memory/mutate_memory.py
go -C Component/brrdhouse test -race ./...
cargo test --manifest-path Component/aviary/engine/Cargo.toml --locked
```

The opt-in `local_fault_proof.py --out <evidence-directory> --image <cached-python-image>`
creates only a uniquely named transient **user** service/container, verifies
the kernel ancestor's 256 MiB limit before allocating, uses no network/pull,
records one OOM receipt, and requires one restart followed by a 60-second
32-MiB fixture soak. It retains its journal and exact properties. It is not
proof of rootful Pi radio capture or normal native engine memory usage.
With `--repeat-oom`, it instead requires three OOM receipts followed by an
inactive/failed service, a start-limit refusal in the journal, and no restart
across two further restart intervals. systemd may retain `Result=oom-kill`
rather than replace it with `start-limit-hit`.
Warn the shared-host operator **before** invoking either fault mode: memory
shortage alerts can fire even though the test allocation is cgroup-bounded.
Units use `codex-brrd-memory-test-*` names and an explicit `TEST ONLY` description;
the harness removes its own unit/container on completion and retains only proof
files. It does not suppress host monitoring alerts.

The real ARM64 image build, receiver compatibility checks and hosted test
receipts belong in the review handoff. Fielded-node normal-operation soak,
native allocation fault/restart, heartbeat receipt at command, and the console
view after repeated OOMs remain a separate **NOT_RUN** operations gate until
the existing node-contact approval is given. No deployment follows from CI.
