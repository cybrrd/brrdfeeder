<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Proposal

## Why

A BRRDfeeder has no on-device tool that shows an operator what the unit is doing. This change builds the smallest useful piece of `brrd-config`: the screen frame and navigation that every later screen will live in, and one live page, so that the layout, the shared-library boundary and the snapshot-test method are all proven on something small before more is added.

## What Changes

- `brrd-config` becomes a working full-screen program with a fixed frame: a header with the program name and numbered section shortcuts, a status line, a body, and a key hint line.
- Five sections exist: Status, Configuration, Events, Advanced Tools, About. Number keys jump between them; `0` exits.
- Inside a section, one list is navigated with the arrow keys; Enter or right arrow opens an item; Esc or left arrow goes back.
- One page is implemented: Status > System Info, showing CPU, memory and disk state, refreshing while it is open.
- `brrd-core` gains its first operation: a read-only system information snapshot. `brrd-config` displays it and contains no measurement logic of its own.
- `brrdctl` gains its first command, `brrdctl status --system`, which prints the same system information once and exits. It proves that both front ends are served by the one library.
- A section lists only the items that work in this build. Sections with nothing built say so.
- Screen output is covered by text snapshot tests at 80×24.

Not in this change: the unit's license plate, feeder and radio state, any setting that can be changed, a process list, machine-readable (`--json`) output, any other `brrdctl` command, Debian packaging, and ARM64 release builds.

## Capabilities

### New Capabilities

- `brrd-config-shell`: the frame, sections, navigation keys, exit behavior and terminal-size behavior that every `brrd-config` screen shares.
- `system-info`: what the system information snapshot reports about the host (CPU, memory, disks), how fresh it is, and how a value that cannot be read is presented, in either tool.
- `brrdctl-cli`: how `brrdctl` is invoked: its help, its commands, and how it reports a mistake in the command line.

### Modified Capabilities

None.

## Impact

- `crates/brrd-core`: first public operation and its data types.
- `crates/brrd-config`: replaced placeholder with the program described above.
- `crates/brrdctl`: replaced placeholder with a command-line program offering `status --system`.
- New dependencies: `ratatui`, `crossterm` (terminal interface), `sysinfo` and `rustix` (host measurements), `clap` (command-line parsing), `insta` (snapshot tests, development only).
- No effect on a running feeder: the change reads host statistics and writes nothing.
