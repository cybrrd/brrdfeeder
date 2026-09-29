<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Offline shipped-code gate

Both `.github/workflows/test.yml` and `.gitea/workflows/package-console.yml`
invoke `python3 Component/brrdfeeder/install/tests/run.py --group all`.
`--list` prints the full catalog; `--out DIRECTORY` retains per-suite logs,
counts, failures and explicit skips. A missing dependency is a failure, not a skip.

The gate includes the original nine installer CI suites, BLE/rfkill, one-liner,
simple uninstall, bounded installer completion, self-update installer and host
updater, console Go tests, Rust workspace, mutation controls, image-identity and
build-procedure guards, and disposable OS/broker/Podman runtime fixtures.
Python suite discovery has a contract of its own: adding an uncatalogued
`test-*.py` fails the gate.

The root-only ownership/naming/legacy-installer tests run in disposable,
network-disabled Podman containers. The updater runtime test uses separate
temporary rootless stores, synthetic engine status, real console code, mocked
systemd/registry effectors and no host installations. Broker tests bind only
loopback. Hardware/RF acceptance is not claimed by this gate.

Provisioning downloads the exact dependencies in `.github/scripts/setup-tests.sh`
before the offline run. Locally provide `TEST_OS_IMAGE` (an already built immutable
fixture image) and the pinned NATS image. Tests require Python/YAML, Bash, jq,
udevadm, a Quadlet generator, Rust 1.88.0, Go 1.27.0, C headers and rootless Podman.
Do not run the guarded root fixture programs directly on an operator's machine.

Only test code and necessary synthetic/frozen fixtures are shipped here.
Private fleet/history/Caddy and signer-source comparisons stay in the private
gate. Historical host-bound build/RF acceptance and evidence validators are not
portable contracts; current product assertions live in these suites and the
component-local Go/Rust tests. Review reports and evidence logs are not shipped.

Frozen fixtures under `uninstall/fixtures` and `d44-rework/fixtures` replace
private Git-object reads; they retain the pre-change enrollment/bootstrap
comparisons with only the explicitly registered public-namespace substitutions.
