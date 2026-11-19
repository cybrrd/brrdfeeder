<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Fresh 0.9.0 helper 4 → 5 qualification kit

**UNBOUND / NATIVE NOT_RUN.** This kit prepares the first 0.9.1 item. Offline
fixtures are not native qualification. There is no assigned host, operator,
window, OOB path or permission to execute this runbook in the implementation PR.
The earlier 0.8.31 helper-4 kit is immutable historical evidence, not this kit.
This operation does not migrate engine policy, renew credentials, update images,
publish a signed feed, or prove physical power-cut safety.

## Assemble and bind

From a clean reviewed source commit, with Go 1.27.3 installed:

```sh
bash Component/aviary/release-system/build-host.sh /tmp/helper5-artifact
python3 Component/aviary/release-system/qualification/helper5/package.py \
  --helper /tmp/helper5-artifact/brrdfeeder-release-arm64 --output /tmp/helper5-kit
```

The package contains the new installer/helper/launcher, the exact released
0.9.0 installer from `9faae55f53e27cea198401ee34b4c66c1898666a`, source provenance,
licenses, checksums and an unbound template. A dirty-source package is rehearsal
material only. Archive it and independently record the archive/SHA256SUMS hashes.
Do not use the new installer as the old fresh-install baseline. No workload image
pins or product version are changed by this PR.

An operator must later approve and observe an actual fresh released 0.9.0 node.
Fill `binding.template.json` into a new `binding.json` with the actual full
hostname, node ID, operator, window, OOB recovery path and physical asset. Retain
all compiled source/artifact hashes. Verify the real helper SHA/build against the
released installer and require effective floor 4. Confirm the baseline's release,
installer, source and engine/console image provenance independently. Both system
and service runtime credentials must have been created naturally by the real
fresh install. Never synthesize credentials, a node identity or a missing floor.

Register the complete binding hash **off-box before mutation**. Export that
independently accepted value as `HELPER5_BINDING_SHA256` in the approved native
root shell. Every kit entry point requires the binding and checks the packaged
artifact hashes. Put the kit in a new root-owned, non-writable-by-others directory
on the approved ARM64 host; preserve the checksummed original archive off-box.
All commands below are future operator instructions, not executed evidence.

## R00–R02: admission, quiescence and registration

1. Verify `sha256sum -c SHA256SUMS`, the accepted binding hash, the current host
   and node, physical/OOB access and named window. Record current boot ID and
   fresh radio, GPS, trusted clock, engine, console and ordinary broker health.
   No renewal or enrollment may overlap the protected-state window.
2. Save enabled/active states of `brrdfeeder-release-poll.timer` and
   `brrdfeeder-host-update.timer`, then stop and disable those triggers. Let work
   finish. Require inactive poll/host-update/updater workers, no pending request,
   no host/workload transaction and available locks. Record pre-existing failed
   states; do not reset failed units to improve receipts.
3. Under `launch.lock` then `lock`, register six helper preimages:

   ```sh
   flock -n /var/lib/brrdfeeder-updater/launch.lock \
     flock -n /var/lib/brrdfeeder-updater/lock \
     python3 -B helper-preimages.py save "$PWD/preimages"
   ```

   Export this manifest and register its hash independently off-box. Set
   `HELPER5_PREIMAGES_SHA256` to that accepted hash. The six targets are the helper,
   its receipt, host-state JSON (including absence), retained `.previous`, retained
   checksum, and launcher. No credentials or spools are saved by this companion.
4. Run `python3 -B fixture.py capture-protected`. Independently register/export
   its metadata/hash-only `protected.json`. It covers config/credential hashes,
   updater/workload state, accounts, units and pins. It must never contain JWTs,
   seeds or config payloads. Preserve advancing spools; do not snapshot/restore
   their history. Capture service invocation IDs for the no-reboot window.
5. Create `registration.json` off-box with `binding` (the exact accepted object),
   `preimages_sha256` and `protected_sha256`; independently retain its hash before
   testing. This file is the validator's authority, not a self-generated PASS.

## Native syscall calibration

On this actual host/toolchain, compile `calibrate.c` with `cc -pthread`. Create a
new empty evidence directory and run:

```sh
cc -pthread -O0 -g calibrate.c -o calibrate
mkdir -m 700 evidence/calibration
B03_MODE=calibrate B03_GDB_OUT="$PWD/evidence/calibration" \
  unshare --net -- gdb -nx -q -batch -x "$PWD/gdb-fsync.py" \
  --args "$PWD/calibrate" "$PWD/evidence/calibration"
```

Require native ARM64, syscall 82, x0 argument/x0 result, two threads and all 18
paired entry/return observations (directory success, regular-file success and
EBADF, three each per thread). A mismatch stops qualification. Portable or
another host's calibration cannot authorize this run. Retain events, pairs,
expected records, result and complete GDB stdout, including failures.

## R03–R04: ordinary upgrade and ten fault rows

