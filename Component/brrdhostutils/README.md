<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Host utilities

`Component/brrdhostutils` contains brrdtools: the two user-driven utilities
`brrd-config` and `brrdctl` for [BRRDfeeder](https://get.cybrrd.com), the open
drone Remote ID sensor from cyBRRD.

> **Status: pre-alpha.** This component is under active early development. Interfaces, file formats, and command names will change without notice.

## What this is

Today, brrdtools shows read-only CPU, memory, swap and disk information.
Use Status > System Info in `brrd-config` or `brrdctl status --system`.
Diagnostics and changes to hardware or settings are planned.

The utilities provide two front ends over one shared library:

| Tool | Kind | For |
|---|---|---|
| `brrd-config` | Full-screen terminal interface (TUI) | Operators who want guided menus, in the spirit of `raspi-config` |
| `brrdctl` | Command-line interface | Power users, scripts, automation, and support |

Both use the shared system-information reader. The TUI refreshes the report
once a second; the CLI prints it once and exits.

The Debian package will be `brrdhostutils.deb`, containing these utilities and
supporting files. The containers are a separate download.
The host service `brrd-hostd` is planned for that package.

## Design principles

- **One core, two front ends.** Every capability lives in the library. Neither front end contains logic the other lacks.
- **Declared versus observed.** The feeder's configuration states what should be true. Its status reports what is true. The tools show the difference and reconcile it.
- **One source of truth for devices.** Supported radios and receivers are described in one place, and everything else is derived from it.
- **Read before write.** Inspection commands change nothing. Commands that modify a feeder say what they will do first.
- **Say so when it is non-standard.** A feeder running outside the recommended configuration is reported as such, clearly, to the operator.
- **Works on a plain terminal.** 80×24, SSH, and limited-color consoles are supported targets, not afterthoughts.

## Planned capabilities

- [ ] `status` — what is running, which devices are bound, what each last heard, uplink health
- [ ] `doctor` — checks for the known causes of a feeder not capturing, each with a plain-language reason
- [ ] `detect` / `reconfigure` — recognize a swapped or added device and rebind it without a reinstall
- [ ] Power and USB health reporting
- [ ] `brrd-config` screens over each of the above

## Technology

- **Language:** Rust
- **TUI:** ratatui with the crossterm backend
- **Planned packaging:** a Debian package (`brrdhostutils.deb`) for ARM64 single-board computers running Debian 13
- **Workspace layout:**

```
Component/brrdhostutils/
├── crates/
│   ├── brrd-core/      # shared library: all operations
│   ├── brrdctl/        # CLI front end
│   └── brrd-config/    # TUI front end
├── openspec/           # specifications and change proposals
└── README.md
```

## How we develop

brrdtools is built spec-first, in small changes. Each change starts as a written proposal, becomes a scenario, and lands with a test that proves it. Screen output is verified with text snapshots, so every interface change is reviewable as a diff.

## Building and running

On Linux, install the Rust toolchain selected by `rust-toolchain.toml`.
Build and launch the terminal interface:

```bash
cargo build --workspace
cargo run -p brrd-config
```

Use an 80×24 or larger terminal. Enter opens System Info in Status. The page
refreshes once a second; CPU usage says `measuring` until its second sample.
Use 1–5 to change sections, Esc to go back, and 0, q or Ctrl-C to exit.
No administrator rights or feeder software are required.

Print the same report once and exit:

```bash
cargo run -p brrdctl -- status --system
```

The installed command is `brrdctl status --system`. It waits briefly for a CPU
sample and prints plain text suitable for a file or pipe.

Run the checks (the terminal integration tests also require Python 3):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Related

- [get.cybrrd.com](https://get.cybrrd.com) — install BRRDfeeder
- [globe.cybrrd.com](https://globe.cybrrd.com) — the free public live map
- [www.cybrrd.com](https://www.cybrrd.com) — cyBRRD

## License

Copyright © 2026 Macawi LLC.

Licensed under AGPL-3.0-or-later. See the repository
[LICENSE](../../LICENSE).
