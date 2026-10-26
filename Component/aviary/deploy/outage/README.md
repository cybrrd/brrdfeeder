<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Durable outage custody

The telemetry supervisor stores every accepted outbound message on disk before
returning success to its producer. This includes telemetry, heartbeat, drop audit,
substrate audit and enabled Green ticks. Stored subject and payload bytes are
replayed unchanged, oldest first. The v4 telemetry model and goldens are unchanged.

```yaml
backhaul:
  outage_spool:
    directory: /var/lib/brrdfeeder/outage
    cap_bytes: 2147483648
```

These defaults apply when the configuration block is absent. The installer
requires the canonical directory so teardown can validate every owned file.
The cap is configurable. It bounds physical segment file lengths, including
confirmed prefixes and corrupt fragments still retained. Allow additional space
for filesystem metadata, block rounding, two 64-KiB gap slots, the cursor and its
temporary replacement. Segments rotate at 1 MiB; individual records are limited
to 1 MiB. A directory lock excludes simultaneous writers. Files are private
(0600, directory 0700), and file opens refuse symlinks.

## Full, interrupted, or corrupt storage

At the cap, append stops. Existing bytes are never evicted to make room. Each
refused observation adds to a persistent per-boundary gap containing the exact
count and minimum/maximum capture timestamps in Unix milliseconds. The ledger
preallocates two checksummed slots before capture starts; updates overwrite the
alternate slot and sync it. This permits gap persistence during ordinary ENOSPC
without allocating another file. A read-only or failed device can still prevent
persistence: this is logged explicitly, counts remain in memory, and health stays
impaired. That condition cannot promise count survival through a subsequent crash.

The additive Red code is `telemetry_dropped`. The approved optional
`telemetry_gap` object appears in Red diagnostics and operational reports:

```json
{"boundary":"outage_spool","count":137,"first_capture_ms":1800000000000,"last_capture_ms":1800000005440}
```

Red is scheduled at cumulative counts 1, 10, 100, 1000, and so on per boundary.
Each alert retains the exact count and capture range at that threshold; operational
reports carry the latest cumulative gap. Pending alerts live in the gap ledger,
independently of the 32-record Red queue. A full Red queue delays their admission;
it cannot erase them. Only a confirmed JetStream PubAck retires a pending alert.
Stable message IDs allow retries across restarts. Coverage remains impaired while
a boundary is dropping or storage is faulty. Successful admission clears that
boundary's active impairment (including `ProducerHandoff` only after durable
admission), while historical counts and unacknowledged alerts
remain. Capture timestamps can be untrusted until the node clock is disciplined;
the existing clock-trust field remains authoritative.

Capture retains its bounded 1024-item `try_send` handoff, avoiding a wait for
network or producer capacity in the radio loop. Full and closed paths both count
loss and persist the gap; that exceptional accounting does perform synchronous
disk I/O. The second 1024-item producer handoff awaits capacity and a disk receipt.
A dedicated disk thread keeps accepting records during network connects, timeouts,
and reconnect backoff. Drop-audit snapshots are subtracted only after durable
acceptance; a refused audit retains its original window for retry.

Replay leases at most 256 records **and 4 MiB of stored bytes** per batch,
including record headers and subjects. The next header is validated and its size
checked against the remaining byte budget before its body is read or allocated.
A legal 1-MiB record always fits the production budget; a caller-supplied budget
too small for the next record fails explicitly instead of stalling or overshooting.
Decoding adds at most one temporary 1-MiB body plus bounded record metadata.
The 4-MiB lease is 1/64 of the engine's 256-MiB/no-swap service cap, leaving room
for capture, client buffers, conmon and charged tmpfs. This bounds replay memory;
it does not qualify arbitrary native-radio workloads or all other engine queues.

Replay leases records without removing them from disk. A failed publish retries
the same lease; there is no prepend operation that can overshoot the cap. After
ordered publishes, the existing Blue stream-info request provides a broker
round trip on the same connection. Connection-generation changes invalidate the
confirmation. This avoids treating async-nats 0.50's local socket `flush()` as a
broker acknowledgement. The control stream and its existing stream-info permission
must be available; otherwise custody remains on disk. This is broker-receipt
confirmation, not permanent lake storage. Duplicates are possible around failures.

The cursor is atomically replaced and synced before confirmed segments are
unlinked. Reopening scans checksums and record boundaries. A corrupt tail is
retained byte-for-byte while its valid prefix can replay; later segments can
continue replay. Corrupt cursor or gap files fail startup without deletion.
Operators must preserve corrupt evidence and reconcile it before removing files.
Uninstall explicitly destroys owned outage state after its normal validation;
export needed evidence before an intentional uninstall.