Source `operator-functions.sh` only after the preceding registration. It checks
quiescence and protected state and uses exact local artifacts inside
`unshare --net`. `normal_trial normal` performs normal and repeated installation,
traces descendants, requires no AF_INET/AF_INET6 calls, verifies helper/floor 5,
and compares the six installed-file snapshots for idempotence. `restore_baseline`
then restores registered helper 4 for the next trial.

Run each of these ten rows separately; stop on the first failure and retain the
entire failed attempt. The helper is killed at the successful **directory fsync
return**, with all threads stopped and no intervening continue:

| Phase | Binary / receipt / floor / journal at kill | Launcher recovery | Direct preimages |
|---|---|---|---|
| prepared | 4 / 4 / exact old / present | coherent 4 | registered six |
| switched | 5 / 4 / exact old / present | coherent 4 | registered six |
| receipted | 5 / 5 / exact old / present | coherent 4 | registered six |
| floored | 5 / 5 / 5 / present | coherent 4 | registered six |
| committed | 5 / 5 / 5 / absent | remain 5, then R06 | registered six |

Use `boundary_trial PHASE launcher` and `boundary_trial PHASE preimages` for each
phase. Every row starts from independently verified registered 4 and ends with
R06. Prepared may already contain the compatible launcher and retained helper.
Earlier-phase launcher recovery may leave that compatible launcher; R06 restores
all six original targets, with the old launcher last.

At each stop, retain the actual floor bytes (or a zero-byte evidence file for
absence) and the adapter's snapshot, GDB events, at-stop and result files. This
must happen **before** recovery, so `fault_trial` writes `floor-at-stop.json`.
Capture independent health and service observations before and after each row.
No-reboot rows must preserve service invocation IDs and expected service state.
A local-file upgrade must have complete descendant exec/network traces; the
public download path is a separate network-using delivery path.

## R05: two real boot cases

Each requires separately named reboot permission in the approved window.

- Interrupted: start at registered 4, `fault_trial floored reboot-interrupted`,
  save `/proc/sys/kernel/random/boot_id` to that row's `boot-id-before`, then
  reboot without manually invoking recovery. Run `after_boot interrupted` only
  after reconnection. Require a different boot ID, the actual current-boot
  recovery service invocation, successful result and local restoration message,
  helper 4, exact prior floor/absence and no journal. Then R06.
- Committed: start at registered 4, complete normal/idempotent 5, create a new
  `reboot-committed` evidence directory and record boot ID. Reboot, then
  `after_boot committed`. Require successful actual recovery-unit execution,
  helper/floor 5 and no uncommitted-restoration message. Then R06.

Do not use a boot filename or a successful manual launcher call as proof that
boot recovery ran. Retain boot journal, invocation ID, status, monotonic start
and current boot ID. Health/service observations after reboot have new invocations.

## R06–R07: exceptional restore and handback

`restore_baseline` is a named qualification exception, not a public downgrade
command. It checks the independent preimage hash, all six preimages/destinations,
known old/candidate bytes, exact recognized journal including prior floor, and
absence of unrelated host-floor changes **before the first mutation**. It restores
helper/receipt/floor/retained files, removes only the recognized trial journal,
then restores the old launcher last. A refusal requires operator/OOB recovery;
never weaken hashes/guards or overwrite credentials/spools to continue.

Export the six-file preimage manifest again under a new evidence directory after
each R06; it must match the original exactly. Register final self-test, helper
4/SHA, effective floor 4, original floor bytes/absence, no journal, all protected
state, and fresh radio/GPS/time/engine/console/broker health. Restore the original
timer states and observe a normal cycle. Default handback is helper 4. Keeping
helper 5 requires separate acceptance of its exact hash and a separately reviewed
handback contract; this validator deliberately requires baseline restoration.

## Evidence validator and limitations

`validate.py EVIDENCE.json registration.json --registration-sha256 ACCEPTED_HASH`
checks the independently registered binding, actual hash-addressed calibration
and stop captures, all ten phase/route rows, normal/idempotent traces, two boot
records, six-preimage restoration and fresh health (within 120 seconds of each
boundary). Artifact references use `{ "path": "relative/path", "sha256": "…" }`.
The executable synthetic example/schema is `test_validator.py`: its complete
fixture passes structural validation; deliberately altered identities, builds,
hashes, floors, routes, phase omissions, stale health, fake unchanged boot IDs,
missing/network-using traces and syscall pairing are refused.

Normalize the observed health, final state and boot-service receipts into the
fields required by the validator, retaining and referencing the raw captures.
Boolean health witnesses and normalized service receipts still require independent
operator review against the originals; this tool cannot authenticate a fabricated
operator statement. Its strongest output is `EVIDENCE_COMPLETE_REVIEW_REQUIRED`,
never native PASS. Native execution, independent evidence review and release
acceptance remain pending. Do not treat offline fixtures or PASS filenames as
native receipts. No signed automatic helper-feed publication is implied.
