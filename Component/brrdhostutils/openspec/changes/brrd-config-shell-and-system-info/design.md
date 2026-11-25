# Design

## Context

The workspace has three empty crates: `brrd-core` (library), `brrdctl` and `brrd-config` (programs). See proposal.md for why this change comes first.

Five open-source projects were read before this design was written. What each contributed:

| Project | What was taken |
|---|---|
| Ratatui templates (`ratatui/templates`) | The event-driven program shape; separating a key press from what it means; one common interface per screen. |
| `raspi-config` | Movement: one list, arrows, Enter in, Esc back. |
| `netwatch` | The fixed frame: header with numbered sections, status line, key hint line. Stating why a value is missing instead of leaving a blank. |
| `systemctl-tui` | Confirmation of the crate layout: it is also one core library with two front ends. A warning as well: its main screen file has grown to about 3,800 lines. |
| `bottom` | Where host measurements come from: the `sysinfo` crate, with direct reads of `/proc` and `/sys` only where that crate falls short. |

## Goals / Non-Goals

**Goals:**

- A new page can be added by writing one file and adding one entry to the menu, without editing the frame or any other page.
- Every screen can be drawn from fixed data in a test, with no terminal and no real host readings.
- `brrd-config` contains no measurement logic. Everything it shows comes from `brrd-core`.

**Non-Goals:**

- Asynchronous code. Nothing in this change waits on a network.
- Configurable keys, color themes, mouse support, a help panel.
- Running on anything other than Linux.
- Machine-readable output from `brrdctl`. It will be wanted; it is a separate change.

## Decisions

### 1. Synchronous event loop with one background thread

Follow the Ratatui event-driven template: a background thread sends two kinds of message over a channel, terminal input and a periodic tick; the main thread owns all state, handles one message, then redraws.

The tick is once a second, not the template's thirty. A status page has no animation, and a feeder's processor has better things to do.

*Alternative considered:* the async variant with `tokio`. Rejected for now: it adds a large dependency and a harder mental model for no benefit while every data source is a local read.

### 2. Key, then intent, then effect

A key press is first translated into an intent (`MoveDown`, `Open`, `Back`, `JumpTo(section)`, `Quit`). Only intents change state. The translation depends on the current screen, which is also what produces the key hint line, so the hints cannot drift from the real behavior.

### 3. The menu is data

Sections and their items are described in one table: section, item name, and the page that item opens. The frame draws lists from that table. An item that is not built is simply absent from the table, which is how "a section lists only what works" is met without special cases.

### 4. One interface per page, one file per page

Each page implements a small interface: receive a tick, receive an intent, draw into a given area, and report its key hints. System Info is the first implementation. The frame knows nothing about what a page shows.

### 5. `brrd-core` returns a snapshot; pages never measure

`brrd-core` exposes one operation that returns a system information snapshot. Each field is either a value or "unavailable"; CPU usage has a third state, "measuring", for the first reading. The operation never fails as a whole.

`brrd-config` obtains snapshots through an interface that tests replace with a fixed source. This is what makes snapshot tests repeatable.

### 6. `sysinfo` for host measurements

Use the `sysinfo` crate with only the features needed (system, disk). It is what `bottom` uses, it is MIT licensed, and it avoids hand-written parsing of `/proc`.

`rustix` is used alongside it for filesystem sizes, because `sysinfo` does not report why a size could not be read and the specification forbids guessing.

Disk filtering is ours to get right: `sysinfo` reports what is mounted, and the specification excludes memory-backed and kernel filesystems. The filter lives in `brrd-core` and has its own unit tests.

### 7. Snapshot tests with `insta` and Ratatui's test backend

Each scenario in the `brrd-config-shell` specification that describes a screen becomes a test that draws into an in-memory 80×24 buffer and compares the text with a stored snapshot. A change to any screen then appears in review as a text diff.

### 8. Text formatting lives in `brrd-core`

The words and units for each value (`3.7 GiB`, `unavailable`, `measuring`, `none`) are produced by `brrd-core`, not by either front end. `brrd-config` lays them out on a page; `brrdctl` prints them as lines. This is what keeps the two tools from disagreeing.

`brrdctl` is a one-shot program and cannot show `measuring`, so it takes two CPU samples a short interval apart before printing.

### 9. `clap` for the command line

Use `clap` with its derive interface. It supplies help, version and error messages with exit status 2 for a bad command line, which is most of the `brrdctl-cli` specification, and it is the common choice in Rust command-line tools.

### 10. Pinned versions

`ratatui` 0.30, `crossterm` 0.29, `sysinfo` 0.40, `clap` 4, `insta` 1.49 (development only). `Cargo.lock` is committed. Each crate is marked `publish = false`.

## Risks / Trade-offs

- [Container overlay and bind mounts appear as disks on a feeder] → Decide the filter from a real feeder's mount table, not from a development machine; covered by a task.
- [The header does not fit 80 columns with full section names] → Short labels in the header (`config`, `tools`); the full name appears in the body path. Checked by a snapshot test at exactly 80 columns.
- [A failure leaves the terminal unusable] → Restore the terminal on every exit path, including a panic; covered by a test that provokes one.
- [One page file grows without limit, as seen elsewhere] → One file per page from the start, and the frame kept ignorant of page content.
- [Development machine differs from a feeder] → The final task group runs the program on a real unit.

- [A development machine has no containers and tests use fixed numbers, so wrong disk figures pass every test] → Found on the first run on a real feeder: a second mount of the root device was listed as a disk, and reserved space was counted as used. The specification now has a scenario for each, and one that compares with `df`.

## Open Questions

- Exact wording and symbols of the header and hint line. Settled by reviewing the first snapshots.
- Whether list movement should wrap from last to first. The specification says it stops; revisit after use.
