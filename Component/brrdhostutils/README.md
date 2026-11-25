# brrdtools

Command-line management tools for [BRRDfeeder](https://get.cybrrd.com), the open drone Remote ID sensor from cyBRRD.

> **Status: pre-alpha.** This repository is under active early development. Interfaces, file formats, and command names will change without notice.

## What this is

A BRRDfeeder installs in one command, but until now it has had almost no way to manage it afterwards. brrdtools fills that gap. It lets an operator see what a feeder is doing, diagnose why it is not doing it, and change its hardware or settings without reinstalling.

The package provides two front ends over one shared library:

| Tool | Kind | For |
|---|---|---|
| `brrd-config` | Full-screen terminal interface (TUI) | Operators who want guided menus, in the spirit of `raspi-config` |
| `brrdctl` | Command-line interface | Power users, scripts, automation, and support |

Both call the same core operations. `brrd-config` fills in the flags and parameters for you; `brrdctl` exposes all of them. Anything the TUI can do, the CLI can do.

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
- **Packaging:** a Debian package (`.deb`) for ARM64 single-board computers running Debian 13
- **Workspace layout (planned):**

```
brrdtools/
├── crates/
│   ├── brrd-core/      # shared library: all operations
│   ├── brrdctl/        # CLI front end
│   └── brrd-config/    # TUI front end
├── openspec/           # specifications and change proposals
└── README.md
```

## How we develop

brrdtools is built spec-first, in small changes. Each change starts as a written proposal, becomes a scenario, and lands with a test that proves it. Screen output is verified with text snapshots, so every interface change is reviewable as a diff.

## Building

Build instructions will be added with the first working crate.

```bash
cargo build --release
```

## Related

- [get.cybrrd.com](https://get.cybrrd.com) — install BRRDfeeder
- [globe.cybrrd.com](https://globe.cybrrd.com) — the free public live map
- [www.cybrrd.com](https://www.cybrrd.com) — cyBRRD

## License

Copyright © 2026 cyBRRD Corporation. All rights reserved.

A license for this repository will be published before public release. Until then, no license is granted to use, copy, modify, or distribute this software.