## Durability, size and wear

There is no intentional batching delay: each append calls `sync_data()` before
acceptance, and segment creation also syncs the file and directory. The measured
append interval includes creation/rotation. The acceptance drill asserts a maximum
of 250 ms on its local disk; it is not a hardware-independent latency guarantee.
Power-loss survival depends on the filesystem/device honoring sync. In-flight
capture and producer queues remain volatile until the disk acceptance boundary.

The synthetic ASTM fixture is parsed and serialized through the actual v4 model:
656 payload bytes + 36 subject bytes + 46 record-header bytes = **738 bytes**.
The header contains length, subject length, capture timestamp and SHA-256.
These figures count telemetry only; other outbound traffic consumes the same cap.

| Observations/s | 5 minutes: records / bytes | 1 hour: records / bytes | 24 hours: records / bytes |
| ---: | ---: | ---: | ---: |
| 1 | 300 / 221,400 | 3,600 / 2,656,800 | 86,400 / 63,763,200 |
| 5 | 1,500 / 1,107,000 | 18,000 / 13,284,000 | 432,000 / 318,816,000 |
| 25 | 7,500 / 5,535,000 | 90,000 / 66,420,000 | 2,160,000 / 1,594,080,000 |

At this measured size, 2 GiB holds about 32.3 hours at 25/s before overhead and
other traffic. At a 1-KiB stored record, the same rate writes about 2.21 GB/day
and fills 2 GiB in 23.3 hours. Thus “about 24 hours” is a sizing estimate, not a
universal frame-size promise. All accepted traffic now writes disk even while
online. Filesystem journal, cursor replacement, block rounding, gap updates and
flash write amplification add wear beyond the logical byte totals. Per-record
sync trades write amplification for the smallest software durability window.

## Reproducing acceptance

Use an external `CARGO_TARGET_DIR` so generated objects do not enter repository
license checks. Standard `cargo test --offline --locked --workspace` covers append,
rotation, restart scan, cap/resume, lease capacity, corrupt-tail preservation,
gap recovery, full Red queue, producer backpressure, retained audits, fake-broker
replay, missing confirmation and SIGKILL recovery. Loopback sockets are required.

Run `python3 engine/tests/run-broker-tests.py` from `Component/aviary` with the
cached pinned NATS image available for the real-broker confirmation and existing
Red/Silver transport tests. It never contacts a production broker.

Run the focused replay-memory proof on a real local filesystem, using the compiled
engine unit-test binary printed by `cargo test -p engine --no-run`:

```sh
python3 deploy/outage/replay_memory_proof.py /path/to/engine-test-binary /path/to/new-evidence-dir
```

It seeds the reviewer's legal 256-record/256-MiB backlog, then retries it in three
fresh processes and drains all records byte-identically in a fourth. A transient
user service enforces `MemoryMax=256M`, `MemorySwapMax=0` around actual Podman/conmon
and the Rust fixture. Each replay process reserves/touches 128 MiB for other engine
state and 16 MiB of tmpfs. The proof checks actual kernel limits, observes Podman
and conmon in the service cgroup, requires peak use below 224 MiB and zero OOM
counters, and records process RSS/high-water marks. It uses a cached local image,
no network and a private disk backlog. This is synthetic headroom evidence, not
native Pi qualification. Standard tests cover the header-before-body check,
subject/header accounting, large records across restart, and ProducerHandoff
recovery/refusal with historical gaps and pending alerts retained.

Run the hour drill separately on a real local filesystem (not `/tmp` if tmpfs):

```sh
B06_TEST_ROOT=/path/to/private/disk-evidence cargo test --offline --locked -p engine \
  broker_down_25_per_second_for_60_minutes -- --ignored --nocapture
```

It accepts 90,000 frames at 40-ms intervals, then requires all 90,000 identical
payloads in order and a zero remaining spool. The ENOSPC test
`outage_spool::tests::actual_enospc_preserves_oldest_and_preallocated_gap_counts`
must run with `B06_ENOSPC_ROOT` pointing to an isolated small tmpfs, e.g. an
8-MiB mount inside a network-disabled disposable container. Never point it at a
host filesystem: it deliberately fills its supplied mount. It requires real
kernel ENOSPC, exact persistent counts, preserved oldest bytes and resumed append
after the filler is released. The ignored crash-child test is launched only by
its SIGKILL parent and is not an independent acceptance run.

See [memory deployment](../memory/README.md) for boot-controller repair, verification
and the local restart-policy proof. Local fixtures do not constitute Pi deployment,
physical power-cut testing or native-radio memory qualification.
